//! The advisory deposit: the rule a booking keeps (`reservation_room.deposit_terms`) and what
//! `reservations::deposit_due` says it owes.

mod common;

use common::{Hotel, Plans, new_guest};
use rates::{DepositDue, DepositKind, DepositRule, RatePlan, RatePlanChanges};
use reservations::{CreatedReservation, DepositDueLine};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Gives `plan` the deposit `rule` (or none), committed.
    async fn set_deposit(&self, plan: &RatePlan, rule: Option<DepositRule>) -> RatePlan {
        let mut tx = self.tx().await;
        let changes = RatePlanChanges { deposit_rule: Some(rule), ..Default::default() };
        let updated =
            rates::update_rate_plan(&mut tx, self.tenant, self.user, self.property, plan.id, plan.version, changes)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        updated
    }

    async fn deposit_due(&self, reservation: Uuid) -> Vec<DepositDueLine> {
        reservations::deposit_due(&mut self.tx().await, reservation).await.unwrap()
    }

    /// The booked deposit terms of the reservation room `room`.
    async fn deposit_terms(&self, room: Uuid) -> Option<serde_json::Value> {
        sqlx::query_scalar("select deposit_terms from reservation_room where id = $1")
            .bind(room)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// Takes a cash payment of `amount` in `currency` on `reservation`'s deposit folio, committed; returns its id.
    async fn pay_deposit(&self, reservation: Uuid, amount: i64, currency: &str) -> Uuid {
        let mut tx = self.tx().await;
        let deposit = folio::ensure_deposit_folio(&mut tx, self.tenant, self.property, reservation).await.unwrap();
        let method = folio::Method::Cash;
        let payment = folio::take_payment(
            &mut tx,
            self.tenant,
            Some(self.user),
            self.property,
            deposit,
            method,
            amount,
            currency,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        payment.id
    }

    /// One room on `plan` for `[D + from, D + to)` for a new guest, committed.
    async fn book_on(&self, plan: &RatePlan, from: i64, to: i64) -> CreatedReservation {
        let booker = self.guest(new_guest("Ada", "Silva")).await;
        self.try_book(&booker, vec![self.room(self.deluxe.id, plan, from, to)]).await.unwrap()
    }
}

fn rule(kind: DepositKind, value: i64, due: DepositDue) -> DepositRule {
    DepositRule { kind, value, due }
}

fn line(currency: &str, amount: i64, due_date: time::Date, paid: i64) -> DepositDueLine {
    DepositDueLine { currency: currency.into(), amount, due_date, paid, outstanding: (amount - paid).max(0) }
}

/// RACK with a 1-night deposit due 7 days before arrival.
async fn deposit_plans(hotel: &Hotel, plans: &Plans) -> RatePlan {
    hotel.set_deposit(&plans.rack, Some(rule(DepositKind::Nights, 1, DepositDue::DaysBeforeArrival { days: 7 }))).await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_keeps_its_plan_s_rule_whatever_the_plan_says_later(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let rack = deposit_plans(&hotel, &plans).await;
    let booked = hotel.book_on(&rack, 10, 13).await;
    let terms =
        serde_json::json!({ "kind": "nights", "value": 1, "due": { "type": "days_before_arrival", "days": 7 } });
    assert_eq!(hotel.deposit_terms(booked.rooms[0].id).await, Some(terms.clone()));

    let rack = hotel.set_deposit(&rack, Some(rule(DepositKind::Percent, 10_000, DepositDue::AtBooking))).await;
    assert_eq!(hotel.deposit_terms(booked.rooms[0].id).await, Some(terms.clone()), "a rule edit leaves it alone");
    hotel.set_deposit(&rack, None).await;
    assert_eq!(hotel.deposit_terms(booked.rooms[0].id).await, Some(terms));
    assert_eq!(hotel.deposit_due(booked.id).await, vec![line("USD", 10_000, hotel.day(3), 0)]);

    let without = hotel.book_on(&plans.bar, 10, 13).await;
    assert_eq!(hotel.deposit_terms(without.rooms[0].id).await, None);
    assert!(hotel.deposit_due(without.id).await.is_empty(), "no rule, nothing due");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn each_kind_is_priced_from_the_room_s_nights_and_capped_at_its_total(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    // Three nights of 10000.
    let cases = [
        (rule(DepositKind::Percent, 2_500, DepositDue::AtBooking), 7_500),
        (rule(DepositKind::Nights, 2, DepositDue::AtBooking), 20_000),
        (rule(DepositKind::Nights, 30, DepositDue::AtBooking), 30_000),
        (rule(DepositKind::Amount, 12_345, DepositDue::AtBooking), 12_345),
        (rule(DepositKind::Amount, 1_000_000, DepositDue::AtBooking), 30_000),
    ];
    let mut rack = plans.rack.clone();
    for (n, (deposit, due)) in cases.into_iter().enumerate() {
        rack = hotel.set_deposit(&rack, Some(deposit.clone())).await;
        let from = i64::try_from(3 * n).unwrap();
        let booked = hotel.book_on(&rack, from, from + 3).await;
        assert_eq!(hotel.deposit_due(booked.id).await, vec![line("USD", due, hotel.day(0), 0)], "{deposit:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_due_date_is_the_booking_s_business_date_or_days_before_arrival(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let rack = hotel.set_deposit(&plans.rack, Some(rule(DepositKind::Nights, 1, DepositDue::AtBooking))).await;
    let at_booking = hotel.book_on(&rack, 20, 22).await;
    assert_eq!(hotel.deposit_due(at_booking.id).await[0].due_date, hotel.day(0));

    let rack =
        hotel.set_deposit(&rack, Some(rule(DepositKind::Nights, 1, DepositDue::DaysBeforeArrival { days: 5 }))).await;
    let before = hotel.book_on(&rack, 20, 22).await;
    assert_eq!(hotel.deposit_due(before.id).await[0].due_date, hotel.day(15));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_in_two_currencies_owe_per_currency_and_a_cancelled_room_owes_nothing(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let rack = deposit_plans(&hotel, &plans).await;
    let local = rates::NewRatePlan {
        deposit_rule: Some(rule(DepositKind::Percent, 5_000, DepositDue::AtBooking)),
        ..hotel.rate_plan("LOCAL", "LKR", &[hotel.deluxe.id])
    };
    let local = hotel.priced_plan(local, 0, 40, 2_000_000).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let rooms = vec![
        hotel.room(hotel.deluxe.id, &rack, 10, 12),
        hotel.room(hotel.deluxe.id, &local, 10, 12),
        hotel.room(hotel.deluxe.id, &rack, 12, 14),
    ];
    let booked = hotel.try_book(&booker, rooms).await.unwrap();

    assert_eq!(
        hotel.deposit_due(booked.id).await,
        vec![line("USD", 20_000, hotel.day(3), 0), line("LKR", 2_000_000, hotel.day(0), 0)]
    );

    let cancelled = &booked.rooms[2];
    let mut tx = hotel.tx().await;
    reservations::cancel_room(
        &mut tx,
        hotel.tenant,
        Some(hotel.user),
        hotel.property,
        cancelled.id,
        cancelled.version,
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        hotel.deposit_due(booked.id).await,
        vec![line("USD", 10_000, hotel.day(3), 0), line("LKR", 2_000_000, hotel.day(0), 0)]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_payment_then_a_refund_move_what_is_outstanding(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let rack = deposit_plans(&hotel, &plans).await;
    let booked = hotel.book_on(&rack, 10, 13).await;
    let due = |paid| vec![line("USD", 10_000, hotel.day(3), paid)];
    assert_eq!(hotel.deposit_due(booked.id).await[0].outstanding, 10_000);

    let payment = hotel.pay_deposit(booked.id, 6_000, "USD").await;
    hotel.pay_deposit(booked.id, 500_000, "LKR").await;
    assert_eq!(hotel.deposit_due(booked.id).await, due(6_000), "another currency's payment does not count");
    assert_eq!(hotel.deposit_due(booked.id).await[0].outstanding, 4_000);

    let mut tx = hotel.tx().await;
    folio::refund(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, payment, 2_500, "Overpaid", None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(hotel.deposit_due(booked.id).await, due(3_500));
    assert_eq!(hotel.deposit_due(booked.id).await[0].outstanding, 6_500);

    hotel.pay_deposit(booked.id, 9_000, "USD").await;
    assert_eq!(hotel.deposit_due(booked.id).await[0].outstanding, 0, "never below 0");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_outstanding_deposit_never_refuses_a_booking_or_a_check_in(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let rack = hotel.set_deposit(&plans.rack, Some(rule(DepositKind::Percent, 10_000, DepositDue::AtBooking))).await;
    let booked = hotel.book_on(&rack, 0, 2).await;
    hotel.pay_deposit(booked.id, 5_000, "USD").await;
    assert_eq!(hotel.deposit_due(booked.id).await, vec![line("USD", 20_000, hotel.day(0), 5_000)]);

    let stay = &booked.rooms[0];
    assert!(stay.room_id.is_some(), "auto-assigned");
    let mut tx = hotel.tx().await;
    reservations::check_in(&mut tx, hotel.tenant, hotel.user, hotel.property, stay.id, stay.version, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Check-in moves the deposit to the guest folio as transfers, which are not payments.
    assert_eq!(hotel.deposit_due(booked.id).await, vec![line("USD", 20_000, hotel.day(0), 5_000)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_reservation_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::for_booking(opts, 1).await;
    let missing = reservations::deposit_due(&mut hotel.tx().await, Uuid::now_v7()).await;
    assert!(matches!(missing, Err(reservations::ReservationsError::NotFound("reservation"))), "{missing:?}");
}
