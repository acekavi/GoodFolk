//! Phase 1 performance gate: one month of `inventory` for a 200-room, 12-type property is answered in
//! under 20 ms at p95. Timed in-process through the router, so it is server time: authentication, the
//! indexed range scan of at most 372 rows, and JSON, without the network.
//!
//! Ignored by default because debug builds are several times slower. Run it in release mode:
//!
//! ```sh
//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
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
