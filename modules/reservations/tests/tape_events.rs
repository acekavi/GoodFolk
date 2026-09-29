//! Every command that changes what the tape chart shows announces the months it touched
//! (`docs/superpowers/plans/2026-09-29-phase-4-tape-chart.md`'s Task 2). One test per command, checking the
//! `tape:` keys of the event it queues against `rooms::tape_keys` of the range the brief names.

mod common;

use common::{Hotel, Plans, new_guest};
use db::{CHANNEL, Event};
use reservations::{
    AssignedRoom, CancelledRoom, CheckInPolicy, CheckedIn, CheckedOut, CreatedReservation, Guest, ModifiedRoom,
    RoomChanges, UndoneCheckIn,
};
use rooms::Room;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::collections::BTreeSet;
use std::time::Duration;
use uuid::Uuid;

impl Hotel {
    /// A listener subscribed to the event channel, ready to receive whatever this hotel's commands queue next.
    async fn listener(&self) -> PgListener {
        let mut listener = PgListener::connect_with(&self.pool).await.unwrap();
        listener.listen(CHANNEL).await.unwrap();
        listener
    }

    async fn try_assign(&self, id: Uuid, version: i32, room: Uuid) -> AssignedRoom {
        let mut tx = self.tx().await;
        let assigned =
            reservations::assign_room(&mut tx, self.tenant, self.user, self.property, id, version, room).await.unwrap();
        tx.commit().await.unwrap();
        assigned
    }

    async fn try_unassign(&self, id: Uuid, version: i32) -> AssignedRoom {
        let mut tx = self.tx().await;
        let unassigned =
            reservations::unassign_room(&mut tx, self.tenant, self.user, self.property, id, version).await.unwrap();
        tx.commit().await.unwrap();
        unassigned
    }

    async fn try_cancel(&self, id: Uuid, version: i32) -> CancelledRoom {
        let mut tx = self.tx().await;
        let cancelled =
            reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, id, version).await.unwrap();
        tx.commit().await.unwrap();
        cancelled
    }

    async fn try_modify(&self, id: Uuid, version: i32, changes: RoomChanges) -> ModifiedRoom {
        let mut tx = self.tx().await;
        let modified = reservations::modify_room(&mut tx, self.tenant, self.user, self.property, id, version, changes)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        modified
    }

    async fn try_check_in(&self, id: Uuid, version: i32) -> CheckedIn {
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
        .await
        .unwrap();
        tx.commit().await.unwrap();
        checked_in
    }

    async fn try_undo_check_in(&self, id: Uuid, version: i32) -> UndoneCheckIn {
        let mut tx = self.tx().await;
        let undone =
            reservations::undo_check_in(&mut tx, self.tenant, self.user, self.property, id, version).await.unwrap();
        tx.commit().await.unwrap();
        undone
    }

    async fn try_check_out(&self, id: Uuid, version: i32) -> CheckedOut {
        let mut tx = self.tx().await;
        let checked_out =
            reservations::check_out(&mut tx, self.tenant, self.user, self.property, id, version).await.unwrap();
        tx.commit().await.unwrap();
        checked_out
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Books one DLX room on BAR for `[business date + from, business date + to)`.
    async fn stay(&self, booker: &Guest, plans: &Plans, from: i64, to: i64) -> CreatedReservation {
        self.try_book(booker, vec![self.room(self.deluxe.id, &plans.bar, from, to)]).await.unwrap()
    }

    /// Moves the business date to `business date + days` and extends the counter window to match, as the
    /// night audit would (see `tests/cancel.rs`'s `a_stay_already_under_way_releases_only_the_nights_left`).
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
}

/// Waits for the next event on `listener` and returns its keys.
async fn recv(listener: &mut PgListener) -> Vec<String> {
    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
    let event: Event = serde_json::from_str(received.payload()).unwrap();
    event.keys
}

/// Only the `tape:` keys of `keys`, as a set (duplicates would be a bug; order doesn't matter to the chart).
fn tape_only(keys: &[String]) -> BTreeSet<String> {
    keys.iter().filter(|key| key.starts_with("tape:")).cloned().collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn create_emits_tape_keys_for_every_booked_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let mut listener = hotel.listener().await;

    hotel
        .try_book(
            &booker,
            vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3), hotel.room(hotel.standard.id, &plans.bar, 28, 33)],
        )
        .await
        .unwrap();

    let keys = recv(&mut listener).await;
    let expected: BTreeSet<String> = rooms::tape_keys(hotel.property, hotel.day(0), hotel.day(3))
        .into_iter()
        .chain(rooms::tape_keys(hotel.property, hotel.day(28), hotel.day(33)))
        .collect();
    assert_eq!(tape_only(&keys), expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn assign_and_unassign_emit_tape_keys_for_the_stays_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 0, 3).await;
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    let expected = rooms::tape_keys(hotel.property, hotel.day(0), hotel.day(3)).into_iter().collect::<BTreeSet<_>>();
    let mut listener = hotel.listener().await;

    hotel.try_assign(room, 1, target.id).await;
    let assigned_keys = recv(&mut listener).await;
    hotel.try_unassign(room, 2).await;
    let unassigned_keys = recv(&mut listener).await;

    assert_eq!(tape_only(&assigned_keys), expected);
    assert_eq!(tape_only(&unassigned_keys), expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancel_emits_tape_keys_for_the_whole_stay_not_only_the_released_nights(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 1, 4).await;
    // Two days later: the counters only give back [2, 4), but the tape key must still cover [1, 4).
    hotel.move_business_date(2).await;
    let mut listener = hotel.listener().await;

    hotel.try_cancel(booked.rooms[0].id, 1).await;

    let keys = recv(&mut listener).await;
    let expected = rooms::tape_keys(hotel.property, hotel.day(1), hotel.day(4)).into_iter().collect::<BTreeSet<_>>();
    assert_eq!(tape_only(&keys), expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn modify_emits_tape_keys_for_the_old_and_new_months(_: PgPoolOptions, opts: PgConnectOptions) {
    // A stay 2..5 moved to 30..33 (crosses a month boundary for any business date) emits both ranges' months.
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 2, 5).await;
    let mut listener = hotel.listener().await;

    let changes =
        RoomChanges { check_in: Some(hotel.day(30)), check_out: Some(hotel.day(33)), ..RoomChanges::default() };
    hotel.try_modify(booked.rooms[0].id, 1, changes).await;

    let keys = recv(&mut listener).await;
    let expected: BTreeSet<String> = rooms::tape_keys(hotel.property, hotel.day(2), hotel.day(5))
        .into_iter()
        .chain(rooms::tape_keys(hotel.property, hotel.day(30), hotel.day(33)))
        .collect();
    assert_eq!(tape_only(&keys), expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_in_and_undo_emit_tape_keys_for_the_stays_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 0, 3).await;
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await;
    let expected = rooms::tape_keys(hotel.property, hotel.day(0), hotel.day(3)).into_iter().collect::<BTreeSet<_>>();
    let mut listener = hotel.listener().await;

    hotel.try_check_in(room, 2).await;
    let checked_in_keys = recv(&mut listener).await;
    hotel.try_undo_check_in(room, 3).await;
    let undone_keys = recv(&mut listener).await;

    assert_eq!(tape_only(&checked_in_keys), expected);
    assert_eq!(tape_only(&undone_keys), expected);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_emits_tape_keys_for_the_original_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 0, 5).await;
    let room = booked.rooms[0].id;
    let target = hotel.numbered("101").await;
    hotel.try_assign(room, 1, target.id).await;
    hotel.try_check_in(room, 2).await;
    // An early departure shortens the stay to [0, 2); the tape key must still cover the booked [0, 5).
    hotel.move_business_date(2).await;
    let mut listener = hotel.listener().await;

    hotel.try_check_out(room, 3).await;

    let keys = recv(&mut listener).await;
    let expected = rooms::tape_keys(hotel.property, hotel.day(0), hotel.day(5)).into_iter().collect::<BTreeSet<_>>();
    assert_eq!(tape_only(&keys), expected);
}
