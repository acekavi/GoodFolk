mod common;

use common::{Hotel, new_guest};
use reservations::{ModifiedRoom, ReservationsError, RoomChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;
use uuid::Uuid;

impl Hotel {
    /// Modifies room `id` at `version` in its own transaction, committed if it succeeds.
    async fn try_modify(
        &self,
        id: Uuid,
        version: i32,
        changes: RoomChanges,
    ) -> Result<ModifiedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let modified =
            reservations::modify_room(&mut tx, self.tenant, self.user, self.property, id, version, changes).await?;
        tx.commit().await.unwrap();
        Ok(modified)
    }

    /// `room`'s nights as `(date, room_amount, meal_amount)`, oldest first.
    async fn nights(&self, room: Uuid) -> Vec<(Date, i64, i64)> {
        sqlx::query_as(
            "select date, room_amount, meal_amount from reservation_night
             where reservation_room_id = $1 order by date",
        )
        .bind(room)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// `room`'s stored `(check_in, check_out, room_type_id, adults, children, room_id, version)`.
    #[allow(clippy::type_complexity)]
    async fn room_row(&self, room: Uuid) -> (Date, Date, Uuid, i32, i32, Option<Uuid>, i32) {
        sqlx::query_as(
            "select lower(stay), upper(stay), room_type_id, adults, children, room_id, version
             from reservation_room where id = $1",
        )
        .bind(room)
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> rooms::Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Blocks `room` for `[business date + from, business date + to)`, in its own transaction.
    async fn block(&self, room: &rooms::Room, from: i64, to: i64) {
        let mut tx = self.tx().await;
        let kind = rooms::BlockKind::OutOfOrder;
        let leak = rooms::NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: kind };
        let reason = rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap();
        let block = rooms::NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Sets `room`'s status to `checked_in` directly (the check-in command itself is a later task), with a
    /// business date matching the property's own, as the real command would leave it.
    async fn set_checked_in(&self, room: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query(
            "update reservation_room set status = 'checked_in', checked_in_at = now(),
                    checked_in_business_date = (select business_date from property where id = $2)
             where id = $1",
        )
        .bind(room)
        .bind(self.property)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_the_stay_quotes_only_the_added_nights(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 8).await;

    // The plan's price changes after booking; the nights already booked must not pick it up.
    let mut tx = hotel.tx().await;
    let repriced: Vec<rates::Price> = [5, 6]
        .into_iter()
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 2,
            amount: 20_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &repriced).await.unwrap();
    tx.commit().await.unwrap();

    let modified =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(7)), ..Default::default() }).await.unwrap();

    assert_eq!((modified.check_in, modified.check_out), (hotel.day(2), hotel.day(7)));
    assert_eq!(modified.version, 2);
    assert_eq!(modified.total, 3 * 10_000 + 2 * 20_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![
            (hotel.day(2), 10_000, 0),
            (hotel.day(3), 10_000, 0),
            (hotel.day(4), 10_000, 0),
            (hotel.day(5), 20_000, 0),
            (hotel.day(6), 20_000, 0),
        ],
        "kept nights keep their booked amount; only the added nights pick up the new price"
    );
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 8).await;
    assert_eq!(sold_after[5], sold_before[5] + 1);
    assert_eq!(sold_after[6], sold_before[6] + 1);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shortening_the_stay_releases_its_counters_and_deletes_its_nights(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 6)]).await.unwrap();
    let room = booked.rooms[0].id;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 8).await;

    let modified =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(4)), ..Default::default() }).await.unwrap();

    assert_eq!(modified.check_out, hotel.day(4));
    assert_eq!(modified.total, 2 * 10_000);
    assert_eq!(hotel.nights(room).await, vec![(hotel.day(2), 10_000, 0), (hotel.day(3), 10_000, 0)]);
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 8).await;
    assert_eq!(sold_after[4], sold_before[4] - 1);
    assert_eq!(sold_after[5], sold_before[5] - 1);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shifting_both_dates_moves_the_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let modified = hotel
        .try_modify(
            room,
            1,
            RoomChanges { check_in: Some(hotel.day(4)), check_out: Some(hotel.day(7)), ..Default::default() },
        )
        .await
        .unwrap();

    assert_eq!((modified.check_in, modified.check_out), (hotel.day(4), hotel.day(7)));
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(4), 10_000, 0), (hotel.day(5), 10_000, 0), (hotel.day(6), 10_000, 0)]
    );
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 8).await, [0, 0, 0, 0, 1, 1, 1, 0], "day 4 was already held, kept as is");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_without_keep_price_requotes_every_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room and a type change would re-assign one; this test is about an unassigned stay.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let std_price = rates::Price { room_type_id: hotel.standard.id, date: hotel.day(2), occupancy: 2, amount: 15_000 };
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &[std_price]).await.unwrap();
    tx.commit().await.unwrap();

    // Version 2: the unassign above bumped it.
    let modified = hotel
        .try_modify(room, 2, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert_eq!(modified.room_type_id, hotel.standard.id);
    assert!(!modified.unassigned, "the room was never assigned to begin with");
    let nights = hotel.nights(room).await;
    assert_eq!(nights[0], (hotel.day(2), 15_000, 0), "requoted on the new type's own price");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5], "released from DLX");
    assert_eq!(hotel.sold(hotel.standard.id, 2, 5).await, [1, 1, 1], "taken on STD");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_upgrade_with_keep_price_keeps_amounts_and_moves_the_counters(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room and a type change would re-assign one; this test is about an unassigned stay.
    let booked =
        hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.standard.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let before_nights = hotel.nights(room).await;
    let before_total = booked.rooms[0].total;

    // Version 2: the unassign above bumped it.
    let modified = hotel
        .try_modify(
            room,
            2,
            RoomChanges { room_type_id: Some(hotel.deluxe.id), keep_price: true, ..Default::default() },
        )
        .await
        .unwrap();

    assert_eq!(modified.room_type_id, hotel.deluxe.id);
    assert!(!modified.unassigned);
    assert_eq!(modified.total, before_total, "the upgrade keeps its original nightly prices");
    assert_eq!(hotel.nights(room).await, before_nights);
    assert_eq!(hotel.sold(hotel.standard.id, 2, 5).await, [0, 0, 0], "freed on STD");
    assert_eq!(hotel.sold(hotel.deluxe.id, 2, 5).await, [1, 1, 1], "taken on DLX");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reprice_requotes_every_night_even_without_a_type_or_occupancy_change(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let repriced: Vec<rates::Price> = (2..5)
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 2,
            amount: 25_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &repriced).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel.try_modify(room, 1, RoomChanges { reprice: true, ..Default::default() }).await.unwrap();

    assert_eq!(modified.total, 3 * 25_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(2), 25_000, 0), (hotel.day(3), 25_000, 0), (hotel.day(4), 25_000, 0)]
    );
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_occupancy_change_without_keep_price_requotes(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let single: Vec<rates::Price> = (2..5)
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 1,
            amount: 6_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &single).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel.try_modify(room, 1, RoomChanges { adults: Some(1), ..Default::default() }).await.unwrap();

    assert_eq!(modified.total, 3 * 6_000);
    assert_eq!(
        hotel.nights(room).await,
        vec![(hotel.day(2), 6_000, 0), (hotel.day(3), 6_000, 0), (hotel.day(4), 6_000, 0)]
    );
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_checked_in_room_may_only_change_its_check_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    hotel.set_checked_in(room).await;

    let blocked =
        hotel.try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() }).await;
    let wrong_field = conflict(blocked);
    let extended =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(7)), ..Default::default() }).await.unwrap();

    assert_eq!(wrong_field, "a checked-in room can only have its check-out date changed");
    assert_eq!(extended.check_out, hotel.day(7));
    assert_eq!(hotel.nights(room).await.len(), 7);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_sold_out_added_night_is_a_conflict_and_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    // Takes the property's only DLX room on the night this modify would add.
    hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 6)]).await.unwrap();
    let before_row = hotel.room_row(room).await;
    let before_nights = hotel.nights(room).await;
    let before_sold = hotel.sold(hotel.deluxe.id, 0, 8).await;

    let refused = hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert_eq!(conflict(refused), format!("no DLX rooms left on {}", hotel.day(5)));
    assert_eq!(hotel.room_row(room).await, before_row, "the stay and version are untouched");
    assert_eq!(hotel.nights(room).await, before_nights);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 8).await, before_sold);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_onto_another_bookings_assigned_room_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; this test puts both stays in room 101 by hand, so they start unassigned.
    let a = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)]).await.unwrap();
    let b = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 4, 7)]).await.unwrap();
    let r101 = hotel.numbered("101").await;
    let mut tx = hotel.tx().await;
    let (a_version, b_version) = (a.rooms[0].version, b.rooms[0].version);
    reservations::assign_room(&mut tx, hotel.tenant, hotel.user, hotel.property, a.rooms[0].id, a_version, r101.id)
        .await
        .unwrap();
    // Back-to-back stays may share a room.
    reservations::assign_room(&mut tx, hotel.tenant, hotel.user, hotel.property, b.rooms[0].id, b_version, r101.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Version 3: the unassign and the assignment each bumped it.
    let refused =
        hotel.try_modify(a.rooms[0].id, 3, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert_eq!(conflict(refused), format!("room 101 is taken by {} on those nights", b.confirmation_no));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_unassigns_a_room_of_the_old_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let r101 = hotel.numbered("101").await;
    assert_eq!(booked.rooms[0].room_id, Some(r101.id), "booking auto-assigned the only DLX room");
    // No STD room is free for those nights, so the type change cannot re-assign one: the overbooking allowance
    // lets the counters sell a second STD stay, which holds the only STD room.
    hotel.set_overbooking(&hotel.standard, 1).await;
    hotel.try_book(&booker, vec![hotel.room(hotel.standard.id, &plans.bar, 2, 5)]).await.unwrap();

    let modified = hotel
        .try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert!(modified.unassigned);
    assert_eq!(modified.room_id, None);
    assert_eq!(hotel.room_row(room).await.5, None);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_reassigns_a_room_of_the_new_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let r201 = hotel.numbered("201").await;

    let modified = hotel
        .try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert!(modified.unassigned, "the DLX room it had was dropped");
    assert_eq!(modified.room_id, Some(r201.id));
    assert_eq!(modified.room_number.as_deref(), Some("201"));
    assert_eq!(hotel.room_row(room).await.5, Some(r201.id));
    let audited: serde_json::Value =
        sqlx::query_scalar("select data from audit_log where entity_id = $1 and action = 'reservation_room.modified'")
            .bind(room)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!((audited["unassigned"].clone(), audited["room_id"].clone()), (true.into(), serde_json::json!(r201.id)));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_with_no_free_room_leaves_it_unassigned(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    // The only STD room is taken for the same nights; an overbooking allowance lets the counters still sell it.
    hotel.set_overbooking(&hotel.standard, 1).await;
    let taken = hotel.try_book(&booker, vec![hotel.room(hotel.standard.id, &plans.bar, 2, 5)]).await.unwrap();
    assert!(taken.rooms[0].room_id.is_some(), "the STD room is held by the other stay");

    let modified = hotel
        .try_modify(room, 1, RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() })
        .await
        .unwrap();

    assert!(modified.unassigned);
    assert_eq!(modified.room_id, None);
    assert_eq!(modified.room_number, None);
    assert_eq!(hotel.room_row(room).await.5, None);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_on_an_unassigned_stay_picks_a_room_of_the_new_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let r201 = hotel.numbered("201").await;

    let modified = hotel
        .try_modify(
            room,
            booked.rooms[0].version,
            RoomChanges { room_type_id: Some(hotel.standard.id), ..Default::default() },
        )
        .await
        .unwrap();

    assert!(!modified.unassigned, "there was no room to drop");
    assert_eq!(modified.room_id, Some(r201.id));
    assert_eq!(modified.room_number.as_deref(), Some("201"));
    assert_eq!(hotel.room_row(room).await.5, Some(r201.id));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    let stale = hotel.try_modify(room, 2, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_propertys_room_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
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
    let changes = RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() };
    let other =
        reservations::modify_room(&mut tx, hotel.tenant, hotel.user, kandy.id, booked.rooms[0].id, 1, changes).await;

    assert!(matches!(other, Err(ReservationsError::NotFound("reservation room"))), "{other:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_modifies_racing_for_the_last_room_on_one_night_let_one_through(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let a = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    let b = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 4, 6)]).await.unwrap();
    let night3 = hotel.day(3);
    let a_check_out = hotel.day(4);
    let b_check_in = hotel.day(3);

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Both transactions are open before either modifies, so the requests really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = tokio::task::JoinSet::new();
    let racers = [
        (a.rooms[0].id, a.rooms[0].version, RoomChanges { check_out: Some(a_check_out), ..Default::default() }),
        (b.rooms[0].id, b.rooms[0].version, RoomChanges { check_in: Some(b_check_in), ..Default::default() }),
    ];
    for (room, version, changes) in racers {
        let (pool, start) = (pool.clone(), start.clone());
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let modified = reservations::modify_room(&mut tx, tenant, user, property, room, version, changes).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(modified)
        });
    }
    let results = tasks.join_all().await;

    let winners: Vec<&ModifiedRoom> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    let conflicts: Vec<String> = results
        .iter()
        .filter_map(|result| match result {
            Err(ReservationsError::Conflict(message)) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    assert_eq!(conflicts, [format!("no DLX rooms left on {night3}")]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_modify_writes_an_audit_entry_naming_before_after_and_the_flags(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;

    hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(6)), ..Default::default() }).await.unwrap();

    let audited: Vec<(String, serde_json::Value)> =
        sqlx::query_as("select action, data from audit_log where entity_id = $1 order by id")
            .bind(room)
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![(
            "reservation_room.modified".into(),
            serde_json::json!({
                "before": {
                    "check_in": hotel.day(2), "check_out": hotel.day(5), "room_type": hotel.deluxe.id,
                    "adults": 2, "children": 0,
                },
                "after": {
                    "check_in": hotel.day(2), "check_out": hotel.day(6), "room_type": hotel.deluxe.id,
                    "adults": 2, "children": 0,
                },
                "keep_price": false,
                "reprice": false,
                "unassigned": false,
                "room_id": booked.rooms[0].room_id,
            })
        )]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shortening_a_checked_in_stays_check_out_releases_only_the_dropped_nights(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 6)]).await.unwrap();
    let room = booked.rooms[0].id;
    // The business date has since moved two nights on; those nights are now history for a checked-in stay.
    let mut tx = hotel.tx().await;
    sqlx::query("update property set business_date = $2 where id = $1")
        .bind(hotel.property)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.set_checked_in(room).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 6).await;

    let modified =
        hotel.try_modify(room, 1, RoomChanges { check_out: Some(hotel.day(4)), ..Default::default() }).await.unwrap();

    assert_eq!(modified.check_out, hotel.day(4));
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 6).await;
    assert_eq!(sold_after[0], sold_before[0], "night 0 is history, untouched");
    assert_eq!(sold_after[1], sold_before[1], "night 1 is history, untouched");
    assert_eq!(sold_after[2], sold_before[2], "night 2 is still held");
    assert_eq!(sold_after[3], sold_before[3], "night 3 is still held");
    assert_eq!(sold_after[4], sold_before[4] - 1, "night 4 was dropped and its counter released");
    assert_eq!(sold_after[5], sold_before[5] - 1, "night 5 was dropped and its counter released");
    assert_eq!(
        hotel.nights(room).await,
        vec![
            (hotel.day(0), 10_000, 0),
            (hotel.day(1), 10_000, 0),
            (hotel.day(2), 10_000, 0),
            (hotel.day(3), 10_000, 0),
        ]
    );
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_occupancy_change_with_keep_price_leaves_the_stored_amounts_unchanged(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let before_nights = hotel.nights(room).await;
    let mut tx = hotel.tx().await;
    let single: Vec<rates::Price> = (2..5)
        .map(|offset| rates::Price {
            room_type_id: hotel.deluxe.id,
            date: hotel.day(offset),
            occupancy: 1,
            amount: 6_000,
        })
        .collect();
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &single).await.unwrap();
    tx.commit().await.unwrap();

    let modified = hotel
        .try_modify(room, 1, RoomChanges { adults: Some(1), keep_price: true, ..Default::default() })
        .await
        .unwrap();

    assert_eq!(modified.total, 3 * 10_000, "keep_price keeps the booked total, not the new occupancy's own price");
    assert_eq!(
        hotel.nights(room).await,
        before_nights,
        "keep_price keeps the booked amounts despite the new occupancy's own price"
    );
    assert_eq!(hotel.drift().await, vec![]);
}

fn invalid<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_same_type_move_to_a_free_room_on_new_dates_works_while_the_old_room_is_taken_there(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let r102 = hotel.numbered("102").await;
    let moving = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    assert_eq!(moving.rooms[0].room_id, Some(r101.id), "booking auto-assigned 101");
    // 101 is taken on the nights the stay moves to; 102 is free.
    let other = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 8)]).await.unwrap();
    assert_eq!(other.rooms[0].room_id, Some(r101.id));

    let stay = moving.rooms[0].id;
    let refused = hotel
        .try_modify(
            stay,
            moving.rooms[0].version,
            RoomChanges { check_in: Some(hotel.day(5)), check_out: Some(hotel.day(7)), ..Default::default() },
        )
        .await;
    assert_eq!(conflict(refused), format!("room 101 is taken by {} on those nights", other.confirmation_no));

    let modified = hotel
        .try_modify(
            stay,
            moving.rooms[0].version,
            RoomChanges {
                check_in: Some(hotel.day(5)),
                check_out: Some(hotel.day(7)),
                room_id: Some(r102.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!((modified.check_in, modified.check_out), (hotel.day(5), hotel.day(7)));
    assert_eq!(
        (modified.room_id, modified.room_number.as_deref(), modified.unassigned),
        (Some(r102.id), Some("102"), false)
    );
    assert_eq!(hotel.room_row(stay).await.5, Some(r102.id));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_target_room_taken_on_the_new_dates_is_a_conflict_naming_the_holder_and_writes_nothing(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r102 = hotel.numbered("102").await;
    let moving = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    let holder = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 8)]).await.unwrap();
    let holder_stay = holder.rooms[0].id;
    // Move the holder to 102 so it is 102 that is taken on the new dates.
    hotel
        .try_modify(holder_stay, holder.rooms[0].version, RoomChanges { room_id: Some(r102.id), ..Default::default() })
        .await
        .unwrap();
    let before = hotel.room_row(moving.rooms[0].id).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 10).await;

    let refused = hotel
        .try_modify(
            moving.rooms[0].id,
            moving.rooms[0].version,
            RoomChanges {
                check_in: Some(hotel.day(5)),
                check_out: Some(hotel.day(7)),
                room_id: Some(r102.id),
                ..Default::default()
            },
        )
        .await;

    assert_eq!(conflict(refused), format!("room 102 is taken by {} on those nights", holder.confirmation_no));
    assert_eq!(hotel.room_row(moving.rooms[0].id).await, before);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 10).await, sold_before);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_target_room_of_another_type_than_the_new_type_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r102 = hotel.numbered("102").await;
    let r201 = hotel.numbered("201").await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    let (stay, version) = (booked.rooms[0].id, booked.rooms[0].version);

    let wrong_type = hotel
        .try_modify(
            stay,
            version,
            RoomChanges { check_out: Some(hotel.day(4)), room_id: Some(r201.id), ..Default::default() },
        )
        .await;
    assert_eq!(invalid(wrong_type), "room 201 is not a DLX");

    // The target must fit the new type, not the old one.
    let old_type = hotel
        .try_modify(
            stay,
            version,
            RoomChanges { room_type_id: Some(hotel.standard.id), room_id: Some(r102.id), ..Default::default() },
        )
        .await;
    assert_eq!(invalid(old_type), "room 102 is not a STD");

    let unknown = hotel
        .try_modify(
            stay,
            version,
            RoomChanges { check_out: Some(hotel.day(4)), room_id: Some(Uuid::now_v7()), ..Default::default() },
        )
        .await;
    assert_eq!(invalid(unknown), "no such room");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_type_change_with_a_target_room_puts_the_stay_in_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    hotel.rooms(hotel.standard.id, &["202"]).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r202 = hotel.numbered("202").await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();

    let modified = hotel
        .try_modify(
            booked.rooms[0].id,
            booked.rooms[0].version,
            RoomChanges { room_type_id: Some(hotel.standard.id), room_id: Some(r202.id), ..Default::default() },
        )
        .await
        .unwrap();

    assert_eq!((modified.room_id, modified.room_number.as_deref()), (Some(r202.id), Some("202")));
    assert!(!modified.unassigned, "the stay went straight to the named room");
    assert_eq!(hotel.room_row(booked.rooms[0].id).await.5, Some(r202.id));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_modifies_swapping_each_others_rooms_do_not_deadlock(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let r102 = hotel.numbered("102").await;
    let a = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    let b = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    assert_eq!((a.rooms[0].room_id, b.rooms[0].room_id), (Some(r101.id), Some(r102.id)));

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    let modify = |stay: Uuid, version: i32, changes: RoomChanges| {
        let pool = pool.clone();
        async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            let modified = reservations::modify_room(&mut tx, tenant, user, property, stay, version, changes).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(modified)
        }
    };
    let (mut a_state, mut b_state) = ((a.rooms[0].id, a.rooms[0].version), (b.rooms[0].id, b.rooms[0].version));
    let mut a_room = r101.id;
    for iteration in 0..20 {
        // Both stays leave their nights for a window nobody holds, each into the other's room, so the swap
        // never overlaps a stay and only the lock order is under test.
        let (from, to) = if iteration % 2 == 0 { (10, 12) } else { (1, 3) };
        let (a_target, b_target) = if a_room == r101.id { (r102.id, r101.id) } else { (r101.id, r102.id) };
        let changes = |room_id| RoomChanges {
            check_in: Some(hotel.day(from)),
            check_out: Some(hotel.day(to)),
            room_id: Some(room_id),
            ..Default::default()
        };
        let both = async {
            tokio::join!(
                modify(a_state.0, a_state.1, changes(a_target)),
                modify(b_state.0, b_state.1, changes(b_target))
            )
        };
        let (a_done, b_done) = tokio::time::timeout(std::time::Duration::from_secs(10), both)
            .await
            .unwrap_or_else(|_| panic!("deadlocked on iteration {iteration}"));
        let (a_done, b_done) = (a_done.unwrap(), b_done.unwrap());
        a_state = (a_done.id, a_done.version);
        b_state = (b_done.id, b_done.version);
        a_room = a_target;
        assert_eq!((a_done.room_id, b_done.room_id), (Some(a_target), Some(b_target)));
    }
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_with_the_current_room_named_is_refused_over_a_block_on_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    assert_eq!(booked.rooms[0].room_id, Some(r101.id));
    hotel.block(&r101, 4, 6).await;
    let before = hotel.room_row(booked.rooms[0].id).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 10).await;

    let refused = hotel
        .try_modify(
            booked.rooms[0].id,
            booked.rooms[0].version,
            RoomChanges { check_out: Some(hotel.day(5)), room_id: Some(r101.id), ..Default::default() },
        )
        .await;

    assert_eq!(conflict(refused), format!("room 101 is blocked from {} to {}", hotel.day(4), hotel.day(6)));
    assert_eq!(hotel.room_row(booked.rooms[0].id).await, before);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 10).await, sold_before);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocked_target_room_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r102 = hotel.numbered("102").await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    hotel.block(&r102, 5, 7).await;

    let refused = hotel
        .try_modify(
            booked.rooms[0].id,
            booked.rooms[0].version,
            RoomChanges {
                check_in: Some(hotel.day(5)),
                check_out: Some(hotel.day(7)),
                room_id: Some(r102.id),
                ..Default::default()
            },
        )
        .await;

    assert_eq!(conflict(refused), format!("room 102 is blocked from {} to {}", hotel.day(5), hotel.day(7)));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_checked_in_stay_is_never_moved_by_a_named_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let r102 = hotel.numbered("102").await;
    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    assert_eq!(booked.rooms[0].room_id, Some(r101.id));
    hotel.set_checked_in(room).await;

    let moved = hotel
        .try_modify(
            room,
            1,
            RoomChanges { check_out: Some(hotel.day(7)), room_id: Some(r102.id), ..Default::default() },
        )
        .await;
    assert_eq!(conflict(moved), "a checked-in room can only have its check-out date changed");
    assert_eq!(hotel.room_row(room).await.5, Some(r101.id));

    let extended = hotel
        .try_modify(
            room,
            1,
            RoomChanges { check_out: Some(hotel.day(7)), room_id: Some(r101.id), ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!((extended.check_out, extended.room_id), (hotel.day(7), Some(r101.id)));
    assert_eq!(hotel.drift().await, vec![]);
}
