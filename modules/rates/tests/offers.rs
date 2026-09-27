mod common;

use common::Hotel;
use rates::{
    ChangeMode, MealPlan, NewMealSupplement, NewRatePlan, OfferRequest, QuoteRequest, RatePlanChanges, RatesError,
    Residency, RestrictionChange, Segment, ViolationKind,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Plans `BAR` (USD, any guest, RO/BB/HB), `OTA` (derived from `BAR`, 10 % off, RO/BB), `FITF` (USD,
/// non-residents), `FITL` (LKR, residents) and a retired `OLD`, with prices, restrictions and supplements that
/// leave some offers unsellable.
async fn seed(hotel: &Hotel) {
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);
    let bar = NewRatePlan {
        extra_adult_amount: 1_000,
        allowed_meal_plans: vec![MealPlan::Hb, MealPlan::Ro, MealPlan::Bb],
        ..hotel.standard_plan("BAR", "USD")
    };
    let bar = hotel.plan(bar).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, -1_000)).await;
    let foreign = hotel.plan(NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") }).await;
    let local = hotel.plan(NewRatePlan { segment: Segment::FitL, ..hotel.standard_plan("FITL", "LKR") }).await;
    let old = hotel.plan(hotel.standard_plan("OLD", "USD")).await;
    hotel.try_update(&old, RatePlanChanges { active: Some(false), ..Default::default() }).await.unwrap();

    let bar_prices = [
        hotel.prices(deluxe, 0, 4, 2, 10_000),
        hotel.prices(deluxe, 0, 4, 1, 8_000),
        hotel.prices(standard, 0, 2, 1, 5_000),
    ];
    hotel.try_prices(&bar, &bar_prices.concat()).await.unwrap();
    hotel.try_prices(&foreign, &hotel.prices(deluxe, 0, 4, 2, 12_000)).await.unwrap();
    hotel.try_prices(&local, &hotel.prices(deluxe, 0, 3, 2, 3_000_000)).await.unwrap();
    hotel.try_prices(&local, &hotel.prices(standard, 0, 3, 1, 1_500_000)).await.unwrap();

    let min_stay = RestrictionChange { min_stay: Some(Some(4)), ..hotel.restrict(&[deluxe], 1, 2) };
    hotel.try_restrict(&ota, min_stay).await.unwrap();
    let no_arrivals = RestrictionChange { closed_to_arrival: Some(true), ..hotel.restrict(&[standard], 0, 1) };
    hotel.try_restrict(&local, no_arrivals).await.unwrap();
    let no_departures = RestrictionChange { closed_to_departure: Some(true), ..hotel.restrict(&[deluxe], 3, 4) };
    hotel.try_restrict(&foreign, no_departures).await.unwrap();

    let mut tx = hotel.tx().await;
    let supplements = [
        (MealPlan::Bb, "USD", 1_500, None),
        (MealPlan::Hb, "USD", 4_000, Some(hotel.day(2))),
        (MealPlan::Bb, "LKR", 450_000, None),
    ];
    for (meal_plan, currency, adult_amount, to) in supplements {
        let supplement = NewMealSupplement {
            meal_plan,
            currency: currency.into(),
            adult_amount,
            child_amount: adult_amount / 2,
            from: hotel.day(-10),
            to,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, supplement).await.unwrap();
    }
    tx.commit().await.unwrap();
}

fn request(hotel: &Hotel, residency: Residency, adults: i32, children: i32) -> OfferRequest {
    OfferRequest { check_in: hotel.day(0), check_out: hotel.day(3), adults, children, residency, room_type_ids: None }
}

/// `(room type code, plan code, meal plan)` of each offer, in order.
fn combinations(hotel: &Hotel, offers: &[rates::Offer]) -> Vec<(String, String, MealPlan)> {
    let code = |id: Uuid| if id == hotel.deluxe.id { &hotel.deluxe.code } else { &hotel.standard.code };
    offers
        .iter()
        .map(|offer| (code(offer.room_type_id).clone(), offer.rate_plan_code.clone(), offer.meal_plan))
        .collect()
}

fn expected(rows: &[(&str, &str, &[MealPlan])]) -> Vec<(String, String, MealPlan)> {
    rows.iter()
        .flat_map(|(room_type, plan, meal_plans)| {
            meal_plans.iter().map(|meal_plan| (room_type.to_string(), plan.to_string(), *meal_plan))
        })
        .collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_offer_is_quoted_exactly_as_load_quote_quotes_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;

    for residency in [Residency::Resident, Residency::NonResident] {
        for (adults, children) in [(2, 1), (1, 0)] {
            let request = request(&hotel, residency, adults, children);
            let offers = rates::load_offers(&mut tx, hotel.property, &request).await.unwrap();

            assert_eq!(offers.len(), 14, "{residency:?} {adults}+{children}");
            for offer in &offers {
                let single = QuoteRequest {
                    room_type_id: offer.room_type_id,
                    rate_plan_id: offer.rate_plan_id,
                    meal_plan: offer.meal_plan,
                    check_in: request.check_in,
                    check_out: request.check_out,
                    adults,
                    children,
                    residency,
                };
                let quoted = rates::load_quote(&mut tx, hotel.property, &single).await.unwrap();
                assert_eq!(
                    offer.quote, quoted,
                    "{} {:?} {residency:?} {adults}+{children}",
                    offer.rate_plan_code, offer.meal_plan
                );
            }
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_are_the_active_plans_for_the_residency_in_display_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;
    const ALL: &[MealPlan] = &[MealPlan::Ro, MealPlan::Bb, MealPlan::Hb];
    const RO_BB: &[MealPlan] = &[MealPlan::Ro, MealPlan::Bb];

    let (residents, foreigners) =
        (request(&hotel, Residency::Resident, 2, 0), request(&hotel, Residency::NonResident, 2, 0));

    let residents = rates::load_offers(&mut tx, hotel.property, &residents).await.unwrap();
    let foreigners = rates::load_offers(&mut tx, hotel.property, &foreigners).await.unwrap();

    assert_eq!(
        combinations(&hotel, &residents),
        expected(&[
            ("DLX", "BAR", ALL),
            ("DLX", "FITL", RO_BB),
            ("DLX", "OTA", RO_BB),
            ("STD", "BAR", ALL),
            ("STD", "FITL", RO_BB),
            ("STD", "OTA", RO_BB),
        ])
    );
    assert_eq!(
        combinations(&hotel, &foreigners),
        expected(&[
            ("DLX", "BAR", ALL),
            ("DLX", "FITF", RO_BB),
            ("DLX", "OTA", RO_BB),
            ("STD", "BAR", ALL),
            ("STD", "FITF", RO_BB),
            ("STD", "OTA", RO_BB),
        ])
    );
    let deluxe_bar_bb = &foreigners[1].quote;
    assert_eq!((deluxe_bar_bb.total, deluxe_bar_bb.currency.as_str()), (3 * (10_000 + 2 * 1_500), "USD"));
    assert!(deluxe_bar_bb.restrictions_ok, "{:?}", deluxe_bar_bb.violations);
    let kinds =
        |index: usize| -> Vec<ViolationKind> { foreigners[index].quote.violations.iter().map(|v| v.kind).collect() };
    assert_eq!(kinds(2), [ViolationKind::NoMealSupplement], "half board has no supplement on the last night");
    assert_eq!(kinds(5), [ViolationKind::MinStay], "OTA needs 4 nights over the second night");
    assert_eq!(kinds(3), [ViolationKind::ClosedToDeparture]);
    assert_eq!(kinds(7), [ViolationKind::NoPrice], "BAR has no standard price on the last night");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn offers_can_be_limited_to_some_room_types_and_refuse_invalid_stays(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed(&hotel).await;
    let mut tx = hotel.tx().await;
    let standard_only =
        OfferRequest { room_type_ids: Some(vec![hotel.standard.id]), ..request(&hotel, Residency::NonResident, 1, 0) };
    let backwards = OfferRequest { check_out: hotel.day(0), ..request(&hotel, Residency::NonResident, 1, 0) };
    let too_long = OfferRequest {
        check_out: hotel.day(rates::MAX_STAY_NIGHTS + 1),
        ..request(&hotel, Residency::NonResident, 1, 0)
    };

    let offers = rates::load_offers(&mut tx, hotel.property, &standard_only).await.unwrap();
    let backwards = rates::load_offers(&mut tx, hotel.property, &backwards).await;
    let too_long = rates::load_offers(&mut tx, hotel.property, &too_long).await;
    let elsewhere = rates::load_offers(&mut tx, Uuid::now_v7(), &request(&hotel, Residency::Resident, 1, 0)).await;

    assert_eq!(offers.len(), 7);
    assert!(offers.iter().all(|offer| offer.room_type_id == hotel.standard.id));
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "check-out is after check-in"), "{backwards:?}");
    assert!(matches!(&too_long, Err(RatesError::Invalid(m)) if m.contains("exceeds maximum")), "{too_long:?}");
    assert_eq!(elsewhere.unwrap(), [], "another property's plans are not offered");
}
