mod common;

use common::{Hotel, Plans, new_guest};
use domain::RoomStatus;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use reservations::{
    AssignedRoom, CancelledRoom, CheckInPolicy, CheckedIn, CheckedOut, ModifiedRoom, ReservationsError, RoomChanges,
    UndoneCheckIn,
};
use rooms::{BlockKind, NewBlockReason, Room};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

impl Hotel {
    async fn try_check_in(&self, id: Uuid, version: i32) -> Result<CheckedIn, ReservationsError> {
        let mut tx = self.tx().await;
        let checked_in = reservations::check_in(
            &mut tx,
            self.tenant,
            self.user,
            self.property,
            id,
            version,
            CheckInPolicy::default(),
        )
        .await?;
        tx.commit().await.unwrap();
        Ok(checked_in)
    }

    async fn try_undo_check_in(&self, id: Uuid, version: i32) -> Result<UndoneCheckIn, ReservationsError> {
        let mut tx = self.tx().await;
        let undone = reservations::undo_check_in(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(undone)
    }

    async fn try_check_out(&self, id: Uuid, version: i32) -> Result<CheckedOut, ReservationsError> {
        let mut tx = self.tx().await;
        let checked_out = reservations::check_out(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(checked_out)
    }

    async fn try_assign(&self, id: Uuid, version: i32, room: Uuid) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let assigned =
            reservations::assign_room(&mut tx, self.tenant, self.user, self.property, id, version, room).await?;
        tx.commit().await.unwrap();
        Ok(assigned)
    }

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

    async fn try_cancel(&self, id: Uuid, version: i32) -> Result<CancelledRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let cancelled = reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(cancelled)
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// `room`'s stored columns that check-in, undo and check-out read or write.
    async fn room_row(&self, room: Uuid) -> RoomRow {
        sqlx::query_as(
            "select status, lower(stay) as check_in, upper(stay) as check_out, room_type_id, room_id, checked_in_at,
                    checked_in_business_date, checked_out_at, version
             from reservation_room where id = $1",
        )
        .bind(room)
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// `room`'s nights as `(date, room_amount, meal_amount)`, oldest first.
    async fn nights(&self, room: Uuid) -> Vec<(Date, i64, i64)> {
        sqlx::query_as(
            "select date, room_amount, meal_amount from reservation_night where reservation_room_id = $1 order by date",
        )
        .bind(room)
        .fetch_all(&mut *self.tx().await)
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

    /// Moves the business date to `business date + days` and extends the counter window to match, as the night
    /// audit would (see `tests/cancel.rs`'s `a_stay_already_under_way_releases_only_the_nights_left`).
    async fn move_business_date(&self, days: i64) {
        let mut tx = self.tx().await;
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.property)
            .bind(self.day(days))
            .execute(&mut *tx)
            .await
            .unwrap();
        rooms::extend_window(&mut tx, self.property).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Blocks `room` for `[business date + from, business date + to)` directly, bypassing
    /// `rooms::create_block`'s own guard against blocking a room a stay is assigned to. Check-in's own "not
    /// blocked today" check is defence in depth for exactly this state, which the room and block commands' own
    /// lock order keeps unreachable through their public functions -- this is the only way to reach it in a
    /// test.
    async fn raw_block(&self, room: Uuid, from: i64, to: i64) {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak =
                    NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        sqlx::query(
            "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id, note, created_by)
             values ($1, $2, $3, $4, daterange($5, $6), 'out_of_order', $7, '', $8)",
        )
        .bind(Uuid::now_v7())
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(room)
        .bind(self.day(from))
        .bind(self.day(to))
        .bind(reason.id)
        .bind(self.user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RoomRow {
    status: String,
    check_in: Date,
    check_out: Date,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    checked_in_at: Option<OffsetDateTime>,
    checked_in_business_date: Option<Date>,
    checked_out_at: Option<OffsetDateTime>,
    version: i32,
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checking_in_on_the_arrival_date_marks_the_room_and_bumps_versions(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();

    let checked_in = hotel.try_check_in(room, 3).await.unwrap();

    assert_eq!(checked_in.id, room);
    assert_eq!(checked_in.reservation_id, booked.id);
    assert_eq!(checked_in.status, RoomStatus::CheckedIn);
    assert_eq!(checked_in.version, 4);
    assert_eq!(checked_in.checked_in_business_date, hotel.day(0));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(3)));
    assert_eq!(row.room_id, Some(target.id));
    assert_eq!(row.checked_in_at, Some(checked_in.checked_in_at));
    assert_eq!(row.checked_in_business_date, Some(hotel.day(0)));
    assert!(row.checked_out_at.is_none());
    assert_eq!(row.version, 4);
    assert_eq!(hotel.reservation_version(booked.id).await, 4);
    let audited: Vec<Uuid> =
        sqlx::query_scalar("select entity_id from audit_log where action = 'reservation_room.checked_in'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(audited, vec![room]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_before_the_arrival_date_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();

    let refused = conflict(hotel.try_check_in(room, 3).await);

    assert_eq!(refused, format!("check-in is only on the arrival date ({})", hotel.day(2)));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_after_the_arrival_date_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 4)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.move_business_date(1).await;

    let refused = conflict(hotel.try_check_in(room, 3).await);

    assert_eq!(refused, format!("check-in is only on the arrival date ({})", hotel.day(0)));
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_without_an_assigned_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;

    let refused = conflict(hotel.try_check_in(room, 2).await);

    assert_eq!(refused, "assign a room first");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 2);
    assert_eq!(hotel.reservation_version(booked.id).await, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_on_a_blocked_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.raw_block(target.id, 0, 1).await;

    let refused = conflict(hotel.try_check_in(room, 3).await);

    assert_eq!(refused, format!("room 101 is blocked from {} to {}", hotel.day(0), hotel.day(1)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_with_a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();

    let stale = hotel.try_check_in(room, 2).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.version, 3);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn undo_check_in_on_the_same_day_reverts_to_confirmed(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();

    let undone = hotel.try_undo_check_in(room, 4).await.unwrap();

    assert_eq!(undone.status, RoomStatus::Confirmed);
    assert_eq!(undone.version, 5);
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "confirmed");
    assert_eq!(row.room_id, Some(target.id), "the room stays assigned");
    assert!(row.checked_in_at.is_none());
    assert!(row.checked_in_business_date.is_none());
    assert!(row.checked_out_at.is_none());
    assert_eq!(row.version, 5);
    assert_eq!(hotel.reservation_version(booked.id).await, 5);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn undo_check_in_after_the_business_date_moves_on_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 4)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();
    hotel.move_business_date(1).await;

    let refused = conflict(hotel.try_undo_check_in(room, 4).await);

    assert_eq!(refused, "check-in can only be undone on the day it happened");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!(row.version, 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn undo_check_in_with_a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();

    let stale = hotel.try_undo_check_in(room, 3).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!(row.version, 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_early_releases_the_right_nights_and_deletes_them(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();
    hotel.move_business_date(2).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 6).await;

    let checked_out = hotel.try_check_out(room, 4).await.unwrap();

    assert_eq!(checked_out.status, RoomStatus::CheckedOut);
    assert_eq!(checked_out.version, 5);
    assert_eq!(checked_out.released_nights, vec![hotel.day(2), hotel.day(3), hotel.day(4)]);
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_out");
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(2)));
    assert_eq!(row.checked_out_at, Some(checked_out.checked_out_at));
    assert_eq!(row.version, 5);
    assert_eq!(hotel.nights(room).await.len(), 2, "only day 0 and day 1 remain");
    let sold_after = hotel.sold(hotel.deluxe.id, 0, 6).await;
    assert_eq!(sold_after[2], sold_before[2] - 1);
    assert_eq!(sold_after[3], sold_before[3] - 1);
    assert_eq!(sold_after[4], sold_before[4] - 1);
    assert_eq!(sold_after[0], sold_before[0], "past nights stay sold");
    assert_eq!(sold_after[1], sold_before[1], "the night just past stays sold");
    assert_eq!(hotel.drift().await, vec![]);
    let audited: Vec<serde_json::Value> =
        sqlx::query_scalar("select data from audit_log where action = 'reservation_room.checked_out'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![serde_json::json!({
            "reservation_id": booked.id,
            "released_nights": [hotel.day(2), hotel.day(3), hotel.day(4)],
        })]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_on_the_arrival_day_keeps_one_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();

    let checked_out = hotel.try_check_out(room, 4).await.unwrap();

    assert_eq!(checked_out.released_nights, vec![hotel.day(1), hotel.day(2)]);
    let row = hotel.room_row(room).await;
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(1)));
    assert_eq!(hotel.nights(room).await.len(), 1, "the arrival night is kept");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_late_check_out_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 2)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();
    hotel.move_business_date(2).await;
    let nights_before = hotel.nights(room).await;
    let sold_before = hotel.sold(hotel.deluxe.id, 0, 3).await;

    let checked_out = hotel.try_check_out(room, 4).await.unwrap();

    assert_eq!(checked_out.released_nights, Vec::<Date>::new());
    let row = hotel.room_row(room).await;
    assert_eq!((row.check_in, row.check_out), (hotel.day(0), hotel.day(2)));
    assert_eq!(hotel.nights(room).await, nights_before);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 3).await, sold_before);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_with_a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();

    let stale = hotel.try_check_out(room, 3).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    let row = hotel.room_row(room).await;
    assert_eq!(row.status, "checked_in");
    assert_eq!(row.version, 4);
}

/// The Phase 3a carry-over this task resolves: `rooms::assigned_stay` used to see a checked-out room's stay as
/// reaching the business date even after an early departure, so the room could not be blocked or deactivated
/// until the whole original stay was over. It now stops at the shrunk `stay`.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn after_an_early_check_out_the_room_can_be_blocked_or_deactivated(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Booking auto-assigns a room; these stay tests assign by hand, so the stay starts unassigned and every
    // version below counts the unassign's bump.
    let booked = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 5)]).await.unwrap();
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 2, target.id).await.unwrap();
    hotel.try_check_in(room, 3).await.unwrap();
    hotel.move_business_date(2).await;
    hotel.try_check_out(room, 4).await.unwrap();

    let mut tx = hotel.tx().await;
    let reason = rooms::create_block_reason(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        NewBlockReason { code: "CLEAN".into(), label: "Clean".into(), default_kind: BlockKind::OutOfService },
    )
    .await
    .unwrap();
    let block = rooms::NewBlock {
        room_id: target.id,
        from: hotel.day(2),
        to: hotel.day(3),
        kind: BlockKind::OutOfService,
        reason_id: reason.id,
        note: String::new(),
    };
    rooms::create_block(&mut tx, hotel.tenant, hotel.user, hotel.property, block).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let changes = rooms::RoomChanges { active: Some(false), ..Default::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, target.id, target.version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_or_tenant_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap().rooms[0].id;
    let mut tx = hotel.tx().await;
    let other = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let other = property::create_property(&mut tx, hotel.tenant, hotel.user, other).await.unwrap();
    tx.commit().await.unwrap();
    let stranger = Hotel::new(opts).await;

    let mut tx = hotel.tx().await;
    let check_in_elsewhere =
        reservations::check_in(&mut tx, hotel.tenant, hotel.user, other.id, room, 1, CheckInPolicy::default()).await;
    let undo_elsewhere = reservations::undo_check_in(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let check_out_elsewhere = reservations::check_out(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let mut tx = stranger.tx().await;
    let check_in_other_tenant = reservations::check_in(
        &mut tx,
        stranger.tenant,
        stranger.user,
        stranger.property,
        room,
        1,
        CheckInPolicy::default(),
    )
    .await;

    assert!(
        matches!(check_in_elsewhere, Err(ReservationsError::NotFound("reservation room"))),
        "{check_in_elsewhere:?}"
    );
    assert!(matches!(undo_elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{undo_elsewhere:?}");
    assert!(
        matches!(check_out_elsewhere, Err(ReservationsError::NotFound("reservation room"))),
        "{check_out_elsewhere:?}"
    );
    assert!(
        matches!(check_in_other_tenant, Err(ReservationsError::NotFound("reservation room"))),
        "{check_in_other_tenant:?}"
    );
}

/// How a randomized step changes a booked room's stay, beyond a plain cancel.
#[derive(Debug, Clone)]
enum ModifyKind {
    /// Moves `check_out` by this many days (may land before `check_in`, or leave it unchanged: both refused).
    Dates(i64),
    /// Switches between DLX and STD, with or without keeping the booked price.
    Type { keep_price: bool },
}

#[derive(Debug, Clone)]
enum Op {
    /// Book one room per `(deluxe?, first night, nights)`, on BAR, room only.
    Book(Vec<(bool, i64, i64)>),
    /// Cancel the booked room at this index (modulo the rooms booked so far), whatever its status.
    Cancel(usize),
    /// Assigns the booked room at this index to a same-type physical room, picked by this index too.
    Assign(usize),
    Modify(usize, ModifyKind),
    CheckIn(usize),
    UndoCheckIn(usize),
    CheckOut(usize),
    /// Moves the business date forward by this many days (0 is a no-op step).
    Advance(u8),
}

fn op() -> impl Strategy<Value = Op> {
    let room = (any::<bool>(), 0..6_i64, 1..=3_i64);
    let modify_kind = prop_oneof![
        (-2..=3_i64).prop_map(ModifyKind::Dates),
        any::<bool>().prop_map(|keep_price| ModifyKind::Type { keep_price }),
    ];
    prop_oneof![
        3 => prop::collection::vec(room, 1..=2).prop_map(Op::Book),
        2 => any::<usize>().prop_map(Op::Cancel),
        2 => any::<usize>().prop_map(Op::Assign),
        2 => (any::<usize>(), modify_kind).prop_map(|(index, kind)| Op::Modify(index, kind)),
        2 => any::<usize>().prop_map(Op::CheckIn),
        1 => any::<usize>().prop_map(Op::UndoCheckIn),
        2 => any::<usize>().prop_map(Op::CheckOut),
        1 => (0..2_u8).prop_map(Op::Advance),
    ]
}

/// What a refused operation must leave unchanged: the counters and every booked room's row.
#[allow(clippy::type_complexity)]
async fn state(hotel: &Hotel) -> impl PartialEq + std::fmt::Debug {
    let counters = rooms::list_inventory(&mut hotel.tx().await, hotel.property, hotel.day(0), hotel.day(20)).await;
    let stays: Vec<(Uuid, String, Option<Uuid>, Date, Date, Option<Date>, i32)> = sqlx::query_as(
        "select id, status, room_id, lower(stay), upper(stay), checked_in_business_date, version
         from reservation_room order by id",
    )
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    (counters.unwrap(), stays, hotel.confirmation_numbers().await)
}

/// Random histories mixing every stay transition (Task 6's modify, and Task 7's check-in, undo and check-out)
/// with business-date moves: `find_drift` must stay empty and a refused step must change nothing, whatever
/// order they come in. `TestRunner::deterministic()` fixes the seed, so a failure reproduces without printing
/// one.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn find_drift_stays_empty_through_random_stays_and_business_date_moves(_: PgPoolOptions, opts: PgConnectOptions) {
    const HISTORIES: usize = 6;
    let mut runner = TestRunner::deterministic();

    for history in 0..HISTORIES {
        let ops = prop::collection::vec(op(), 1..=16).new_tree(&mut runner).unwrap().current();
        // Two DLX rooms and one STD room, so most operations are eventually refused for want of a room.
        let (hotel, Plans { bar, .. }) = Hotel::for_booking(opts.clone(), 2).await;
        let booker = hotel.guest(new_guest("Ada", "Silva")).await;
        let all_rooms = rooms::list_rooms(&mut hotel.tx().await, hotel.property, None).await.unwrap();
        let dlx_rooms: Vec<Room> =
            all_rooms.iter().filter(|room| room.room_type_id == hotel.deluxe.id).cloned().collect();
        let std_rooms: Vec<Room> =
            all_rooms.iter().filter(|room| room.room_type_id == hotel.standard.id).cloned().collect();
        // Every room booked so far, with its current version.
        let mut booked: Vec<(Uuid, i32)> = Vec::new();
        let mut numbers = 0;
        let mut business_offset = 0_i64;

        for (step, op) in ops.iter().enumerate() {
            let context = format!("history {history}, step {step} of {ops:?}");
            let before = state(&hotel).await;
            let refused = match op {
                Op::Book(stays) => {
                    let rooms = stays
                        .iter()
                        .map(|&(deluxe, from, nights)| {
                            let room_type = if deluxe { hotel.deluxe.id } else { hotel.standard.id };
                            hotel.room(room_type, &bar, from, from + nights)
                        })
                        .collect();
                    match hotel.try_book(&booker, rooms).await {
                        Ok(created) => {
                            numbers += 1;
                            booked.extend(created.rooms.iter().map(|room| (room.id, 1)));
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Cancel(_) if booked.is_empty() => continue,
                Op::Cancel(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_cancel(room, version).await {
                        Ok(cancelled) => {
                            booked[slot].1 = cancelled.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Assign(_) if booked.is_empty() => continue,
                Op::Assign(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    let row = hotel.room_row(room).await;
                    let pool = if row.room_type_id == hotel.deluxe.id { &dlx_rooms } else { &std_rooms };
                    let target = &pool[*index % pool.len()];
                    match hotel.try_assign(room, version, target.id).await {
                        Ok(assigned) => {
                            booked[slot].1 = assigned.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Modify(_, _) if booked.is_empty() => continue,
                Op::Modify(index, kind) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    let row = hotel.room_row(room).await;
                    let changes = match kind {
                        ModifyKind::Dates(delta) => RoomChanges {
                            check_out: Some(row.check_out + Duration::days(*delta)),
                            ..Default::default()
                        },
                        ModifyKind::Type { keep_price } => {
                            let new_type =
                                if row.room_type_id == hotel.deluxe.id { hotel.standard.id } else { hotel.deluxe.id };
                            RoomChanges { room_type_id: Some(new_type), keep_price: *keep_price, ..Default::default() }
                        }
                    };
                    match hotel.try_modify(room, version, changes).await {
                        Ok(modified) => {
                            booked[slot].1 = modified.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::CheckIn(_) if booked.is_empty() => continue,
                Op::CheckIn(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_check_in(room, version).await {
                        Ok(checked_in) => {
                            booked[slot].1 = checked_in.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::UndoCheckIn(_) if booked.is_empty() => continue,
                Op::UndoCheckIn(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_undo_check_in(room, version).await {
                        Ok(undone) => {
                            booked[slot].1 = undone.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::CheckOut(_) if booked.is_empty() => continue,
                Op::CheckOut(index) => {
                    let slot = index % booked.len();
                    let (room, version) = booked[slot];
                    match hotel.try_check_out(room, version).await {
                        Ok(checked_out) => {
                            booked[slot].1 = checked_out.version;
                            None
                        }
                        Err(err) => Some(err),
                    }
                }
                Op::Advance(days) => {
                    if *days > 0 {
                        business_offset += i64::from(*days);
                        hotel.move_business_date(business_offset).await;
                    }
                    None
                }
            };
            match refused {
                None => {}
                Some(
                    ReservationsError::Conflict(_)
                    | ReservationsError::Invalid(_)
                    | ReservationsError::VersionMismatch(_),
                ) => {
                    assert_eq!(state(&hotel).await, before, "a refused operation changed something: {context}");
                }
                Some(err) => panic!("unexpected {err:?}: {context}"),
            }
            assert_eq!(hotel.drift().await, vec![], "{context}");
            let expected: Vec<String> = (1..=numbers).map(|number| format!("GAL-{number:06}")).collect();
            assert_eq!(hotel.confirmation_numbers().await, expected, "{context}");
        }
    }
}
