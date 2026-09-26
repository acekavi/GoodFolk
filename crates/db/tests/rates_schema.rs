//! Constraints the Phase 2 migration puts on rate plans, prices and meal supplements, and the price
//! derivation function, checked directly in the database.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
}

/// A tenant with one property and one room type. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel { tenant: Uuid::now_v7(), property: Uuid::now_v7(), room_type: Uuid::now_v7() };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, $3, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(hotel.property)
    .bind(hotel.tenant)
    .bind(code)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 0, 2)",
    )
    .bind(hotel.room_type)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
    hotel
}

/// Inserts a rate plan; `parent` makes it a derived plan (+10%).
async fn plan(
    pool: &PgPool,
    hotel: &Hotel,
    code: &str,
    kind: &str,
    parent: Option<Uuid>,
    segment: &str,
    residency: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let (mode, value) = if kind == "derived" { (Some("percent"), Some(1000_i64)) } else { (None, None) };
    sqlx::query(
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency, parent_id,
                                derive_mode, derive_value)
         values ($1, $2, $3, $4, $4, $5, $6, $7, 'USD', $8, $9, $10)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(code)
    .bind(kind)
    .bind(segment)
    .bind(residency)
    .bind(parent)
    .bind(mode)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(id)
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_and_only_a_derived_plan_has_a_parent(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let bar = plan(&pool, &hotel, "BAR", "standard", None, "IBE", None).await.unwrap();

    let orphan = plan(&pool, &hotel, "OTA", "derived", None, "OTA", None).await;
    let standard_with_parent = plan(&pool, &hotel, "TA", "standard", Some(bar), "TA", None).await;
    let derived = plan(&pool, &hotel, "OTA", "derived", Some(bar), "OTA", None).await;

    assert_eq!(constraint(orphan), "rate_plan_derivation_check");
    assert_eq!(constraint(standard_with_parent), "rate_plan_derivation_check");
    assert!(derived.is_ok(), "{derived:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fit_segments_carry_their_residency(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let foreign_for_residents = plan(&pool, &hotel, "FITF", "custom", None, "FIT_F", Some("resident")).await;
    let local_for_anyone = plan(&pool, &hotel, "FITL", "custom", None, "FIT_L", None).await;
    let foreign = plan(&pool, &hotel, "FITF", "custom", None, "FIT_F", Some("non_resident")).await;
    let local = plan(&pool, &hotel, "FITL", "custom", None, "FIT_L", Some("resident")).await;

    assert_eq!(constraint(foreign_for_residents), "rate_plan_segment_residency_check");
    assert_eq!(constraint(local_for_anyone), "rate_plan_segment_residency_check");
    assert!(foreign.is_ok() && local.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_plan_cannot_derive_from_another_propertys_plan(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;
    let theirs = plan(&pool, &kandy, "BAR", "standard", None, "IBE", None).await.unwrap();

    let derived = plan(&pool, &galle, "OTA", "derived", Some(theirs), "OTA", None).await;

    assert_eq!(constraint(derived), "rate_plan_property_id_parent_id_fkey");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn prices_and_restrictions_exist_only_for_room_types_the_plan_sells(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let bar = plan(&pool, &hotel, "BAR", "standard", None, "IBE", None).await.unwrap();
    let price = "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
                 values ($1, $2, $3, $4, current_date, 2, 15000)";
    let restriction = "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed)
                       values ($1, $2, $3, $4, current_date, true)";
    let insert = |sql: &'static str| {
        sqlx::query(sql).bind(hotel.tenant).bind(hotel.property).bind(bar).bind(hotel.room_type).execute(&pool)
    };

    let unsold_price = insert(price).await;
    let unsold_restriction = insert(restriction).await;
    sqlx::query(
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id) values ($1, $2, $3, $4)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(bar)
    .bind(hotel.room_type)
    .execute(&pool)
    .await
    .unwrap();
    insert(price).await.unwrap();
    insert(restriction).await.unwrap();
    sqlx::query("delete from rate_plan_room_type").execute(&pool).await.unwrap();
    let left: (i64, i64) =
        sqlx::query_as("select (select count(*) from rate_day), (select count(*) from rate_restriction)")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(constraint(unsold_price), "rate_day_property_id_rate_plan_id_room_type_id_fkey");
    assert_eq!(constraint(unsold_restriction), "rate_restriction_property_id_rate_plan_id_room_type_id_fkey");
    assert_eq!(left, (0, 0), "no longer selling a room type removes its prices and restrictions");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn meal_supplements_for_one_meal_plan_and_currency_cannot_overlap(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let supplement = |meal_plan: &'static str, currency: &'static str, from: i32, to: Option<i32>| {
        sqlx::query(
            "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
             values ($1, $2, $3, $4, $5, 1500, 750, daterange(current_date + $6, current_date + $7))",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(meal_plan)
        .bind(currency)
        .bind(from)
        .bind(to)
        .execute(&pool)
    };
    supplement("BB", "USD", 0, None).await.unwrap();

    let overlapping = supplement("BB", "USD", 30, Some(60)).await;
    let other_currency = supplement("BB", "LKR", 30, Some(60)).await;
    let other_meal_plan = supplement("HB", "USD", 30, Some(60)).await;
    let room_only = supplement("RO", "USD", 0, None).await;

    assert_eq!(constraint(overlapping), "meal_supplement_no_overlap");
    assert!(other_currency.is_ok() && other_meal_plan.is_ok());
    assert_eq!(constraint(room_only), "meal_supplement_meal_plan_check");
}

/// `app.derive_amount(base, mode, value, step)` computed exactly: `base` plus `value` basis points
/// (`percent`) or minor units (`amount`), at least 0, rounded half-up to a multiple of `step`.
fn derived(base: i64, mode: &str, value: i64, step: i64) -> i64 {
    let (numerator, denominator) = match mode {
        "percent" => (i128::from(base) * (10_000 + i128::from(value)), 10_000_i128),
        _ => (i128::from(base) + i128::from(value), 1),
    };
    let unit = denominator * i128::from(step);
    let (whole, rest) = (numerator.max(0) / unit, numerator.max(0) % unit);
    let steps = if 2 * rest >= unit { whole + 1 } else { whole };
    i64::try_from(steps * i128::from(step)).unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derive_amount_rounds_half_up_to_the_step(pool: PgPool) {
    let cases: [(i64, &str, i64, i64, i64); 8] = [
        (10_000, "percent", 1_500, 1, 11_500),   // +15%
        (10_050, "percent", 1_500, 100, 11_600), // 115.575 rounds to 116.00
        (1_000, "percent", 500, 100, 1_100),     // 10.50 is a half: up to 11.00
        (10_000, "percent", -500, 1, 9_500),     // -5%
        (10_000, "percent", -10_000, 1, 0),      // -100%
        (10_000, "amount", -2_550, 100, 7_500),  // 74.50 is a half: up to 75.00
        (1_000, "amount", -5_000, 1, 0),         // never below zero
        (0, "amount", 0, 100, 0),
    ];
    for (base, mode, value, step, expected) in cases {
        let actual: i64 = sqlx::query_scalar("select app.derive_amount($1, $2, $3, $4)")
            .bind(base)
            .bind(mode)
            .bind(value)
            .bind(step)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, expected, "{base} {mode} {value} step {step}");
        assert_eq!(derived(base, mode, value, step), expected, "the test's own model");
    }
}

/// A NULL `base` or `value` gives NULL, not 0: `greatest()` ignores NULL arguments, so without an explicit
/// NULL arm the CASE would silently treat a NULL input as if it were 0.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derive_amount_is_null_for_a_null_base_or_value(pool: PgPool) {
    let null_base: Option<i64> =
        sqlx::query_scalar("select app.derive_amount(null, 'percent', 0, 1)").fetch_one(&pool).await.unwrap();
    let null_value: Option<i64> =
        sqlx::query_scalar("select app.derive_amount(100, 'percent', null, 1)").fetch_one(&pool).await.unwrap();

    assert_eq!(null_base, None);
    assert_eq!(null_value, None);
}

/// Random inputs across the whole allowed range, compared with exact arithmetic. Each run draws new ones.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derive_amount_matches_exact_arithmetic(pool: PgPool) {
    let step = prop_oneof![Just(1_i64), Just(5), Just(10), Just(50), Just(100), Just(1_000), 1..=1_000_000_i64];
    let case = prop_oneof![
        (0..=100_000_000_000_i64, -10_000..=100_000_i64, step.clone()).prop_map(|(b, v, s)| (b, "percent", v, s)),
        (0..=100_000_000_000_i64, -100_000_000_000..=100_000_000_000_i64, step)
            .prop_map(|(b, v, s)| (b, "amount", v, s)),
    ];
    let mut runner = TestRunner::default();
    let cases: Vec<(i64, &str, i64, i64)> = (0..5_000).map(|_| case.new_tree(&mut runner).unwrap().current()).collect();

    let actual: Vec<i64> = sqlx::query_scalar(
        "select app.derive_amount(c.base, c.mode, c.value, c.step)
         from unnest($1::bigint[], $2::text[], $3::bigint[], $4::bigint[]) with ordinality as c (base, mode, value, step, n)
         order by c.n",
    )
    .bind(cases.iter().map(|c| c.0).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.1).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.2).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.3).collect::<Vec<_>>())
    .fetch_all(&pool)
    .await
    .unwrap();

    for (case, actual) in cases.iter().zip(actual) {
        assert_eq!(actual, derived(case.0, case.1, case.2, case.3), "app.derive_amount{case:?}");
    }
}
