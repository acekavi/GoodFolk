mod common;

use common::Hotel;
use rates::{
    ChangeMode, MealPlan, NewMealSupplement, QuoteNight, QuoteRequest, RatesError, Residency, RestrictionChange,
    Segment, ViolationKind,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_quoted_from_the_stored_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let foreign = hotel.plan(rates::NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") }).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &foreign, ChangeMode::Percent, 1_500)).await;
    hotel.try_prices(&foreign, &hotel.prices(hotel.deluxe.id, 0, 3, 2, 10_000)).await.unwrap();
    hotel.try_prices(&foreign, &hotel.prices(hotel.standard.id, 0, 3, 2, 5_000)).await.unwrap();
    let mut tx = hotel.tx().await;
    for (currency, adult_amount) in [("USD", 1_500), ("LKR", 450_000)] {
        let breakfast = NewMealSupplement {
            meal_plan: MealPlan::Bb,
            currency: currency.into(),
            adult_amount,
            child_amount: 750,
            from: hotel.day(-10),
            to: None,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
    }
    tx.commit().await.unwrap();
    let departures = RestrictionChange { closed_to_departure: Some(true), ..hotel.restrict(&[hotel.deluxe.id], 3, 4) };
    hotel.try_restrict(&ota, departures).await.unwrap();
    let stay = |plan: &rates::RatePlan, residency: Residency| QuoteRequest {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plan.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(0),
        check_out: hotel.day(3),
        adults: 2,
        children: 1,
        residency,
    };

    let mut tx = hotel.tx().await;
    let quoted = rates::load_quote(&mut tx, hotel.property, &stay(&ota, Residency::NonResident)).await.unwrap();
    let resident = rates::load_quote(&mut tx, hotel.property, &stay(&foreign, Residency::Resident)).await.unwrap();
    let backwards = QuoteRequest { check_out: hotel.day(0), ..stay(&ota, Residency::NonResident) };
    let backwards = rates::load_quote(&mut tx, hotel.property, &backwards).await;
    let unknown = QuoteRequest { rate_plan_id: Uuid::now_v7(), ..stay(&ota, Residency::NonResident) };
    let unknown = rates::load_quote(&mut tx, hotel.property, &unknown).await;

    let night = |day: i64| QuoteNight { date: hotel.day(day), room: 11_500, meal: 3_750 };
    assert_eq!(quoted.nights, [night(0), night(1), night(2)], "the derived plan's resolved price, USD breakfast");
    assert_eq!((quoted.total, quoted.currency.as_str(), quoted.restrictions_ok), (45_750, "USD", false));
    let kinds: Vec<_> = quoted.violations.iter().map(|v| (v.kind, v.date)).collect();
    assert_eq!(kinds, [(ViolationKind::ClosedToDeparture, Some(hotel.day(3)))]);
    assert_eq!(resident.violations[0].kind, ViolationKind::Residency);
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "check-out is after check-in"), "{backwards:?}");
    assert!(matches!(unknown, Err(RatesError::NotFound("rate plan"))), "{unknown:?}");
}
