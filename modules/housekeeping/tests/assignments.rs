mod common;

use common::Hotel;
use db::{CHANNEL, Event, UserId};
use housekeeping::{HousekeepingError, copy_yesterday, hk_key, housekeepers, set_assignments};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use time::Duration as Days;
use uuid::Uuid;

impl Hotel {
    async fn set(&self, changes: &[(Uuid, Option<UserId>)]) -> Result<(), HousekeepingError> {
        let mut tx = self.tx().await;
        set_assignments(&mut tx, self.tenant, self.user, self.property, changes).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    async fn copy(&self) -> u32 {
        let mut tx = self.tx().await;
        let copied = copy_yesterday(&mut tx, self.tenant, self.user, self.property).await.unwrap();
        tx.commit().await.unwrap();
        copied
    }

    /// Who is assigned `room` today.
    async fn assignee(&self, room: Uuid) -> Option<Uuid> {
        sqlx::query_scalar("select user_id from hk_assignment where room_id = $1 and business_date = $2")
            .bind(room)
            .bind(self.business_date)
            .fetch_optional(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// Records that `user` had `room` the day before the business date.
    async fn assign_yesterday(&self, room: Uuid, user: UserId) {
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(self.business_date - Days::days(1))
        .bind(room)
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn block_today(&self, room: Uuid) {
        let mut tx = self.tx().await;
        let reason = rooms::NewBlockReason {
            code: "LEAK".into(),
            label: "Leak".into(),
            default_kind: rooms::BlockKind::OutOfService,
        };
        let reason =
            rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, reason).await.unwrap().id;
        let block = rooms::NewBlock {
            room_id: room,
            from: self.business_date,
            to: self.business_date + Days::days(2),
            kind: rooms::BlockKind::OutOfService,
            reason_id: reason,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn deactivate(&self, room: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query("update room set active = false where id = $1").bind(room).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// A second property of the tenant.
    async fn other_property(&self) -> Uuid {
        let mut tx = self.tx().await;
        let hotel = property::NewProperty {
            code: "KND".into(),
            name: "Kandy".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let id = property::create_property(&mut tx, self.tenant, self.user, hotel).await.unwrap().id;
        tx.commit().await.unwrap();
        id
    }

    async fn revoke_roles(&self, user: UserId) {
        let mut tx = self.tx().await;
        sqlx::query("delete from role_grant where user_id = $1").bind(user.0).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn audit_actions(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn assignments_are_set_replaced_and_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102", "103"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let bob = hotel.member(Some("hk_supervisor"), None).await;

    hotel.set(&[(rooms[0], Some(ann)), (rooms[1], Some(ann)), (rooms[2], Some(bob))]).await.unwrap();
    assert_eq!(hotel.assignee(rooms[0]).await, Some(ann.0));
    assert_eq!(hotel.assignee(rooms[2]).await, Some(bob.0));

    hotel.set(&[(rooms[0], Some(bob)), (rooms[1], None)]).await.unwrap();
    assert_eq!(hotel.assignee(rooms[0]).await, Some(bob.0));
    assert_eq!(hotel.assignee(rooms[1]).await, None);
    assert_eq!(hotel.assignee(rooms[2]).await, Some(bob.0));
    assert_eq!(hotel.audit_actions("room.assigned").await, 4);
    assert_eq!(hotel.audit_actions("room.unassigned").await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_assignee_without_the_role_is_invalid_and_nothing_is_written(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let no_role = hotel.member(None, None).await;
    let other_property = hotel.member(Some("housekeeping"), Some(hotel.other_property().await)).await;
    let front_desk = hotel.member(Some("front_desk"), Some(hotel.property)).await;

    for outsider in [no_role, other_property, front_desk] {
        let result = hotel.set(&[(rooms[0], Some(ann)), (rooms[1], Some(outsider))]).await;
        assert!(matches!(result, Err(HousekeepingError::Invalid(_))), "{result:?}");
    }

    assert_eq!(hotel.assignee(rooms[0]).await, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_must_be_active_rooms_of_the_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let ann = hotel.member(Some("housekeeping"), None).await;
    hotel.deactivate(rooms[1]).await;

    let inactive = hotel.set(&[(rooms[0], Some(ann)), (rooms[1], Some(ann))]).await;
    let unknown = hotel.set(&[(Uuid::now_v7(), Some(ann))]).await;
    let none = hotel.set(&[]).await;
    let twice = hotel.set(&[(rooms[0], Some(ann)), (rooms[0], None)]).await;

    assert!(matches!(inactive, Err(HousekeepingError::NotFound("room"))), "{inactive:?}");
    assert!(matches!(unknown, Err(HousekeepingError::NotFound("room"))), "{unknown:?}");
    assert!(matches!(none, Err(HousekeepingError::Invalid(_))), "{none:?}");
    assert!(matches!(twice, Err(HousekeepingError::Invalid(_))), "{twice:?}");
    assert_eq!(hotel.assignee(rooms[0]).await, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn copy_yesterday_copies_and_counts(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let bob = hotel.member(Some("housekeeping"), None).await;
    hotel.assign_yesterday(rooms[0], ann).await;
    hotel.assign_yesterday(rooms[1], bob).await;

    assert_eq!(hotel.copy().await, 2);
    assert_eq!(hotel.assignee(rooms[0]).await, Some(ann.0));
    assert_eq!(hotel.assignee(rooms[1]).await, Some(bob.0));
    assert_eq!(hotel.audit_actions("hk.assignments_copied").await, 1);
    assert_eq!(hotel.copy().await, 0, "a second copy finds everything already assigned");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn copy_yesterday_skips_inactive_blocked_assigned_and_role_less(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102", "103", "104", "105"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let bob = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let carl = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    for room in &rooms[..3] {
        hotel.assign_yesterday(*room, ann).await;
    }
    hotel.assign_yesterday(rooms[3], carl).await;
    hotel.assign_yesterday(rooms[4], ann).await;
    hotel.deactivate(rooms[0]).await;
    hotel.block_today(rooms[1]).await;
    hotel.assign(rooms[2], bob).await;
    hotel.revoke_roles(carl).await;

    assert_eq!(hotel.copy().await, 1);

    assert_eq!(hotel.assignee(rooms[0]).await, None, "inactive");
    assert_eq!(hotel.assignee(rooms[1]).await, None, "blocked");
    assert_eq!(hotel.assignee(rooms[2]).await, Some(bob.0), "already assigned today");
    assert_eq!(hotel.assignee(rooms[3]).await, None, "lost the role");
    assert_eq!(hotel.assignee(rooms[4]).await, Some(ann.0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeepers_lists_only_role_holders(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let here = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let everywhere = hotel.member(Some("hk_supervisor"), None).await;
    hotel.member(Some("housekeeping"), Some(hotel.other_property().await)).await;
    hotel.member(Some("front_desk"), Some(hotel.property)).await;
    hotel.member(None, None).await;

    let mut tx = hotel.tx().await;
    let mut listed: Vec<Uuid> =
        housekeepers(&mut tx, hotel.property).await.unwrap().into_iter().map(|h| h.user_id).collect();
    listed.sort_unstable();

    let mut expected = vec![here.0, everywhere.0];
    expected.sort_unstable();
    assert_eq!(listed, expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn setting_and_copying_emit_the_housekeeping_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let ann = hotel.member(Some("housekeeping"), None).await;
    hotel.assign_yesterday(rooms[1], ann).await;
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(CHANNEL).await.unwrap();

    hotel.set(&[(rooms[0], Some(ann))]).await.unwrap();
    let set = next_keys(&mut listener).await;
    hotel.copy().await;
    let copy = next_keys(&mut listener).await;

    assert!(set.contains(&hk_key(hotel.property)));
    assert!(copy.contains(&hk_key(hotel.property)));
}

async fn next_keys(listener: &mut PgListener) -> Vec<String> {
    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
    serde_json::from_str::<Event>(received.payload()).unwrap().keys
}
