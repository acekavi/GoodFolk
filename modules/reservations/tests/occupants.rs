mod common;

use common::{Hotel, new_guest};
use reservations::{RemovedOccupant, ReservationsError, RoomOccupant};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Adds `guest` as an occupant of `room` at `version`, in its own transaction, committed if it succeeds.
    async fn try_add(&self, room: Uuid, version: i32, guest: Uuid) -> Result<RoomOccupant, ReservationsError> {
        let mut tx = self.tx().await;
        let added =
            reservations::add_occupant(&mut tx, self.tenant, self.user, self.property, room, version, guest).await?;
        tx.commit().await.unwrap();
        Ok(added)
    }

    /// Removes `guest` from `room`'s occupants at `version`, in its own transaction, committed if it succeeds.
    async fn try_remove(&self, room: Uuid, version: i32, guest: Uuid) -> Result<RemovedOccupant, ReservationsError> {
        let mut tx = self.tx().await;
        let removed =
            reservations::remove_occupant(&mut tx, self.tenant, self.user, self.property, room, version, guest).await?;
        tx.commit().await.unwrap();
        Ok(removed)
    }

    /// `room`'s occupant guest ids, in order.
    async fn occupant_guest_ids(&self, room: Uuid) -> Vec<Uuid> {
        sqlx::query_scalar("select guest_id from reservation_guest where reservation_room_id = $1 order by guest_id")
            .bind(room)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// `room`'s own current `(status, version)`.
    async fn room_status_version(&self, room: Uuid) -> (String, i32) {
        sqlx::query_as("select status, version from reservation_room where id = $1")
            .bind(room)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn reservation_version(&self, reservation: Uuid) -> i32 {
        sqlx::query_scalar("select version from reservation where id = $1")
            .bind(reservation)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The `data` of every `reservation_room` audit entry of `room` for `action`, oldest first.
    async fn audit_data(&self, room: Uuid, action: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar(
            "select data from audit_log
             where entity = 'reservation_room' and entity_id = $1 and action = $2
             order by at, id",
        )
        .bind(room)
        .bind(action)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }
}

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_occupant_is_added_then_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let added = hotel.try_add(room, 1, extra.id).await.unwrap();

    assert_eq!(added.room_id, room);
    assert_eq!(added.reservation_id, booked.id);
    assert_eq!(added.version, 2);
    assert_eq!(added.guest, extra);
    assert_eq!(hotel.occupant_guest_ids(room).await, [extra.id]);
    assert_eq!(hotel.room_status_version(room).await, ("confirmed".into(), 2));
    assert_eq!(hotel.reservation_version(booked.id).await, 2, "the reservation's own version also bumps");
    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_added").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Ben Perera" })]
    );

    let removed = hotel.try_remove(room, 2, extra.id).await.unwrap();

    assert_eq!(removed.room_id, room);
    assert_eq!(removed.reservation_id, booked.id);
    assert_eq!(removed.version, 3);
    assert_eq!(removed.guest_id, extra.id);
    assert!(hotel.occupant_guest_ids(room).await.is_empty());
    assert_eq!(hotel.room_status_version(room).await, ("confirmed".into(), 3));
    assert_eq!(hotel.reservation_version(booked.id).await, 3);
    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_removed").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Ben Perera" })]
    );

    // Once removed, removing again finds no such occupant -- the room's version has moved on to 3.
    let gone = hotel.try_remove(room, 3, extra.id).await;
    assert!(matches!(gone, Err(ReservationsError::NotFound("occupant"))), "{gone:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_with_a_single_name_is_added(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("", "Madonna")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    hotel.try_add(room, 1, extra.id).await.unwrap();

    assert_eq!(
        hotel.audit_data(room, "reservation_room.occupant_added").await,
        [serde_json::json!({ "guest_id": extra.id, "name": "Madonna" })]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_primary_guest_cannot_also_be_an_occupant(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = hotel.try_add(room, 1, booker.id).await;

    assert_eq!(invalid(refused), "Ada Silva is already the room's primary guest");
    assert!(hotel.occupant_guest_ids(room).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn adding_the_same_occupant_twice_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.try_add(room, 1, extra.id).await.unwrap();

    let refused = hotel.try_add(room, 2, extra.id).await;

    assert_eq!(conflict(refused), "Ben Perera is already an occupant of this room");
    assert_eq!(hotel.occupant_guest_ids(room).await, [extra.id], "the duplicate attempt changed nothing");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_cannot_hold_more_than_its_max_occupancy(_: PgPoolOptions, opts: PgConnectOptions) {
    // The test hotel's DLX room type holds at most 3 guests (see `Hotel::new`).
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let first = hotel.guest(new_guest("Ben", "Perera")).await;
    let second = hotel.guest(new_guest("Chandra", "Silva")).await;
    let third = hotel.guest(new_guest("Dilani", "Fernando")).await;

    hotel.try_add(room, 1, first.id).await.unwrap();
    hotel.try_add(room, 2, second.id).await.unwrap();
    let refused = hotel.try_add(room, 3, third.id).await;

    assert_eq!(invalid(refused), "a DLX room holds at most 3 guests");
    assert_eq!(hotel.occupant_guest_ids(room).await.len(), 2, "the room is left at its two occupants");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_confirmed_or_checked_in_room_takes_occupants(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room, 1).await.unwrap();
    tx.commit().await.unwrap();

    let add_refused = hotel.try_add(room, 2, extra.id).await;
    let remove_refused = hotel.try_remove(room, 2, extra.id).await;

    assert_eq!(
        conflict(add_refused),
        "only a confirmed or checked-in room can have an occupant added; this one is cancelled"
    );
    assert_eq!(
        conflict(remove_refused),
        "only a confirmed or checked-in room can have an occupant removed; this one is cancelled"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.try_add(room, 1, extra.id).await.unwrap();

    let stale_add = hotel.try_add(room, 1, extra.id).await;
    let stale_remove = hotel.try_remove(room, 1, extra.id).await;

    assert!(matches!(stale_add, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_add:?}");
    assert!(matches!(stale_remove, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_remove:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_cannot_be_added(_: PgPoolOptions, opts: PgConnectOptions) {
    let (ours, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let theirs = Hotel::new(opts).await;
    let their_guest = theirs.guest(new_guest("Eve", "Stranger")).await;
    let booker = ours.guest(new_guest("Ada", "Silva")).await;
    let booked = ours.try_book(&booker, vec![ours.room(ours.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = ours.try_add(room, 1, their_guest.id).await;
    assert_eq!(invalid(refused), "no such guest");
    assert!(ours.occupant_guest_ids(room).await.is_empty(), "another tenant's guest was never added");

    // A wholly unknown id is refused exactly the same way.
    let refused_unknown = ours.try_add(room, 1, Uuid::now_v7()).await;
    assert_eq!(invalid(refused_unknown), "no such guest");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let kandy = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let kandy = property::create_property(&mut tx, hotel.tenant, hotel.user, kandy).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let add_elsewhere =
        reservations::add_occupant(&mut tx, hotel.tenant, hotel.user, kandy.id, room, 1, extra.id).await;
    let remove_elsewhere =
        reservations::remove_occupant(&mut tx, hotel.tenant, hotel.user, kandy.id, room, 1, extra.id).await;

    assert!(matches!(add_elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{add_elsewhere:?}");
    assert!(matches!(remove_elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{remove_elsewhere:?}");
    assert!(hotel.occupant_guest_ids(room).await.is_empty(), "the wrong-property attempts wrote nothing");
    assert_eq!(hotel.room_status_version(room).await, ("confirmed".into(), 1), "the room's version is untouched");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn detail_lists_occupants_as_masked_guests(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let extra = hotel.guest(new_guest("Ben", "Perera")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let before = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(before.rooms[0].occupants.is_empty());

    hotel.try_add(room, 1, extra.id).await.unwrap();
    let with_occupant = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert_eq!(with_occupant.rooms[0].occupants.len(), 1);
    assert_eq!(with_occupant.rooms[0].occupants[0], extra);
    assert_eq!(with_occupant.rooms[0].occupants[0].id_doc_masked, None);

    hotel.try_remove(room, 2, extra.id).await.unwrap();
    let after = reservations::get_reservation(&mut hotel.tx().await, hotel.property, booked.id).await.unwrap();
    assert!(after.rooms[0].occupants.is_empty(), "removed occupants no longer show in the detail");
}
