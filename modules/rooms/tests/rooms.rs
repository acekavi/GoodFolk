mod common;

use common::Hotel;
use rooms::{NewRoom, Room, RoomChanges, RoomRange, RoomTypeChanges, RoomsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

impl Hotel {
    async fn update_room(&self, room: &Room, changes: RoomChanges) -> Result<Room, RoomsError> {
        let mut tx = self.tx().await;
        let updated =
            rooms::update_room(&mut tx, self.tenant, self.user, self.property, room.id, room.version, changes).await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    /// `room_type`'s physical count on every day of the window, collapsed to the distinct values.
    async fn physical(&self, room_type: Uuid) -> Vec<i32> {
        let mut values: Vec<i32> = self.counters(room_type).await.iter().map(|day| day.physical).collect();
        values.dedup();
        values
    }
}

fn range(room_type: Uuid, first: u32, last: u32) -> RoomRange {
    RoomRange { room_type_id: room_type, prefix: String::new(), first, last, floor: Some("1".into()), section_id: None }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_counts_on_every_day_of_the_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;

    let room = hotel.room(dlx.id, "101").await;

    assert_eq!((room.number.as_str(), room.active, room.version), ("101", true, 1));
    assert_eq!(hotel.physical(dlx.id).await, vec![1]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_range_creates_numbered_rooms_in_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let mut tx = hotel.tx().await;

    let created =
        rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 101, 105)).await.unwrap();
    tx.commit().await.unwrap();

    let numbers: Vec<&str> = created.iter().map(|room| room.number.as_str()).collect();
    assert_eq!(numbers, ["101", "102", "103", "104", "105"]);
    assert_eq!(created.iter().map(|room| room.sort_order).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
    assert!(created.iter().all(|room| room.floor.as_deref() == Some("1")));
    assert_eq!(hotel.physical(dlx.id).await, vec![5]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn existing_numbers_are_a_conflict_that_names_them(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    hotel.room(dlx.id, "103").await;
    let mut tx = hotel.tx().await;

    let result = rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 101, 105)).await;

    let Err(RoomsError::Conflict(message)) = result else { panic!("expected a conflict, got {result:?}") };
    assert_eq!(message, "room 103 already exists");
    assert_eq!(rooms::list_rooms(&mut tx, hotel.property, None).await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_range_must_be_in_order_and_at_most_200_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let mut tx = hotel.tx().await;

    let backwards =
        rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 120, 101)).await;
    let too_many = rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 1, 201)).await;

    assert!(matches!(backwards, Err(RoomsError::Invalid(_))), "{backwards:?}");
    assert!(matches!(too_many, Err(RoomsError::Invalid(_))), "{too_many:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn retyping_and_deactivating_move_the_counts(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(std.id, "101").await;

    let retyped =
        hotel.update_room(&room, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await.unwrap();
    let after_retype = (hotel.physical(std.id).await, hotel.physical(dlx.id).await);
    let deactivated =
        hotel.update_room(&retyped, RoomChanges { active: Some(false), ..RoomChanges::default() }).await.unwrap();
    let after_deactivate = hotel.physical(dlx.id).await;
    let reactivated =
        hotel.update_room(&deactivated, RoomChanges { active: Some(true), ..RoomChanges::default() }).await.unwrap();

    assert_eq!(after_retype, (vec![0], vec![1]));
    assert_eq!(after_deactivate, vec![0]);
    assert_eq!(hotel.physical(dlx.id).await, vec![1]);
    assert_eq!((reactivated.version, reactivated.room_type_id), (4, dlx.id));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_needs_an_active_room_type_of_its_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let std = hotel.room_type("STD").await;
    hotel.room(dlx.id, "101").await;
    let mut tx = hotel.tx().await;
    let retired = RoomTypeChanges { active: Some(false), ..RoomTypeChanges::default() };
    rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, std.id, 1, retired.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let unknown = NewRoom { room_type_id: Uuid::now_v7(), number: "102".into(), floor: None, section_id: None };
    let unknown = rooms::create_room(&mut tx, hotel.tenant, hotel.user, hotel.property, unknown).await;
    let inactive = NewRoom { room_type_id: std.id, number: "103".into(), floor: None, section_id: None };
    let inactive = rooms::create_room(&mut tx, hotel.tenant, hotel.user, hotel.property, inactive).await;
    let retire_in_use =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, retired).await;

    assert!(matches!(unknown, Err(RoomsError::Invalid(_))), "{unknown:?}");
    assert!(matches!(inactive, Err(RoomsError::Invalid(_))), "{inactive:?}");
    assert!(matches!(retire_in_use, Err(RoomsError::Conflict(_))), "{retire_in_use:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_added_while_its_type_is_being_retired_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    // Retire the empty type, but hold the transaction open while a room is added to it.
    let mut retiring = hotel.tx().await;
    let retired = RoomTypeChanges { active: Some(false), ..RoomTypeChanges::default() };
    rooms::update_room_type(&mut retiring, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, retired).await.unwrap();

    let commit_later = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        retiring.commit().await.unwrap();
    };
    let add = async {
        let mut tx = hotel.tx().await;
        let input = NewRoom { room_type_id: dlx.id, number: "101".into(), floor: None, section_id: None };
        let added = rooms::create_room(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await;
        if added.is_ok() {
            tx.commit().await.unwrap();
        }
        added
    };
    let ((), added) = tokio::join!(commit_later, add);

    assert!(matches!(added, Err(RoomsError::Invalid(_))), "{added:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_are_listed_in_display_order_and_filtered_by_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let first = hotel.room(std.id, "101").await;
    let second = hotel.room(dlx.id, "102").await;
    let third = hotel.room(std.id, "103").await;

    let mut tx = hotel.tx().await;
    rooms::reorder_rooms(&mut tx, hotel.tenant, hotel.property, &[third.id, first.id, second.id]).await.unwrap();
    let all = rooms::list_rooms(&mut tx, hotel.property, None).await.unwrap();
    let standard = rooms::list_rooms(&mut tx, hotel.property, Some(std.id)).await.unwrap();
    let partial = rooms::reorder_rooms(&mut tx, hotel.tenant, hotel.property, &[third.id]).await;

    assert_eq!(all.iter().map(|room| room.number.as_str()).collect::<Vec<_>>(), ["103", "101", "102"]);
    assert_eq!(standard.iter().map(|room| room.number.as_str()).collect::<Vec<_>>(), ["103", "101"]);
    assert!(matches!(partial, Err(RoomsError::Invalid(_))), "{partial:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let section = {
        let mut tx = hotel.tx().await;
        let section = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East").await.unwrap();
        tx.commit().await.unwrap();
        section
    };

    let moved = hotel
        .update_room(
            &room,
            RoomChanges {
                number: Some("101A".into()),
                floor: Some(Some("2".into())),
                section_id: Some(Some(section.id)),
                ..RoomChanges::default()
            },
        )
        .await
        .unwrap();
    let stale = hotel.update_room(&room, RoomChanges { floor: Some(None), ..RoomChanges::default() }).await;
    let cleared = hotel.update_room(&moved, RoomChanges { floor: Some(None), ..RoomChanges::default() }).await.unwrap();

    assert_eq!(
        (moved.number.as_str(), moved.floor.as_deref(), moved.section_id),
        ("101A", Some("2"), Some(section.id))
    );
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("room"))), "{stale:?}");
    assert_eq!((cleared.floor, cleared.section_id, cleared.version), (None, Some(section.id), 3));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_with_a_stay_still_to_come_keeps_its_type_and_stays_active(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let (room, other) = (hotel.room(std.id, "101").await, hotel.room(std.id, "102").await);
    let upcoming = hotel.assigned_stay(&room, 2, 4, "confirmed").await;
    // Stays that are over or were cancelled don't hold a room.
    hotel.assigned_stay(&other, -3, 0, "checked_out").await;
    hotel.assigned_stay(&other, 1, 3, "cancelled").await;

    let deactivated = hotel.update_room(&room, RoomChanges { active: Some(false), ..RoomChanges::default() }).await;
    let retyped = hotel.update_room(&room, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await;
    let moved = hotel.update_room(&room, RoomChanges { floor: Some(Some("2".into())), ..RoomChanges::default() }).await;
    let other = hotel.update_room(&other, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await;
    let other = hotel.update_room(&other.unwrap(), RoomChanges { active: Some(false), ..RoomChanges::default() }).await;

    let until = hotel.day(4);
    let Err(RoomsError::Conflict(deactivated)) = deactivated else { panic!("expected a conflict: {deactivated:?}") };
    assert_eq!(
        deactivated,
        format!("room 101 is assigned to {upcoming} until {until}; move that stay before deactivating the room")
    );
    let Err(RoomsError::Conflict(retyped)) = retyped else { panic!("expected a conflict: {retyped:?}") };
    assert_eq!(
        retyped,
        format!("room 101 is assigned to {upcoming} until {until}; move that stay before changing the room's type")
    );
    assert_eq!(moved.unwrap().floor.as_deref(), Some("2"), "other changes are fine");
    assert!(!other.unwrap().active);
}
