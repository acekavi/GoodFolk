//! Performance gates, timed in-process through the router, so they measure server time (authentication, the
//! queries and JSON) without the network:
//!
//! - Phase 1: one month of `inventory` for a 200-room, 12-type property in under 20 ms at p95.
//! - Phase 2: a 62-day `rateGrid` for 1 plan and 12 room types with 2 occupancies (1488 prices, plus a
//!   restriction per type and day) in under 30 ms at p95; a bulk change of one year of prices for 12 room
//!   types, with two levels of derived plans below it, in under 300 ms (median of ten runs).
//! - Phase 3: creating a reservation (1 room, 3 nights, a 12-type property with a standard plan priced for 400
//!   days, restrictions and BB/HB supplements) in under 60 ms at p95; the reservations list, 50 rows filtered
//!   by arrival and status out of 10k reservation rooms, in under 25 ms at p95; availability for 7 nights
//!   across 12 room types and 5 rate plans (derived plans included, room only and breakfast) in under 40 ms at
//!   p95.
//!
//! Ignored by default because debug builds are several times slower. Run them in release mode, one at a time:
//!
//! ```sh
//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
//! ```

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::{Duration, Instant};
use time::format_description::well_known::Iso8601;
use uuid::Uuid;

const ROOM_TYPES: u32 = 12;
const ROOMS: u32 = 200;
const SAMPLES: usize = 200;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
    response
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn a_month_of_inventory_for_200_rooms_is_served_under_20ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let property = json!({"code": "BIG", "name": "Big Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let property = post(&app, &owner, "/api/v1/properties", property).await.body;
    let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
    let business_date = time::Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
    let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'RENOVATION'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    for index in 0..ROOM_TYPES {
        let room_type = json!({"code": format!("T{index}"), "name": format!("Type {index}"), "base_occupancy": 2,
                               "max_adults": 2, "max_children": 1, "max_occupancy": 3});
        let room_type = post(&app, &owner, &format!("{path}/room-types"), room_type).await.body["id"].clone();
        let count = ROOMS / ROOM_TYPES + u32::from(index < ROOMS % ROOM_TYPES);
        let first = (index + 1) * 100 + 1;
        let range = json!({"room_type_id": room_type, "first": first, "last": first + count - 1});
        let rooms = post(&app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        // Two blocks per type inside the measured month.
        for (offset, room) in rooms.as_array().unwrap().iter().take(2).enumerate() {
            let from = business_date + time::Duration::days(i64::try_from(offset).unwrap() * 7 + 3);
            let block = json!({"from": from.to_string(), "to": (from + time::Duration::days(4)).to_string(),
                               "kind": "out_of_order", "reason_id": reason});
            post(&app, &owner, &format!("{path}/rooms/{}/blocks", room["id"].as_str().unwrap()), block).await;
        }
    }
    let query = json!({
        "query": "query ($p: UUID!, $from: Date!, $to: Date!) {
                    inventory(propertyId: $p, from: $from, to: $to) { date roomTypeId physical sold outOfOrder available }
                  }",
        "variables": {"p": property["id"], "from": business_date.to_string(),
                      "to": (business_date + time::Duration::days(31)).to_string()},
    });

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES + 20 {
        let started = Instant::now();
        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
        let elapsed = started.elapsed();
        assert_eq!(response.body["data"]["inventory"].as_array().map(Vec::len), Some(31 * 12), "{:?}", response.body);
        // The first 20 warm the connection pool and Postgres' caches.
        if round >= 20 {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100 - 1];
    println!("inventory(month), {ROOMS} rooms / {ROOM_TYPES} types: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(20), "p95 {p95:?} is over the 20 ms gate");
}

/// An owner's property with 12 room types for up to 2 adults and a child; returns the property's API path,
/// its business date and the room type ids.
async fn property_with_room_types(app: &TestApp, owner: &str) -> (String, time::Date, Vec<Value>) {
    let property = json!({"code": "RATES", "name": "Rates Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let property = post(app, owner, "/api/v1/properties", property).await.body;
    let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
    let business_date = time::Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
    let mut types = Vec::new();
    for index in 0..ROOM_TYPES {
        let room_type = json!({"code": format!("T{index}"), "name": format!("Type {index}"), "base_occupancy": 2,
                               "max_adults": 2, "max_children": 1, "max_occupancy": 3});
        types.push(post(app, owner, &format!("{path}/room-types"), room_type).await.body["id"].clone());
    }
    (path, business_date, types)
}

/// Prices for 1 and 2 adults in every room type on each day of `[business date, + days)`, in requests of at
/// most 5000 prices.
async fn price_every_cell(app: &TestApp, owner: &str, plan_path: &str, day0: time::Date, types: &[Value], days: i64) {
    let prices: Vec<Value> = (0..days)
        .flat_map(|day| types.iter().map(move |room_type| (day, room_type)))
        .flat_map(|(day, room_type)| {
            let date = (day0 + time::Duration::days(day)).to_string();
            (1..=2).map(move |occupancy| {
                json!({"room_type_id": room_type, "date": date, "occupancy": occupancy, "amount": 10_000 + day * 10})
            })
        })
        .collect();
    for batch in prices.chunks(5000) {
        let response =
            app.send(Method::PUT, &format!("{plan_path}/prices"), Some(owner), Some(json!({"prices": batch}))).await;
        assert_eq!(response.status, StatusCode::NO_CONTENT, "{:?}", response.body);
    }
}

fn rate_plan(code: &str, types: &[Value], parent: Option<&Value>) -> Value {
    match parent {
        None => json!({"code": code, "name": code, "kind": "standard", "segment": "IBE", "currency": "USD",
                       "room_type_ids": types}),
        Some(parent) => json!({"code": code, "name": code, "kind": "derived", "segment": "OTA", "currency": "USD",
                               "parent_id": parent["id"], "derive_mode": "percent", "derive_value": 1500,
                               "rounding_step": 100, "inherit_restrictions": true, "room_type_ids": types}),
    }
}

/// [`rate_plan`], selling `meal_plans` instead of the default (room only alone).
fn plan_with_meals(code: &str, types: &[Value], parent: Option<&Value>, meal_plans: &[&str]) -> Value {
    let mut plan = rate_plan(code, types, parent);
    plan["allowed_meal_plans"] = json!(meal_plans);
    plan
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn a_62_day_rate_grid_for_12_room_types_is_served_under_30ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
    const DAYS: i64 = 62;
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (path, day0, types) = property_with_room_types(&app, &owner).await;
    let bar = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("BAR", &types, None)).await.body;
    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
    price_every_cell(&app, &owner, &bar_path, day0, &types, DAYS).await;
    let to = (day0 + time::Duration::days(DAYS)).to_string();
    let restriction = json!({"from": day0.to_string(), "to": to, "min_stay": 2, "closed_to_arrival": false});
    let restricted = app.send(Method::PUT, &format!("{bar_path}/restrictions"), Some(&owner), Some(restriction)).await;
    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
    let query = json!({
        "query": "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
                    rateGrid(propertyId: $p, ratePlanId: $plan, from: $from, to: $to) {
                      prices { roomTypeId date occupancy amount }
                      restrictions { roomTypeId date closed minStay maxStay closedToArrival closedToDeparture }
                    }
                  }",
        "variables": {"p": path.trim_start_matches("/api/v1/properties/"), "plan": bar["id"],
                      "from": day0.to_string(), "to": to},
    });

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES + 20 {
        let started = Instant::now();
        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
        let elapsed = started.elapsed();
        let grid = &response.body["data"]["rateGrid"];
        assert_eq!(grid["prices"].as_array().map(Vec::len), Some(1488), "{:?}", response.body);
        assert_eq!(grid["restrictions"].as_array().map(Vec::len), Some(744));
        if round >= 20 {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100 - 1];
    println!("rateGrid, 62 days x 12 types x 2 occupancies: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(30), "p95 {p95:?} is over the 30 ms gate");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn a_year_of_bulk_change_with_two_derived_levels_takes_under_300ms_at_the_median(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    const DAYS: i64 = 365;
    const RUNS: usize = 10;
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (path, day0, types) = property_with_room_types(&app, &owner).await;
    let bar = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("BAR", &types, None)).await.body;
    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
    price_every_cell(&app, &owner, &bar_path, day0, &types, DAYS).await;
    let ota = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("OTA", &types, Some(&bar))).await.body;
    post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("OTA-NR", &types, Some(&ota))).await;
    let rows: i64 = sqlx::query_scalar("select count(*) from rate_day").fetch_one(&superuser).await.unwrap();
    assert_eq!(rows, 3 * 365 * 12 * 2, "three plans, every cell priced");
    // As autovacuum leaves a table after a bulk load, so it does not start in the middle of the timed runs.
    sqlx::query("vacuum analyze rate_day").execute(&superuser).await.unwrap();

    let mut runs: Vec<Duration> = Vec::with_capacity(RUNS);
    for run in 0..RUNS + 2 {
        // Up 5 %, then down 5 %, so every run changes every price.
        let value = if run % 2 == 0 { 500 } else { -500 };
        let change = json!({"from": day0.to_string(), "to": (day0 + time::Duration::days(DAYS)).to_string(),
                            "change": {"mode": "percent", "value": value}});
        let started = Instant::now();
        let response = post_ok(&app, &owner, &format!("{bar_path}/bulk-change"), change).await;
        let elapsed = started.elapsed();
        assert_eq!(response.body["changed"], 365 * 12 * 2, "{:?}", response.body);
        // The first two warm the connection pool and Postgres' caches.
        if run >= 2 {
            runs.push(elapsed);
        }
    }

    runs.sort();
    let (median, slowest) = (runs[RUNS / 2], runs[RUNS - 1]);
    println!(
        "bulk change, 365 days x 12 types x 2 occupancies, 2 derived levels: median {median:?}, slowest {slowest:?}"
    );
    // The median of ten runs, like the p95 of the read gates: one slow run on a busy machine is noise.
    assert!(median < Duration::from_millis(300), "median {median:?} is over the 300 ms gate");
}

/// Sends a command with a fresh idempotency key and expects 200.
async fn post_ok(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn creating_a_reservation_is_served_under_60ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
    const ROOMS_PER_TYPE: u32 = 20;
    const DAYS: i64 = 400;
    const WARMUP: usize = 10;
    const CREATES: usize = 50;
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (path, day0, types) = property_with_room_types(&app, &owner).await;
    for (index, room_type) in types.iter().enumerate() {
        let first = (u32::try_from(index).unwrap() + 1) * 100 + 1;
        let range = json!({"room_type_id": room_type, "first": first, "last": first + ROOMS_PER_TYPE - 1});
        post(&app, &owner, &format!("{path}/rooms/bulk"), range).await;
    }
    let standard = plan_with_meals("STD", &types, None, &["RO", "BB", "HB"]);
    let standard = post(&app, &owner, &format!("{path}/rate-plans"), standard).await.body;
    let plan_id = standard["id"].clone();
    let plan_path = format!("{path}/rate-plans/{}", standard["id"].as_str().unwrap());
    price_every_cell(&app, &owner, &plan_path, day0, &types, DAYS).await;
    // Restrictions on some days, well past the dates this gate books, so they never block a booking.
    let restriction = json!({"from": (day0 + time::Duration::days(300)).to_string(),
                             "to": (day0 + time::Duration::days(320)).to_string(),
                             "min_stay": 1, "closed_to_arrival": false});
    let restricted = app.send(Method::PUT, &format!("{plan_path}/restrictions"), Some(&owner), Some(restriction)).await;
    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
    for (meal_plan, adult_amount) in [("BB", 1_500i64), ("HB", 3_000i64)] {
        let supplement = json!({"meal_plan": meal_plan, "currency": "USD", "adult_amount": adult_amount,
                                "child_amount": adult_amount / 2, "from": day0.to_string()});
        post(&app, &owner, &format!("{path}/meal-supplements"), supplement).await;
    }
    let guest = json!({"first_name": "Ada", "last_name": "Booker", "residency": "non_resident"});
    let guest = post(&app, &owner, &format!("{path}/guests"), guest).await.body["id"].clone();

    let reservations_path = format!("{path}/reservations");
    let meal_plans = ["RO", "BB", "HB"];
    let mut samples: Vec<Duration> = Vec::with_capacity(CREATES);
    for round in 0..CREATES + WARMUP {
        // A distinct check-in date each round, so bookings never wait on one another's counter locks.
        let check_in = day0 + time::Duration::days(i64::try_from(round).unwrap() + 1);
        let check_out = check_in + time::Duration::days(3);
        let room_type = &types[round % types.len()];
        let meal_plan = meal_plans[round % meal_plans.len()];
        let body = json!({"booker_guest_id": guest, "source": "front_desk",
                          "rooms": [{"room_type_id": room_type, "rate_plan_id": plan_id, "meal_plan": meal_plan,
                                     "check_in": check_in.to_string(), "check_out": check_out.to_string(),
                                     "adults": 2}]});
        let started = Instant::now();
        let response = post(&app, &owner, &reservations_path, body).await;
        let elapsed = started.elapsed();
        assert_eq!(response.body["rooms"].as_array().map(Vec::len), Some(1), "{:?}", response.body);
        // The first ten warm the connection pool and Postgres' caches.
        if round >= WARMUP {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[CREATES / 2];
    let p95 = samples[CREATES * 95 / 100 - 1];
    println!("create reservation, 1 room x 3 nights, {ROOM_TYPES} types: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(60), "p95 {p95:?} is over the 60 ms gate");
}

/// The SPA's reservations-list query, as `crates/core-api/tests/reservation_reads.rs`'s `LIST` sends it.
const RESERVATIONS_LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter, $withCount: Boolean!) {
    reservations(propertyId: $p, filter: $filter, first: 50) {
        nodes {
            id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
            total currency version accountName
        }
        pageInfo { endCursor hasNextPage }
        totalCount @include(if: $withCount)
    }
}";

/// Inserts `count` reservation rooms directly with batched, `unnest`-based SQL through the owner pool (the
/// reservations module's own `create_reservation`, one row per round trip, would take far too long for 10k
/// rows). Spread over a year of arrivals across a small pool of guests and every room type, with a realistic
/// mix of statuses, so the list's arrival and status filters have a genuine slice to match. No room is
/// assigned and the inventory counters are never touched: this gate's query reads none of them.
async fn seed_reservation_rooms(
    superuser: &PgPool,
    tenant: Uuid,
    property: Uuid,
    rate_plan: Uuid,
    types: &[Uuid],
    business_date: time::Date,
    count: usize,
) {
    const GUESTS: usize = 200;
    const SPREAD_DAYS: i64 = 365;
    const STATUSES: [&str; 5] = ["confirmed", "checked_in", "checked_out", "tentative", "no_show"];
    const MEAL_PLANS: [&str; 3] = ["RO", "BB", "HB"];

    let guest_ids: Vec<Uuid> = (0..GUESTS).map(|_| Uuid::now_v7()).collect();
    let guest_names: Vec<String> = (0..GUESTS).map(|index| format!("Guest{index}")).collect();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency)
         select g.id, $1, '', g.name, 'non_resident' from unnest($2::uuid[], $3::text[]) as g (id, name)",
    )
    .bind(tenant)
    .bind(&guest_ids)
    .bind(&guest_names)
    .execute(superuser)
    .await
    .unwrap();

    let room_ids: Vec<Uuid> = (0..count).map(|_| Uuid::now_v7()).collect();
    let reservation_ids: Vec<Uuid> = (0..count).map(|_| Uuid::now_v7()).collect();
    let confirmations: Vec<String> = (0..count).map(|index| format!("PERF-{index:06}")).collect();
    let sources: Vec<&str> = (0..count).map(|_| "front_desk").collect();
    let bookers: Vec<Uuid> = (0..count).map(|index| guest_ids[index % GUESTS]).collect();
    sqlx::query(
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         select r.id, $1, $2, r.confirmation_no, r.source, r.booker_guest_id
         from unnest($3::uuid[], $4::text[], $5::text[], $6::uuid[])
              as r (id, confirmation_no, source, booker_guest_id)",
    )
    .bind(tenant)
    .bind(property)
    .bind(&reservation_ids)
    .bind(&confirmations)
    .bind(&sources)
    .bind(&bookers)
    .execute(superuser)
    .await
    .unwrap();

    let arrivals: Vec<time::Date> = (0..count)
        .map(|index| business_date + time::Duration::days(i64::try_from(index).unwrap() % SPREAD_DAYS))
        .collect();
    let room_types: Vec<Uuid> = (0..count).map(|index| types[index % types.len()]).collect();
    let statuses: Vec<&str> = (0..count).map(|index| STATUSES[index % STATUSES.len()]).collect();
    let meal_plans: Vec<&str> = (0..count).map(|index| MEAL_PLANS[index % MEAL_PLANS.len()]).collect();
    let primary_guests: Vec<Uuid> = (0..count).map(|index| guest_ids[(index + 1) % GUESTS]).collect();
    sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                        children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                        checked_in_at, checked_in_business_date, checked_out_at)
         select c.id, $1, $2, c.reservation_id, c.room_type_id, daterange(c.arrival, c.arrival + 2, '[)'), 2, 0,
                $3, c.meal_plan, c.status, c.primary_guest_id, 'USD',
                case when c.status in ('checked_in', 'checked_out') then now() end,
                case when c.status in ('checked_in', 'checked_out') then c.arrival end,
                case when c.status = 'checked_out' then now() end
         from unnest($4::uuid[], $5::uuid[], $6::uuid[], $7::date[], $8::text[], $9::text[], $10::uuid[])
              as c (id, reservation_id, room_type_id, arrival, meal_plan, status, primary_guest_id)",
    )
    .bind(tenant)
    .bind(property)
    .bind(rate_plan)
    .bind(&room_ids)
    .bind(&reservation_ids)
    .bind(&room_types)
    .bind(&arrivals)
    .bind(&meal_plans)
    .bind(&statuses)
    .bind(&primary_guests)
    .execute(superuser)
    .await
    .unwrap();

    // Two priced nights per room, so the list's per-row `total` subquery does the same work it would in
    // production.
    let mut night_room_ids = Vec::with_capacity(count * 2);
    let mut night_dates = Vec::with_capacity(count * 2);
    let mut night_room_amounts = Vec::with_capacity(count * 2);
    let mut night_meal_amounts = Vec::with_capacity(count * 2);
    for index in 0..count {
        let meal_amount: i64 = match meal_plans[index] {
            "BB" => 1_500,
            "HB" => 3_000,
            _ => 0,
        };
        for night in 0..2 {
            night_room_ids.push(room_ids[index]);
            night_dates.push(arrivals[index] + time::Duration::days(night));
            night_room_amounts.push(10_000i64);
            night_meal_amounts.push(meal_amount);
        }
    }
    sqlx::query(
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount,
                                         meal_amount, currency)
         select $1, $2, n.reservation_room_id, n.date, n.room_amount, n.meal_amount, 'USD'
         from unnest($3::uuid[], $4::date[], $5::bigint[], $6::bigint[])
              as n (reservation_room_id, date, room_amount, meal_amount)",
    )
    .bind(tenant)
    .bind(property)
    .bind(&night_room_ids)
    .bind(&night_dates)
    .bind(&night_room_amounts)
    .bind(&night_meal_amounts)
    .execute(superuser)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn a_filtered_50_row_reservations_list_is_served_under_25ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
    const ROOMS_SEEDED: usize = 10_000;
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (path, day0, types) = property_with_room_types(&app, &owner).await;
    let property = Uuid::parse_str(path.trim_start_matches("/api/v1/properties/")).unwrap();
    let tenant: Uuid = sqlx::query_scalar("select tenant_id from property where id = $1")
        .bind(property)
        .fetch_one(&superuser)
        .await
        .unwrap();
    let type_ids: Vec<Uuid> = types.iter().map(uuid).collect();
    let standard = json!({"code": "STD", "name": "Standard", "kind": "standard", "segment": "IBE",
                          "currency": "USD", "room_type_ids": types});
    let standard = post(&app, &owner, &format!("{path}/rate-plans"), standard).await.body;
    let rate_plan_id = uuid(&standard["id"]);

    seed_reservation_rooms(&superuser, tenant, property, rate_plan_id, &type_ids, day0, ROOMS_SEEDED).await;

    // A realistic filter: about 60 days of arrivals and one status, the app role's default first page.
    let filter = json!({"arrivalFrom": (day0 + time::Duration::days(100)).to_string(),
                        "arrivalTo": (day0 + time::Duration::days(160)).to_string(), "statuses": ["CONFIRMED"]});
    let query = json!({"query": RESERVATIONS_LIST, "variables": {"p": property, "filter": filter, "withCount": true}});

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES + 20 {
        let started = Instant::now();
        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
        let elapsed = started.elapsed();
        let nodes = response.body["data"]["reservations"]["nodes"].as_array();
        assert_eq!(nodes.map(Vec::len), Some(50), "{:?}", response.body);
        assert!(response.body["data"]["reservations"]["totalCount"].as_i64().unwrap() >= 50, "{:?}", response.body);
        // The first 20 warm the connection pool and Postgres' caches.
        if round >= 20 {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100 - 1];
    println!("reservations list, 50 rows filtered out of {ROOMS_SEEDED}: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(25), "p95 {p95:?} is over the 25 ms gate");
}

/// The SPA's availability query, as `crates/core-api/tests/reservation_reads.rs`'s `AVAILABILITY` sends it.
const AVAILABILITY: &str = "query ($p: UUID!, $in: Date!, $out: Date!) {
    availability(propertyId: $p, checkIn: $in, checkOut: $out, adults: 2, children: 0, residency: NON_RESIDENT) {
        roomTypeId code name free
        offers { ratePlanId ratePlanCode mealPlan total currency restrictionsOk violations { kind message }
                 nights { date room meal } }
    }
}";

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn availability_for_7_nights_12_types_and_5_plans_is_served_under_40ms_at_p95(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    const NIGHTS: i64 = 7;
    // Derived plans are at most 3 levels below a standard plan, so the chain below BAR tops out there; a
    // second, independent standard plan reaches 5 plans in all.
    const LEVELS: i32 = 3;
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (path, day0, types) = property_with_room_types(&app, &owner).await;
    let to = (day0 + time::Duration::days(NIGHTS)).to_string();

    // Two standard plans (room only and breakfast) priced for the search window, plus a breakfast
    // supplement, and three levels of derived plans below the first, all selling the same 12 types.
    let bar = plan_with_meals("BAR", &types, None, &["RO", "BB"]);
    let bar = post(&app, &owner, &format!("{path}/rate-plans"), bar).await.body;
    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
    price_every_cell(&app, &owner, &bar_path, day0, &types, NIGHTS).await;
    let corporate = plan_with_meals("CORP", &types, None, &["RO", "BB"]);
    let corporate = post(&app, &owner, &format!("{path}/rate-plans"), corporate).await.body;
    let corporate_path = format!("{path}/rate-plans/{}", corporate["id"].as_str().unwrap());
    price_every_cell(&app, &owner, &corporate_path, day0, &types, NIGHTS).await;
    let supplement = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1_500, "child_amount": 750,
                            "from": day0.to_string()});
    post(&app, &owner, &format!("{path}/meal-supplements"), supplement).await;
    let mut parent = bar;
    for level in 1..=LEVELS {
        let plan = plan_with_meals(&format!("OTA{level}"), &types, Some(&parent), &["RO", "BB"]);
        parent = post(&app, &owner, &format!("{path}/rate-plans"), plan).await.body;
    }

    let query = json!({
        "query": AVAILABILITY,
        "variables": {"p": path.trim_start_matches("/api/v1/properties/"), "in": day0.to_string(), "out": to},
    });

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES + 20 {
        let started = Instant::now();
        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
        let elapsed = started.elapsed();
        let by_type = response.body["data"]["availability"].as_array();
        assert_eq!(by_type.map(Vec::len), Some(ROOM_TYPES as usize), "{:?}", response.body);
        // 2 meal plans (RO, BB) per rate plan, 5 rate plans in all (BAR and its 3 derived levels, plus CORP).
        assert_eq!(by_type.unwrap()[0]["offers"].as_array().map(Vec::len), Some(2 * (LEVELS as usize + 2)));
        // The first 20 warm the connection pool and Postgres' caches.
        if round >= 20 {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100 - 1];
    println!("availability, {NIGHTS} nights x {ROOM_TYPES} types x 5 plans: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(40), "p95 {p95:?} is over the 40 ms gate");
}
