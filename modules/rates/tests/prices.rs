mod common;

use common::Hotel;
use rates::{BulkChange, ChangeMode, Price, PriceChange, PriceChangeMode, RatePlanChanges, RatesError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Weekday};
use uuid::Uuid;

fn invalid(result: Result<impl std::fmt::Debug, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derived_plans_are_repriced_in_the_same_transaction(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel
        .plan(rates::NewRatePlan { rounding_step: 100, ..hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500) });
    let ota = ota.await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -1_050)).await;

    let mut tx = hotel.tx().await;
    let prices = [Price { room_type_id: hotel.deluxe.id, date: hotel.day(3), occupancy: 2, amount: 10_050 }];
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &prices).await.unwrap();
    let inside = rates::list_prices(&mut tx, hotel.property, ota_nr.id, hotel.day(0), hotel.day(7)).await.unwrap();
    tx.rollback().await.unwrap();
    hotel.try_prices(&bar, &prices).await.unwrap();

    assert_eq!(inside.len(), 1, "the grandchild's row is written before the transaction commits");
    // 100.50 + 15% = 115.575, rounded to 116.00; then -10.50 = 105.50.
    assert_eq!(hotel.stored(&ota).await[0].amount, 11_600);
    assert_eq!(hotel.stored(&ota_nr).await, [Price { amount: 10_550, ..prices[0] }]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_hand_priced_plans_take_prices_for_what_they_sell(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar =
        hotel.plan(rates::NewRatePlan { room_type_ids: vec![hotel.deluxe.id], ..hotel.standard_plan("BAR", "USD") });
    let bar = bar.await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let price = |room_type: Uuid, day: i64, occupancy: i32| {
        [Price { room_type_id: room_type, date: hotel.day(day), occupancy, amount: 100 }]
    };

    let derived_plan = hotel.try_prices(&ota, &price(hotel.deluxe.id, 0, 1)).await;
    let unsold_type = hotel.try_prices(&bar, &price(hotel.standard.id, 0, 1)).await;
    let too_many_adults = hotel.try_prices(&bar, &price(hotel.deluxe.id, 0, 4)).await;
    let yesterday = hotel.try_prices(&bar, &price(hotel.deluxe.id, -1, 1)).await;
    let past_the_window = hotel.try_prices(&bar, &price(hotel.deluxe.id, rooms::WINDOW_DAYS, 1)).await;
    let mut tx = hotel.tx().await;
    let unknown_plan = rates::set_prices(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        Uuid::now_v7(),
        &price(hotel.deluxe.id, 0, 1),
    )
    .await;

    assert_eq!(invalid(derived_plan), "OTA is derived from BAR; change BAR's prices instead");
    assert_eq!(invalid(unsold_type), "BAR does not sell this room type");
    assert_eq!(invalid(too_many_adults), "DLX takes 1 to 3 guests");
    let window = "prices and restrictions are set from the business date for 730 days";
    assert_eq!((invalid(yesterday), invalid(past_the_window)), (window.to_owned(), window.to_owned()));
    assert!(matches!(unknown_plan, Err(RatesError::NotFound("rate plan"))), "{unknown_plan:?}");
}

/// A price dated `9999-12-31` (`Date::MAX`) is out of the rate window, not a panic: computing `date + 1 day`
/// to build the checked range must not overflow before the window check runs.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_price_dated_the_maximum_date_is_rejected_without_panicking(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let at_max = [Price { room_type_id: hotel.deluxe.id, date: Date::MAX, occupancy: 2, amount: 100 }];

    let result = hotel.try_prices(&bar, &at_max).await;

    let window = "prices and restrictions are set from the business date for 730 days";
    assert_eq!(invalid(result), window);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_or_changed_derived_plan_is_priced_from_its_parent(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rack = hotel.plan(hotel.standard_plan("RACK", "USD")).await;
    hotel.try_prices(&bar, &hotel.prices(hotel.deluxe.id, 0, 10, 2, 10_000)).await.unwrap();
    hotel.try_prices(&rack, &hotel.prices(hotel.deluxe.id, 5, 10, 2, 20_000)).await.unwrap();

    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_000)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -500)).await;
    let created = hotel.stored(&ota_nr).await;
    let cheaper = RatePlanChanges { derive_value: Some(-1_000), ..RatePlanChanges::default() };
    let ota = hotel.try_update(&ota, cheaper).await.unwrap();
    let after_formula = hotel.stored(&ota_nr).await;
    let moved = RatePlanChanges { parent_id: Some(rack.id), ..RatePlanChanges::default() };
    hotel.try_update(&ota, moved).await.unwrap();
    let after_move = hotel.stored(&ota_nr).await;

    assert_eq!(created.len(), 10);
    assert!(created.iter().all(|p| p.amount == 10_500), "110.00 - 5.00");
    assert!(after_formula.iter().all(|p| p.amount == 8_500), "90.00 - 5.00");
    assert_eq!(after_move.len(), 5, "days RACK has no price for are gone");
    assert!(after_move.iter().all(|p| p.date >= hotel.day(5) && p.amount == 17_500), "180.00 - 5.00");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_touches_exactly_what_it_selects_and_cascades(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    for room_type in [hotel.deluxe.id, hotel.standard.id] {
        for occupancy in [1, 2] {
            hotel.try_prices(&bar, &hotel.prices(room_type, 0, 60, occupancy, 10_000)).await.unwrap();
        }
    }
    let change = BulkChange {
        from: hotel.day(10),
        to: hotel.day(40),
        weekdays: vec![Weekday::Saturday, Weekday::Sunday],
        room_type_ids: vec![hotel.deluxe.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Percent, value: 1_000 },
    };

    let mut tx = hotel.tx().await;
    let changed = rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &change).await.unwrap();
    tx.commit().await.unwrap();

    let selected = |p: &Price| {
        p.room_type_id == hotel.deluxe.id
            && p.date >= hotel.day(10)
            && p.date < hotel.day(40)
            && matches!(p.date.weekday(), Weekday::Saturday | Weekday::Sunday)
    };
    let bar_prices = hotel.stored(&bar).await;
    assert_eq!(bar_prices.len(), 240);
    assert_eq!(changed, u64::try_from(bar_prices.iter().filter(|p| selected(p)).count()).unwrap());
    assert!((16..=18).contains(&changed), "8 or 9 weekend days, 2 occupancies: {changed}");
    for price in &bar_prices {
        assert_eq!(price.amount, if selected(price) { 11_000 } else { 10_000 }, "{price:?}");
    }
    for price in hotel.stored(&ota).await {
        assert_eq!(price.amount, if selected(&price) { 12_650 } else { 11_500 }, "OTA {price:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_set_fills_cells_and_its_preview_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    hotel.try_prices(&bar, &hotel.prices(hotel.standard.id, 0, 1, 1, 9_000)).await.unwrap();
    let change = BulkChange {
        from: hotel.day(0),
        to: hotel.day(7),
        weekdays: vec![],
        room_type_ids: vec![hotel.standard.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Set, value: 12_000 },
    };

    let mut tx = hotel.tx().await;
    let preview = rates::preview_bulk_change(&mut tx, hotel.property, bar.id, &change, 5).await.unwrap();
    tx.commit().await.unwrap();
    let before = hotel.stored(&bar).await;
    let mut tx = hotel.tx().await;
    let changed = rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &change).await.unwrap();
    tx.commit().await.unwrap();
    let again = rates::preview_bulk_change(&mut hotel.tx().await, hotel.property, bar.id, &change, 5).await.unwrap();

    assert_eq!(preview.total, 14, "7 days, occupancies 1 and 2 of STD");
    assert_eq!(preview.cells.len(), 5, "at most the requested number of cells");
    assert_eq!((preview.cells[0].before, preview.cells[0].after), (Some(9_000), 12_000));
    assert_eq!((preview.cells[1].before, preview.cells[1].occupancy), (None, 2));
    assert_eq!(before.len(), 1, "a preview writes nothing");
    assert_eq!(changed, 14);
    assert!(hotel.stored(&bar).await.iter().all(|p| p.amount == 12_000));
    assert_eq!(again.total, 0, "nothing left to change");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_stays_in_the_window_and_on_hand_priced_plans(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let change = |from: i64, to: i64| BulkChange {
        from: hotel.day(from),
        to: hotel.day(to),
        weekdays: vec![],
        room_type_ids: vec![hotel.deluxe.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Amount, value: 500 },
    };
    let bulk = |plan: Uuid, change: BulkChange| {
        let hotel = &hotel;
        async move {
            let mut tx = hotel.tx().await;
            rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, plan, &change).await
        }
    };

    assert_eq!(invalid(bulk(ota.id, change(0, 7)).await), "OTA is derived from BAR; change BAR's prices instead");
    let window = "prices and restrictions are set from the business date for 730 days";
    assert_eq!(invalid(bulk(bar.id, change(-1, 7)).await), window);
    assert_eq!(invalid(bulk(bar.id, change(0, rooms::WINDOW_DAYS + 1)).await), window);
    assert_eq!(invalid(bulk(bar.id, change(5, 5)).await), "the range ends after it starts");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_price_exceeding_max_amount_rolls_back(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    // +200% derivation: parent 50B × 3 = 150B > MAX_AMOUNT (100B)
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 20_000)).await;
    let parent_price = 50_000_000_000_i64; // +200% would give 150B, exceeding 100B limit
    let prices = [Price { room_type_id: hotel.deluxe.id, date: hotel.day(0), occupancy: 1, amount: parent_price }];

    let result = hotel.try_prices(&bar, &prices).await;

    assert_eq!(invalid(result), "this change would make a price larger than 100000000000 minor units");
    assert_eq!(hotel.stored(&bar).await.len(), 0, "parent prices unchanged");
    assert_eq!(hotel.stored(&ota).await.len(), 0, "derived prices unchanged");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_percent_that_would_exceed_max_amount_is_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let initial = hotel.prices(hotel.deluxe.id, 0, 1, 1, 9_500_000_000_i64);
    hotel.try_prices(&bar, &initial).await.unwrap();
    let change = BulkChange {
        from: hotel.day(0),
        to: hotel.day(1),
        weekdays: vec![],
        room_type_ids: vec![hotel.deluxe.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Percent, value: 100_000 }, // +1000%, would give 95_000_000_000 * 11
    };

    let result = {
        let mut tx = hotel.tx().await;
        rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &change).await
    };

    assert_eq!(invalid(result), "this change would make a price larger than 100000000000 minor units");
    assert_eq!(hotel.stored(&bar).await[0].amount, 9_500_000_000_i64, "price unchanged");
}
