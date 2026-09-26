mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert!(response.status.is_success(), "{path}: {:?}", response.body);
    response
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// A property with a deluxe room type, BAR (USD, standard) and OTA (BAR + 15 %, inheriting restrictions),
/// BAR priced at 100.00 for two adults on the first week, a minimum stay of 2 on the third day and USD
/// breakfast at 15.00 per adult and 7.50 per child.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: String,
    business_date: Date,
    deluxe: Value,
    bar: Value,
    ota: Value,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = property["id"].as_str().unwrap().to_owned();
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let day = |offset: i64| (business_date + Duration::days(offset)).to_string();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"].clone();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "FIT_F",
                         "currency": "USD", "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [deluxe]});
        let bar = post(app, &owner, &format!("{path}/rate-plans"), bar).await.body;
        let ota = json!({"code": "OTA", "name": "Online agents", "kind": "derived", "segment": "OTA",
                         "currency": "USD", "parent_id": bar["id"], "derive_mode": "percent", "derive_value": 1500,
                         "inherit_restrictions": true, "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [deluxe]});
        let ota = post(app, &owner, &format!("{path}/rate-plans"), ota).await.body;
        let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
        let prices: Vec<Value> = (0..7)
            .map(|offset| json!({"room_type_id": deluxe, "date": day(offset), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced =
            app.send(Method::PUT, &format!("{bar_path}/prices"), Some(&owner), Some(json!({"prices": prices}))).await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT);
        let restriction = json!({"from": day(2), "to": day(3), "min_stay": 2});
        let restricted =
            app.send(Method::PUT, &format!("{bar_path}/restrictions"), Some(&owner), Some(restriction)).await;
        assert_eq!(restricted.status, StatusCode::NO_CONTENT);
        let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                               "from": day(0)});
        post(app, &owner, &format!("{path}/meal-supplements"), breakfast).await;
        Self { owner, superuser, id, business_date, deluxe, bar, ota }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }
}

const PLANS: &str = "query ($p: UUID!) {
    ratePlans(propertyId: $p) { code kind segment residency currency depth deriveMode deriveValue inheritRestrictions
                                allowedMealPlans roomTypeIds parentId version }
    mealSupplements(propertyId: $p) { mealPlan currency adultAmount childAmount from to }
    cancellationPolicies(propertyId: $p) { name }
}";

const GRID: &str = "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
    rateGrid(propertyId: $p, ratePlanId: $plan, from: $from, to: $to) {
        prices { roomTypeId date occupancy amount }
        restrictions { date closed minStay maxStay closedToArrival closedToDeparture }
    }
}";

const QUOTE: &str = "query ($p: UUID!, $type: UUID!, $plan: UUID!, $in: Date!, $out: Date!, $residency: Residency!) {
    quote(propertyId: $p, roomTypeId: $type, ratePlanId: $plan, mealPlan: BB, checkIn: $in, checkOut: $out,
          adults: 2, children: 1, residency: $residency) {
        nights { date room meal } total currency restrictionsOk violations { kind date message }
    }
}";

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_supplements_and_the_rate_grid_are_read_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let plans = graphql(&app, &hotel.owner, PLANS, json!({"p": hotel.id})).await;
    let grid = graphql(
        &app,
        &hotel.owner,
        GRID,
        json!({"p": hotel.id, "plan": hotel.ota["id"], "from": hotel.day(0), "to": hotel.day(31)}),
    )
    .await;
    let too_long = graphql(
        &app,
        &hotel.owner,
        GRID,
        json!({"p": hotel.id, "plan": hotel.ota["id"], "from": hotel.day(0), "to": hotel.day(94)}),
    )
    .await;

    let plans = &plans["data"];
    assert_eq!(plans["ratePlans"][0]["code"], "BAR");
    assert_eq!(plans["ratePlans"][0]["segment"], "FIT_F");
    assert_eq!(plans["ratePlans"][0]["residency"], "NON_RESIDENT");
    assert_eq!(plans["ratePlans"][0]["allowedMealPlans"], json!(["RO", "BB"]));
    assert_eq!(plans["ratePlans"][1]["code"], "OTA");
    assert_eq!(plans["ratePlans"][1]["depth"], 1);
    assert_eq!(plans["ratePlans"][1]["deriveMode"], "PERCENT");
    assert_eq!(plans["ratePlans"][1]["parentId"], hotel.bar["id"]);
    assert_eq!(plans["ratePlans"][1]["roomTypeIds"], json!([hotel.deluxe]));
    assert_eq!(
        plans["mealSupplements"],
        json!([{"mealPlan": "BB", "currency": "USD", "adultAmount": 1500,
                                                  "childAmount": 750, "from": hotel.day(0), "to": null}])
    );
    assert_eq!(plans["cancellationPolicies"], json!([]));
    let prices = grid["data"]["rateGrid"]["prices"].as_array().unwrap();
    assert_eq!(prices.len(), 7);
    assert_eq!(prices[0], json!({"roomTypeId": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 11500}));
    assert_eq!(
        grid["data"]["rateGrid"]["restrictions"],
        json!([{"date": hotel.day(2), "closed": false, "minStay": 2, "maxStay": null, "closedToArrival": false,
                "closedToDeparture": false}]),
        "OTA inherits BAR's restrictions"
    );
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 93 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_is_previewed_and_a_stay_quoted_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let preview = "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
        bulkChangePreview(propertyId: $p, ratePlanId: $plan, from: $from, to: $to, mode: PERCENT, value: 1000) {
            total cells { date occupancy before after }
        }
    }";

    let previewed = graphql(
        &app,
        &hotel.owner,
        preview,
        json!({"p": hotel.id, "plan": hotel.bar["id"], "from": hotel.day(0), "to": hotel.day(3)}),
    )
    .await;
    let too_long = graphql(
        &app,
        &hotel.owner,
        preview,
        json!({"p": hotel.id, "plan": hotel.bar["id"], "from": hotel.day(0), "to": hotel.day(400)}),
    )
    .await;
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 366 days");
    let stay = |plan: &Value, from: i64, to: i64, residency: &str| {
        json!({"p": hotel.id, "type": hotel.deluxe, "plan": plan["id"], "in": hotel.day(from), "out": hotel.day(to),
               "residency": residency})
    };
    let quoted = graphql(&app, &hotel.owner, QUOTE, stay(&hotel.ota, 0, 3, "NON_RESIDENT")).await;
    let refused = graphql(&app, &hotel.owner, QUOTE, stay(&hotel.bar, 2, 3, "RESIDENT")).await;

    let preview = &previewed["data"]["bulkChangePreview"];
    assert_eq!(preview["total"], 3);
    assert_eq!(preview["cells"][0], json!({"date": hotel.day(0), "occupancy": 2, "before": 10000, "after": 11000}));
    let quote = &quoted["data"]["quote"];
    assert_eq!(quote["nights"][0], json!({"date": hotel.day(0), "room": 11500, "meal": 3750}));
    assert_eq!((quote["total"].clone(), quote["currency"].clone()), (json!(3 * (11500 + 3750)), json!("USD")));
    assert_eq!(quote["restrictionsOk"], true, "three nights over the third day meet its minimum stay of 2");
    let violations: Vec<&str> = refused["data"]["quote"]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap())
        .collect();
    assert_eq!(violations, ["RESIDENCY", "MIN_STAY"], "a one-night stay over the third day is too short");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_role_reads_rates_and_another_tenant_reads_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let grid = json!({"p": hotel.id, "plan": hotel.bar["id"], "from": hotel.day(0), "to": hotel.day(7)});
    let stay = json!({"p": hotel.id, "type": hotel.deluxe, "plan": hotel.bar["id"], "in": hotel.day(0),
                      "out": hotel.day(2), "residency": "NON_RESIDENT"});

    let by_housekeeping = graphql(&app, &housekeeping, GRID, grid.clone()).await;
    let plans_by_intruder = graphql(&app, &intruder, PLANS, json!({"p": hotel.id})).await;
    let grid_by_intruder = graphql(&app, &intruder, GRID, grid).await;
    let quote_by_intruder = graphql(&app, &intruder, QUOTE, stay).await;

    assert_eq!(by_housekeeping["data"]["rateGrid"]["prices"].as_array().map(Vec::len), Some(7));
    assert_eq!(plans_by_intruder["data"]["ratePlans"], json!([]));
    assert_eq!(plans_by_intruder["data"]["mealSupplements"], json!([]));
    assert_eq!(grid_by_intruder["data"]["rateGrid"], json!({"prices": [], "restrictions": []}));
    assert_eq!(quote_by_intruder["errors"][0]["message"], "rate plan not found");
}
