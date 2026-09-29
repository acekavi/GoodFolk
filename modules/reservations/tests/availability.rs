mod common;

use common::Hotel;
use rates::{MealPlan, NewRatePlan, Residency, Segment};
use reservations::{AvailabilityRequest, ReservationsError, RoomTypeAvailability};
use rooms::{BlockKind, NewBlock, NewBlockReason, Room};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Three deluxe rooms, two standard rooms, and plan `BAR` (USD, any guest) selling both types.
    async fn with_rooms(opts: PgConnectOptions) -> (Self, Vec<Room>) {
        let hotel = Hotel::new(opts).await;
        let deluxe = hotel.rooms(hotel.deluxe.id, &["101", "102", "103"]).await;
        hotel.rooms(hotel.standard.id, &["201", "202"]).await;
        hotel.priced_plan(hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id]), 0, 40, 10_000).await;
        (hotel, deluxe)
    }

    /// Two adults of `residency` for `[business date + from, business date + to)`.
    fn stay(&self, from: i64, to: i64, residency: Residency) -> AvailabilityRequest {
        AvailabilityRequest { check_in: self.day(from), check_out: self.day(to), adults: 2, children: 0, residency }
    }

    async fn try_availability(
        &self,
        request: &AvailabilityRequest,
    ) -> Result<Vec<RoomTypeAvailability>, ReservationsError> {
        reservations::availability(&mut self.tx().await, self.property, request).await
    }

    /// Free rooms per room type code, in display order.
    async fn free(&self, from: i64, to: i64) -> Vec<(String, i32)> {
        let found = self.try_availability(&self.stay(from, to, Residency::NonResident)).await.unwrap();
        found.into_iter().map(|room_type| (room_type.code, room_type.free)).collect()
    }

    /// Puts `room` out of order for `[business date + from, business date + to)`.
    async fn out_of_order(&self, room: &Room, from: i64, to: i64) {
        let mut tx = self.tx().await;
        let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
        let reason = rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap();
        let block = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind: BlockKind::OutOfOrder,
            reason_id: reason.id,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }
}

fn free(counts: &[(&str, i32)]) -> Vec<(String, i32)> {
    counts.iter().map(|(code, free)| (code.to_string(), *free)).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_order_room_is_not_free_on_the_nights_it_is_blocked(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, deluxe) = Hotel::with_rooms(opts).await;
    let before = hotel.free(0, 3).await;

    hotel.out_of_order(&deluxe[0], 1, 2).await;

    assert_eq!(before, free(&[("DLX", 3), ("STD", 2)]));
    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 2), ("STD", 2)]), "the fewest free over the nights");
    assert_eq!(hotel.free(2, 4).await, free(&[("DLX", 3), ("STD", 2)]), "the block is over by then");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_rooms_are_not_free(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update inventory_day set sold = 2 where room_type_id = $1 and date = $2")
        .bind(hotel.deluxe.id)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 1), ("STD", 2)]));
    assert_eq!(hotel.free(0, 2).await, free(&[("DLX", 3), ("STD", 2)]), "the departure date is not a night");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_overbooking_allowance_adds_to_what_is_free(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    // Every physical DLX room sold on the middle night: free would be 0 without the allowance.
    sqlx::query("update inventory_day set sold = 3 where room_type_id = $1 and date = $2")
        .bind(hotel.deluxe.id)
        .bind(hotel.day(2))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = hotel.tx().await;
    let changes = rooms::RoomTypeChanges { overbooking: Some(2), ..Default::default() };
    rooms::update_room_type(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        hotel.deluxe.id,
        hotel.deluxe.version,
        changes,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        hotel.free(0, 3).await,
        free(&[("DLX", 2), ("STD", 2)]),
        "3 of 3 physical rooms sold, plus a 2-room allowance"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_night_without_counters_has_nothing_free(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("delete from inventory_day where room_type_id = $1 and date = $2")
        .bind(hotel.standard.id)
        .bind(hotel.day(1))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(hotel.free(0, 3).await, free(&[("DLX", 3), ("STD", 0)]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_depend_on_the_guests_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let local = NewRatePlan { segment: Segment::FitL, ..hotel.rate_plan("FITL", "LKR", &[hotel.deluxe.id]) };
    hotel.priced_plan(local, 0, 40, 3_000_000).await;
    let offers = |found: &[RoomTypeAvailability]| -> Vec<(String, String, MealPlan)> {
        found
            .iter()
            .flat_map(|room_type| {
                room_type
                    .offers
                    .iter()
                    .map(|offer| (room_type.code.clone(), offer.rate_plan_code.clone(), offer.meal_plan))
            })
            .collect()
    };

    let residents = hotel.try_availability(&hotel.stay(0, 2, Residency::Resident)).await.unwrap();
    let foreigners = hotel.try_availability(&hotel.stay(0, 2, Residency::NonResident)).await.unwrap();

    let offer = |code: &str, plan: &str, meal_plan| (code.to_string(), plan.to_string(), meal_plan);
    assert_eq!(
        offers(&residents),
        [
            offer("DLX", "BAR", MealPlan::Ro),
            offer("DLX", "BAR", MealPlan::Bb),
            offer("DLX", "FITL", MealPlan::Ro),
            offer("DLX", "FITL", MealPlan::Bb),
            offer("STD", "BAR", MealPlan::Ro),
            offer("STD", "BAR", MealPlan::Bb),
        ]
    );
    assert_eq!(
        offers(&foreigners),
        [
            offer("DLX", "BAR", MealPlan::Ro),
            offer("DLX", "BAR", MealPlan::Bb),
            offer("STD", "BAR", MealPlan::Ro),
            offer("STD", "BAR", MealPlan::Bb),
        ]
    );
    let room_only = &residents[0].offers[2].quote;
    assert_eq!((room_only.total, room_only.currency.as_str()), (6_000_000, "LKR"));
    assert!(room_only.restrictions_ok, "{:?}", room_only.violations);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inactive_room_types_are_left_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let mut tx = hotel.tx().await;
    let (standard, retired) = (hotel.standard.id, rooms::RoomChanges { active: Some(false), ..Default::default() });
    for room in rooms::list_rooms(&mut tx, hotel.property, Some(standard)).await.unwrap() {
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, room.version, retired.clone())
            .await
            .unwrap();
    }
    let changes = rooms::RoomTypeChanges { active: Some(false), ..Default::default() };
    let version = hotel.standard.version;
    rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, standard, version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let found = hotel.try_availability(&hotel.stay(0, 2, Residency::NonResident)).await.unwrap();

    let codes: Vec<&str> = found.iter().map(|room_type| room_type.code.as_str()).collect();
    assert_eq!(codes, ["DLX"]);
    assert!(found[0].offers.iter().all(|offer| offer.room_type_id == hotel.deluxe.id));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stays_must_be_short_and_inside_the_booking_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::with_rooms(opts).await;
    let window = rooms::WINDOW_DAYS;
    let invalid = |result: Result<Vec<RoomTypeAvailability>, ReservationsError>| match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    };

    let month = hotel.try_availability(&hotel.stay(0, 30, Residency::Resident)).await;
    let last_night = hotel.try_availability(&hotel.stay(window - 1, window, Residency::Resident)).await;
    let too_long = hotel.try_availability(&hotel.stay(0, 31, Residency::Resident)).await;
    let backwards = hotel.try_availability(&hotel.stay(2, 2, Residency::Resident)).await;
    let past = hotel.try_availability(&hotel.stay(-1, 1, Residency::Resident)).await;
    let beyond = hotel.try_availability(&hotel.stay(window - 1, window + 1, Residency::Resident)).await;
    let elsewhere = hotel.stay(0, 1, Residency::Resident);
    let elsewhere = reservations::availability(&mut hotel.tx().await, Uuid::now_v7(), &elsewhere).await;

    assert_eq!(month.unwrap().len(), 2);
    assert_eq!(last_night.unwrap()[0].free, 3, "the last night of the counter window");
    assert_eq!(invalid(too_long), "an availability search covers at most 30 nights");
    assert_eq!(invalid(backwards), "check-out is after check-in");
    let (first, last) = (hotel.day(0), hotel.day(window));
    let outside = format!("stays must arrive on or after {first} and leave by {last}");
    assert_eq!(invalid(past), outside);
    assert_eq!(invalid(beyond), outside);
    assert!(matches!(elsewhere, Err(ReservationsError::NotFound("property"))));
}
