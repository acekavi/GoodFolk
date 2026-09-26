//! Performance gates, timed in-process through the router, so they measure server time (authentication, the
//! queries and JSON) without the network:
//!
//! - Phase 1: one month of `inventory` for a 200-room, 12-type property in under 20 ms at p95.
//! - Phase 2: a 62-day `rateGrid` for 1 plan and 12 room types with 2 occupancies (1488 prices, plus a
//!   restriction per type and day) in under 30 ms at p95; a bulk change of one year of prices for 12 room
//!   types, with two levels of derived plans below it, in under 300 ms (median of ten runs).
//!
//! Ignored by default because debug builds are several times slower. Run them in release mode, one at a time:
//!
//! ```sh
//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
//! ```

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
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
