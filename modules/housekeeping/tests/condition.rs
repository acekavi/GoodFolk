mod common;

use common::Hotel;
use db::{CHANNEL, Event};
use housekeeping::{Actor, Condition, HousekeepingError, hk_key, housekeeper_may, set_condition, set_conditions};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

const ALL: [Condition; 4] = [Condition::Dirty, Condition::Cleaning, Condition::Clean, Condition::Inspected];

impl Hotel {
    /// Sets `room`'s condition directly, as a supervisor, and returns the new version.
    async fn force(&self, room: Uuid, to: Condition) -> i32 {
        let (_, version) = self.condition(room).await;
        let mut tx = self.tx().await;
        let row = set_condition(&mut tx, self.tenant, Actor::Supervisor(self.user), self.property, room, version, to)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        row.version
    }

    async fn try_set(
        &self,
        actor: Actor,
        room: Uuid,
        version: i32,
        to: Condition,
    ) -> Result<housekeeping::RoomConditionRow, HousekeepingError> {
        let mut tx = self.tx().await;
        let row = set_condition(&mut tx, self.tenant, actor, self.property, room, version, to).await?;
        tx.commit().await.unwrap();
        Ok(row)
    }
}

#[test]
fn a_housekeeper_may_only_step_dirty_to_cleaning_to_clean() {
    for from in ALL {
        for to in ALL {
            let allowed =
                matches!((from, to), (Condition::Dirty, Condition::Cleaning) | (Condition::Cleaning, Condition::Clean));
            assert_eq!(housekeeper_may(from, to), allowed, "{from:?} to {to:?}");
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_housekeeper_steps_an_assigned_room_forward(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let maid = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    hotel.assign(room, maid).await;
    let version = hotel.force(room, Condition::Dirty).await;

    let row = hotel.try_set(Actor::Housekeeper(maid), room, version, Condition::Cleaning).await.unwrap();

    assert_eq!((row.condition, row.version), (Condition::Cleaning, version + 1));
    assert_eq!(hotel.condition(room).await, ("cleaning".into(), version + 1));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tenant_wide_grant_counts_as_a_housekeeping_role(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let lead = hotel.member(Some("hk_supervisor"), None).await;
    hotel.assign(room, lead).await;
    let version = hotel.force(room, Condition::Dirty).await;

    hotel.try_set(Actor::Housekeeper(lead), room, version, Condition::Cleaning).await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_housekeeper_on_an_unassigned_room_is_forbidden(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let maid = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let other = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    hotel.assign(rooms[1], other).await;
    let version = hotel.force(rooms[0], Condition::Dirty).await;

    let unassigned = hotel.try_set(Actor::Housekeeper(maid), rooms[0], version, Condition::Cleaning).await;
    let theirs = hotel.try_set(Actor::Housekeeper(maid), rooms[1], 1, Condition::Cleaning).await;

    assert!(matches!(unassigned, Err(HousekeepingError::Forbidden(_))));
    assert!(matches!(theirs, Err(HousekeepingError::Forbidden(_))));
    assert_eq!(hotel.condition(rooms[0]).await.0, "dirty");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_housekeeper_skipping_a_step_or_inspecting_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let maid = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    hotel.assign(room, maid).await;
    let version = hotel.force(room, Condition::Dirty).await;

    for to in [Condition::Clean, Condition::Inspected, Condition::Dirty] {
        let result = hotel.try_set(Actor::Housekeeper(maid), room, version, to).await;
        assert!(matches!(result, Err(HousekeepingError::Invalid(_))), "{to:?}");
    }
    assert_eq!(hotel.condition(room).await, ("dirty".into(), version));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_without_a_housekeeping_role_is_forbidden(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let none = hotel.member(None, None).await;
    let front_desk = hotel.member(Some("front_desk"), Some(hotel.property)).await;
    for (room, user) in rooms.iter().zip([none, front_desk]) {
        let version = hotel.force(*room, Condition::Dirty).await;
        hotel.assign(*room, user).await;
        let result = hotel.try_set(Actor::Housekeeper(user), *room, version, Condition::Cleaning).await;
        assert!(matches!(result, Err(HousekeepingError::Forbidden(_))));
        assert_eq!(hotel.condition(*room).await, ("dirty".into(), version));
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_supervisor_sets_any_value_but_not_the_same_one(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let mut version = 1;
    for to in [Condition::Inspected, Condition::Dirty, Condition::Clean, Condition::Cleaning] {
        let row = hotel.try_set(Actor::Supervisor(hotel.user), room, version, to).await.unwrap();
        assert_eq!((row.condition, row.version), (to, version + 1));
        version = row.version;
    }

    let same = hotel.try_set(Actor::Supervisor(hotel.user), room, version, Condition::Cleaning).await;

    assert!(matches!(same, Err(HousekeepingError::Invalid(_))));
    assert_eq!(hotel.audit_count().await, 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];

    let result = hotel.try_set(Actor::Supervisor(hotel.user), room, 7, Condition::Dirty).await;

    assert!(matches!(result, Err(HousekeepingError::VersionMismatch(_))));
    assert_eq!(hotel.condition(room).await, ("clean".into(), 1));
    assert_eq!(hotel.audit_count().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let mut tx = hotel.tx().await;

    let result =
        set_condition(&mut tx, hotel.tenant, Actor::Supervisor(hotel.user), Uuid::now_v7(), room, 1, Condition::Dirty)
            .await;

    assert!(matches!(result, Err(HousekeepingError::NotFound(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_sets_every_room_and_audits_each(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102", "103"]).await;
    let mut tx = hotel.tx().await;

    let rows =
        set_conditions(&mut tx, hotel.tenant, hotel.user, hotel.property, &rooms, Condition::Dirty).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row.condition == Condition::Dirty && row.version == 2));
    for room in &rooms {
        assert_eq!(hotel.condition(*room).await, ("dirty".into(), 2));
    }
    assert_eq!(hotel.audit_count().await, 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_naming_another_propertys_room_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut rooms = hotel.rooms(&["101", "102"]).await;
    rooms.push(Uuid::now_v7());
    let mut tx = hotel.tx().await;

    let result = set_conditions(&mut tx, hotel.tenant, hotel.user, hotel.property, &rooms, Condition::Dirty).await;
    drop(tx);

    assert!(matches!(result, Err(HousekeepingError::NotFound(_))));
    assert_eq!(hotel.condition(rooms[0]).await, ("clean".into(), 1));
    assert_eq!(hotel.audit_count().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_change_emits_the_housekeeping_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(CHANNEL).await.unwrap();

    hotel.try_set(Actor::Supervisor(hotel.user), rooms[0], 1, Condition::Dirty).await.unwrap();
    let single = next_keys(&mut listener).await;
    let mut tx = hotel.tx().await;
    set_conditions(&mut tx, hotel.tenant, hotel.user, hotel.property, &rooms, Condition::Cleaning).await.unwrap();
    tx.commit().await.unwrap();
    let bulk = next_keys(&mut listener).await;

    assert!(single.contains(&hk_key(hotel.property)));
    assert!(bulk.contains(&hk_key(hotel.property)));
}

async fn next_keys(listener: &mut PgListener) -> Vec<String> {
    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
    serde_json::from_str::<Event>(received.payload()).unwrap().keys
}
