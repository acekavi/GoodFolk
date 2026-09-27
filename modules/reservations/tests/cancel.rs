mod common;

use common::{Hotel, Plans, new_guest};
use domain::RoomStatus;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{CancellationPolicyChanges, CancellationRule, MealPlan, Penalty, PenaltyKind};
use reservations::{CancelledRoom, CreatedReservation, Guest, NewReservationRoom, ReservationsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Random booking histories tried per run. Each run draws new ones; a failure prints the history and step.
const HISTORIES: usize = 10;

impl Hotel {
    /// Cancels the reservation room `room` at `version` in its own transaction, committed if it succeeds.
    async fn try_cancel(&self, room: Uuid, version: i32) -> Result<CancelledRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let cancelled =
            reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, room, version).await?;
        tx.commit().await.unwrap();
        Ok(cancelled)
    }

    /// Books one room for `booker`.
    async fn book(&self, booker: &Guest, room: NewReservationRoom) -> CreatedReservation {
        self.try_book(booker, vec![room]).await.unwrap()
    }

    /// Every reservation room of the tenant as `(id, status, version, cancelled_by, cancellation_penalty)`.
    async fn stays(&self) -> Vec<(Uuid, String, i32, Option<Uuid>, Option<i64>)> {
        sqlx::query_as(
            "select id, status, version, cancelled_by, cancellation_penalty from reservation_room order by id",
        )
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
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_releases_the_nights_and_charges_what_the_booked_terms_say(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // Arriving tomorrow, within BAR's "1 night from 2 days out" rule. Breakfast adds 3000 a night.
    let breakfast = NewReservationRoom { meal_plan: MealPlan::Bb, ..hotel.room(hotel.deluxe.id, &plans.bar, 1, 4) };
    let booked = hotel.book(&booker, breakfast).await;
    let room = &booked.rooms[0];
    // The policy now charges the whole stay; the booking keeps the terms it was sold under.
    let mut tx = hotel.tx().await;
    let everything =
        CancellationRule { days_before_arrival: 30, penalty: Penalty { kind: PenaltyKind::Percent, value: 10_000 } };
    let changes = CancellationPolicyChanges { rules: Some(vec![everything]), ..CancellationPolicyChanges::default() };
    let policy = plans.bar.cancellation_policy_id.unwrap();
    rates::update_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy, 1, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let cancelled = hotel.try_cancel(room.id, 1).await.unwrap();

    assert_eq!(
        cancelled,
        CancelledRoom {
            id: room.id,
            reservation_id: booked.id,
            status: RoomStatus::Cancelled,
            version: 2,
            penalty: 10_000,
            currency: "USD".into(),
        },
        "the first night's room, without its breakfast"
    );
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.drift().await, vec![]);
    assert_eq!(hotel.stays().await, vec![(room.id, "cancelled".into(), 2, Some(hotel.user.0), Some(10_000))]);
    assert_eq!(hotel.reservation_version(booked.id).await, 2, "the reservation's status changed with it");
    let audited: Vec<(Uuid, serde_json::Value)> =
        sqlx::query_as("select entity_id, data from audit_log where action = 'reservation_room.cancelled'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![(room.id, serde_json::json!({ "reservation_id": booked.id, "penalty": 10_000, "currency": "USD" }))]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_one_room_leaves_the_other_booked(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel
        .try_book(
            &booker,
            vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4), hotel.room(hotel.deluxe.id, &plans.rack, 2, 5)],
        )
        .await
        .unwrap();
    let (kept, cancelled) = (&booked.rooms[0], &booked.rooms[1]);

    let cancelled = hotel.try_cancel(cancelled.id, 1).await.unwrap();

    assert_eq!(cancelled.penalty, 0, "RACK has no cancellation policy");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 6).await, [0, 1, 1, 1, 0, 0]);
    assert_eq!(hotel.drift().await, vec![]);
    let stays = hotel.stays().await;
    assert_eq!(stays[0], (kept.id, "confirmed".into(), 1, None, None));
    assert_eq!(stays[1], (cancelled.id, "cancelled".into(), 2, Some(hotel.user.0), Some(0)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_already_under_way_releases_only_the_nights_left(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;
    // Two days later, as the night audit will leave it: the counter window moved with the business date.
    let mut tx = hotel.tx().await;
    sqlx::query("update property set business_date = $2 where id = $1")
        .bind(hotel.property)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    rooms::extend_window(&mut tx, hotel.property).await.unwrap();
    tx.commit().await.unwrap();

    let cancelled = hotel.try_cancel(booked.rooms[0].id, 1).await.unwrap();

    assert_eq!(cancelled.penalty, 10_000, "past the arrival, the closest rule applies");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 1, 0, 0, 0], "the night already past stays sold");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_is_cancelled_only_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;
    let room = booked.rooms[0].id;
    hotel.try_cancel(room, 1).await.unwrap();
    let stays = hotel.stays().await;

    let again = conflict(hotel.try_cancel(room, 2).await);

    assert_eq!(again, "a cancelled room can't be cancelled");
    assert_eq!(hotel.stays().await, stays);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.reservation_version(booked.id).await, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await;

    let stale = hotel.try_cancel(booked.rooms[0].id, 2).await;

    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    assert_eq!(hotel.stays().await, vec![(booked.rooms[0].id, "confirmed".into(), 1, None, None)]);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 1, 1, 1, 0]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_of_another_property_or_tenant_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.book(&booker, hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)).await.rooms[0].id;
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
    let elsewhere = reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, other.id, room, 1).await;
    let mut tx = stranger.tx().await;
    let other_tenant =
        reservations::cancel_room(&mut tx, stranger.tenant, stranger.user, stranger.property, room, 1).await;

    assert!(matches!(elsewhere, Err(ReservationsError::NotFound("reservation room"))), "{elsewhere:?}");
    assert!(matches!(other_tenant, Err(ReservationsError::NotFound("reservation room"))), "{other_tenant:?}");
    assert_eq!(hotel.stays().await, vec![(room, "confirmed".into(), 1, None, None)]);
}

#[derive(Debug, Clone)]
enum Op {
    /// Book one room per `(deluxe?, first night, nights)`, on BAR, room only.
    Book(Vec<(bool, i64, i64)>),
    /// Cancel the booked room at this index (modulo the rooms booked so far), cancelled or not.
    Cancel(usize),
}

fn op() -> impl Strategy<Value = Op> {
    let room = (any::<bool>(), 0..10_i64, 1..=4_i64);
    prop_oneof![
        3 => prop::collection::vec(room, 1..=2).prop_map(Op::Book),
        2 => any::<usize>().prop_map(Op::Cancel),
    ]
}

/// What a refused operation must leave unchanged: the counters, the rooms and the confirmation numbers.
async fn state(hotel: &Hotel) -> impl PartialEq + std::fmt::Debug {
    let counters = rooms::list_inventory(&mut hotel.tx().await, hotel.property, hotel.day(0), hotel.day(20)).await;
    (counters.unwrap(), hotel.stays().await, hotel.confirmation_numbers().await)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_always_matches_the_rooms_booked_through_random_bookings_and_cancellations(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let mut runner = TestRunner::default();

    for history in 0..HISTORIES {
        let ops = prop::collection::vec(op(), 1..16).new_tree(&mut runner).unwrap().current();
        // Two DLX rooms and one STD room, so bookings are often refused for want of a room.
        let (hotel, Plans { bar, .. }) = Hotel::for_booking(opts.clone(), 2).await;
        let booker = hotel.guest(new_guest("Ada", "Silva")).await;
        // Every room booked so far, with its current version.
        let mut booked: Vec<(Uuid, i32)> = Vec::new();
        let mut numbers = 0;

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
            };
            match refused {
                None => {}
                Some(ReservationsError::Conflict(_) | ReservationsError::Invalid(_)) => {
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
