mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    post_with_key(app, cookie, path, &key, body).await
}

async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

async fn put(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    app.send(Method::PUT, path, Some(cookie), Some(body)).await
}

/// A property with a deluxe room type (up to 2 adults and a child), set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        Self { owner, superuser, id, path, business_date, deluxe }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn bar(&self) -> Value {
        json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE", "currency": "USD",
               "rounding_step": 100, "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [self.deluxe]})
    }

    fn ota(&self, parent: &Value) -> Value {
        json!({"code": "OTA", "name": "Online agents", "kind": "derived", "segment": "OTA", "currency": "USD",
               "parent_id": parent["id"], "derive_mode": "percent", "derive_value": 1500, "rounding_step": 100,
               "room_type_ids": [self.deluxe]})
    }

    /// `(date, amount)` of a plan's prices for two adults, by date.
    async fn prices(&self, plan: &Value) -> Vec<(Date, i64)> {
        sqlx::query_as("select date, amount from rate_day where rate_plan_id = $1 and occupancy = 2 order by date")
            .bind(uuid(&plan["id"]))
            .fetch_all(&self.superuser)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_prices_and_bulk_changes_over_rest(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);

    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar.body)).await;
    let prices: Vec<Value> = (0..14)
        .map(|day| json!({"room_type_id": hotel.deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
        .collect();
    let bar_path = format!("{plans}/{}", bar.body["id"].as_str().unwrap());
    let priced = put(&app, &hotel.owner, &format!("{bar_path}/prices"), json!({"prices": prices})).await;
    let weekends = json!({"from": hotel.day(0), "to": hotel.day(14), "weekdays": [6, 7],
                          "change": {"mode": "percent", "value": 1000}});
    let bulk_path = format!("{bar_path}/bulk-change");
    let bulk = post_with_key(&app, &hotel.owner, &bulk_path, "bulk-change-0001", weekends.clone()).await;
    let replay = post_with_key(&app, &hotel.owner, &bulk_path, "bulk-change-0001", weekends).await;
    let renamed = patch(&app, &hotel.owner, &bar_path, 1, json!({"name": "Rack", "extra_adult_amount": 2500})).await;

    assert_eq!(bar.status, StatusCode::CREATED, "{:?}", bar.body);
    assert_eq!(bar.headers[header::ETAG], "\"1\"");
    assert_eq!(bar.body["room_type_ids"], json!([hotel.deluxe]));
    assert_eq!(ota.status, StatusCode::CREATED, "{:?}", ota.body);
    assert_eq!((ota.body["depth"].clone(), ota.body["derive_value"].clone()), (json!(1), json!(1500)));
    assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
    assert_eq!(bulk.status, StatusCode::OK, "{:?}", bulk.body);
    assert_eq!(bulk.body["changed"], 4, "two weekends of two days");
    assert_eq!(replay.body, bulk.body, "a retry replays the answer and changes nothing again");
    let weekend = |date: &Date| matches!(date.weekday(), time::Weekday::Saturday | time::Weekday::Sunday);
    for (date, amount) in hotel.prices(&bar.body).await {
        assert_eq!(amount, if weekend(&date) { 11_000 } else { 10_000 }, "BAR on {date}");
    }
    for (date, amount) in hotel.prices(&ota.body).await {
        assert_eq!(amount, if weekend(&date) { 12_700 } else { 11_500 }, "OTA on {date}");
    }
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(
        (renamed.headers[header::ETAG].to_str().unwrap(), renamed.body["name"].as_str()),
        ("\"2\"", Some("Rack"))
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_meal_supplements_and_cancellation_policies_over_rest(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let bar = post(&app, &hotel.owner, &format!("{}/rate-plans", hotel.path), hotel.bar()).await.body;
    let bar_path = format!("{}/rate-plans/{}", hotel.path, bar["id"].as_str().unwrap());

    let restricted = put(
        &app,
        &hotel.owner,
        &format!("{bar_path}/restrictions"),
        json!({"from": hotel.day(0), "to": hotel.day(3), "room_type_ids": [hotel.deluxe], "min_stay": 2,
               "closed_to_arrival": true}),
    )
    .await;
    let cleared = put(
        &app,
        &hotel.owner,
        &format!("{bar_path}/restrictions"),
        json!({"from": hotel.day(0), "to": hotel.day(1), "min_stay": null}),
    )
    .await;
    let supplements = format!("{}/meal-supplements", hotel.path);
    let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                           "from": hotel.day(0)});
    let created = post(&app, &hotel.owner, &supplements, breakfast.clone()).await;
    let overlapping = post(&app, &hotel.owner, &supplements, breakfast).await;
    let supplement_path = format!("{supplements}/{}", created.body["id"].as_str().unwrap());
    let raised =
        patch(&app, &hotel.owner, &supplement_path, 1, json!({"adult_amount": 1800, "to": hotel.day(365)})).await;
    let policy = json!({"name": "Flexible", "rules": [{"days_before_arrival": 2, "penalty": {"kind": "nights", "value": 1}}],
                        "no_show": {"kind": "percent", "value": 10000}});
    let policy = post(&app, &hotel.owner, &format!("{}/cancellation-policies", hotel.path), policy).await;
    let policy_path = format!("{}/cancellation-policies/{}", hotel.path, policy.body["id"].as_str().unwrap());
    let renamed = patch(&app, &hotel.owner, &policy_path, 1, json!({"name": "Flexible 48h"})).await;
    let with_policy =
        patch(&app, &hotel.owner, &bar_path, 1, json!({"cancellation_policy_id": policy.body["id"]})).await;

    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
    assert_eq!(cleared.status, StatusCode::NO_CONTENT, "{:?}", cleared.body);
    let rows: Vec<(Date, Option<i32>, bool)> =
        sqlx::query_as("select date, min_stay, closed_to_arrival from rate_restriction order by date")
            .fetch_all(&hotel.superuser)
            .await
            .unwrap();
    let min_stays: Vec<Option<i32>> = rows.iter().map(|row| row.1).collect();
    assert_eq!(min_stays, [None, Some(2), Some(2)]);
    assert!(rows.iter().all(|row| row.2), "closed to arrival stays when only the minimum stay is cleared");
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["to"], Value::Null);
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(raised.status, StatusCode::OK, "{:?}", raised.body);
    assert_eq!((raised.body["adult_amount"].clone(), raised.body["to"].clone()), (json!(1800), json!(hotel.day(365))));
    assert_eq!(policy.status, StatusCode::CREATED, "{:?}", policy.body);
    assert_eq!(renamed.body["name"], "Flexible 48h");
    assert_eq!(with_policy.body["cancellation_policy_id"], policy.body["id"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);
    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await.body;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar)).await.body;
    let ota_path = format!("{plans}/{}", ota["id"].as_str().unwrap());
    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});

    let mut in_rupees = hotel.ota(&bar);
    in_rupees["code"] = json!("OTA-LKR");
    in_rupees["currency"] = json!("LKR");
    let responses = [
        (post(&app, &hotel.owner, &plans, in_rupees).await, StatusCode::UNPROCESSABLE_ENTITY),
        (post(&app, &hotel.owner, &plans, hotel.bar()).await, StatusCode::CONFLICT),
        (put(&app, &hotel.owner, &format!("{ota_path}/prices"), price.clone()).await, StatusCode::UNPROCESSABLE_ENTITY),
        (patch(&app, &hotel.owner, &ota_path, 7, json!({"name": "X"})).await, StatusCode::PRECONDITION_FAILED),
        (patch(&app, &hotel.owner, &ota_path, 1, json!({})).await, StatusCode::UNPROCESSABLE_ENTITY),
        (
            app.send(Method::POST, &format!("{ota_path}/bulk-change"), Some(&hotel.owner), Some(json!({}))).await,
            StatusCode::BAD_REQUEST,
        ),
        (put(&app, &hotel.owner, &format!("{plans}/{}/prices", Uuid::now_v7()), price).await, StatusCode::NOT_FOUND),
        (
            put(&app, &hotel.owner, &format!("{ota_path}/prices"), json!({"prices": []})).await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ];

    for (index, (response, expected)) in responses.into_iter().enumerate() {
        assert_eq!(response.status, expected, "case {index}: {:?}", response.body);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_owners_and_managers_manage_rates(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let plans = format!("{}/rate-plans", hotel.path);

    let by_manager = post(&app, &manager, &plans, hotel.bar()).await;
    let bar_path = format!("{plans}/{}", by_manager.body["id"].as_str().unwrap());
    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});
    let priced_by_manager = put(&app, &manager, &format!("{bar_path}/prices"), price.clone()).await;
    let bulk = json!({"from": hotel.day(0), "to": hotel.day(7), "change": {"mode": "amount", "value": 100}});
    let restriction = json!({"from": hotel.day(0), "to": hotel.day(1), "closed": true});
    let supplement =
        json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1, "child_amount": 1, "from": hotel.day(0)});
    let policy = json!({"name": "Strict", "rules": [], "no_show": {"kind": "nights", "value": 1}});

    assert_eq!(by_manager.status, StatusCode::CREATED, "{:?}", by_manager.body);
    assert_eq!(priced_by_manager.status, StatusCode::NO_CONTENT, "{:?}", priced_by_manager.body);
    for staff in [&front_desk, &accountant] {
        let refused = [
            post(&app, staff, &plans, hotel.bar()).await,
            put(&app, staff, &format!("{bar_path}/prices"), price.clone()).await,
            post(&app, staff, &format!("{bar_path}/bulk-change"), bulk.clone()).await,
            put(&app, staff, &format!("{bar_path}/restrictions"), restriction.clone()).await,
            post(&app, staff, &format!("{}/meal-supplements", hotel.path), supplement.clone()).await,
            post(&app, staff, &format!("{}/cancellation-policies", hotel.path), policy.clone()).await,
        ];
        for response in refused {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "{:?}", response.body);
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_rates_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let bar = post(&app, &hotel.owner, &format!("{}/rate-plans", hotel.path), hotel.bar()).await.body;
    let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                           "from": hotel.day(0)});
    let supplement = post(&app, &hotel.owner, &format!("{}/meal-supplements", hotel.path), breakfast).await.body;
    let policy = json!({"name": "Flexible", "rules": [], "no_show": {"kind": "nights", "value": 1}});
    let policy = post(&app, &hotel.owner, &format!("{}/cancellation-policies", hotel.path), policy).await.body;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let (plan, supplement, policy) =
        (bar["id"].as_str().unwrap(), supplement["id"].as_str().unwrap(), policy["id"].as_str().unwrap());
    let price = json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 1}]});
    let bulk = json!({"from": hotel.day(0), "to": hotel.day(7), "change": {"mode": "set", "value": 1}});

    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/rate-plans/{plan}"), 1, json!({"name": "X"})).await,
            put(&app, &intruder, &format!("{property}/rate-plans/{plan}/prices"), price.clone()).await,
            post(&app, &intruder, &format!("{property}/rate-plans/{plan}/bulk-change"), bulk.clone()).await,
            put(
                &app,
                &intruder,
                &format!("{property}/rate-plans/{plan}/restrictions"),
                json!({"from": hotel.day(0), "to": hotel.day(1), "closed": true}),
            )
            .await,
            patch(&app, &intruder, &format!("{property}/meal-supplements/{supplement}"), 1, json!({"adult_amount": 1}))
                .await,
            patch(&app, &intruder, &format!("{property}/cancellation-policies/{policy}"), 1, json!({"name": "X"}))
                .await,
        ];
        for response in responses {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}: {:?}", response.body);
        }
    }
    let untouched: (i32, i64, i64, i32, i32) = sqlx::query_as(
        "select (select version from rate_plan), (select count(*) from rate_day), (select count(*) from rate_restriction),
                (select version from meal_supplement), (select version from cancellation_policy)",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(untouched, (1, 0, 0, 1, 1), "nothing of the other tenant changed");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_price_change_tells_screens_to_refetch_each_plans_months(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);
    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await.body;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar)).await.body;
    let mut events = app.state.events.subscribe();

    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});
    put(&app, &hotel.owner, &format!("{plans}/{}/prices", bar["id"].as_str().unwrap()), price).await;

    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(StdDuration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    let month = &hotel.day(0)[..7];
    assert_eq!(event.property_id, Some(hotel.id));
    assert_eq!(
        event.keys,
        [
            format!("rates:{}:{}:{month}", hotel.id, bar["id"].as_str().unwrap()),
            format!("rates:{}:{}:{month}", hotel.id, ota["id"].as_str().unwrap()),
        ]
    );
}
