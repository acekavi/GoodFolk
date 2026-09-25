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
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// Galle with types STD (rooms 101, 102) and DLX (room 201), and room 101 out of order for two days from
/// the business date plus one.
struct Galle {
    owner: String,
    id: String,
    business_date: Date,
    std: Value,
    dlx: Value,
}

impl Galle {
    async fn new(app: &TestApp, superuser: &PgPool) -> Self {
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = property["id"].as_str().unwrap().to_owned();
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let room_type = |code: &str| json!({"code": code, "name": code, "base_occupancy": 2, "max_adults": 2, "max_children": 0, "max_occupancy": 2});
        let std = post(app, &owner, &format!("{path}/room-types"), room_type("STD")).await.body;
        let dlx = post(app, &owner, &format!("{path}/room-types"), room_type("DLX")).await.body;
        let rooms = post(
            app,
            &owner,
            &format!("{path}/rooms/bulk"),
            json!({"room_type_id": std["id"], "first": 101, "last": 102}),
        )
        .await
        .body;
        post(app, &owner, &format!("{path}/rooms"), json!({"room_type_id": dlx["id"], "number": "201", "floor": "2"}))
            .await;
        post(app, &owner, &format!("{path}/sections"), json!({"name": "East"})).await;
        let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'RENOVATION'")
            .fetch_one(superuser)
            .await
            .unwrap();
        let block = json!({"from": (business_date + Duration::days(1)).to_string(),
                           "to": (business_date + Duration::days(3)).to_string(),
                           "kind": "out_of_order", "reason_id": reason});
        let blocked =
            post(app, &owner, &format!("{path}/rooms/{}/blocks", rooms[0]["id"].as_str().unwrap()), block).await;
        assert_eq!(blocked.status, StatusCode::CREATED, "{:?}", blocked.body);
        Self { owner, id, business_date, std, dlx }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_types_rooms_sections_and_reasons_are_listed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $dlx: UUID) {
           roomTypes(propertyId: $p) { code maxOccupancy active version }
           rooms(propertyId: $p) { number floor active }
           dlxRooms: rooms(propertyId: $p, roomTypeId: $dlx) { number }
           sections(propertyId: $p) { name }
           blockReasons(propertyId: $p) { code defaultKind }
         }",
        json!({"p": galle.id, "dlx": galle.dlx["id"]}),
    )
    .await;

    let data = &body["data"];
    assert_eq!(
        data["roomTypes"],
        json!([{"code": "STD", "maxOccupancy": 2, "active": true, "version": 1},
               {"code": "DLX", "maxOccupancy": 2, "active": true, "version": 1}])
    );
    assert_eq!(
        data["rooms"],
        json!([{"number": "101", "floor": null, "active": true}, {"number": "102", "floor": null, "active": true},
               {"number": "201", "floor": "2", "active": true}])
    );
    assert_eq!(data["dlxRooms"], json!([{"number": "201"}]));
    assert_eq!(data["sections"], json!([{"name": "East"}]));
    assert_eq!(data["blockReasons"][0], json!({"code": "CONSTRUCTION", "defaultKind": "OUT_OF_ORDER"}));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_inventory_calendar_counts_rooms_per_type_per_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $from: Date!, $to: Date!) {
           inventory(propertyId: $p, from: $from, to: $to) { date roomTypeId physical sold outOfOrder available }
           blocks(propertyId: $p, from: $from, to: $to) { from to kind note }
         }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(4)}),
    )
    .await;

    let days = body["data"]["inventory"].as_array().unwrap();
    assert_eq!(days.len(), 8, "4 days x 2 room types: {days:?}");
    let std_available: Vec<i64> = days
        .iter()
        .filter(|day| day["roomTypeId"] == galle.std["id"])
        .map(|day| day["available"].as_i64().unwrap())
        .collect();
    assert_eq!(std_available, [2, 1, 1, 2]);
    let dlx = days.iter().find(|day| day["roomTypeId"] == galle.dlx["id"]).unwrap();
    assert_eq!(
        dlx,
        &json!({"date": galle.day(0), "roomTypeId": galle.dlx["id"], "physical": 1, "sold": 0,
                            "outOfOrder": 0, "available": 1})
    );
    assert_eq!(
        body["data"]["blocks"],
        json!([{"from": galle.day(1), "to": galle.day(3), "kind": "OUT_OF_ORDER", "note": ""}])
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_calendar_range_is_limited_to_93_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $from: Date!, $to: Date!) { inventory(propertyId: $p, from: $from, to: $to) { date } }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(94)}),
    )
    .await;

    assert_eq!(body["errors"][0]["message"], "the range must be 1 to 93 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocks_range_is_limited_to_400_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;
    let query = "query ($p: UUID!, $from: Date!, $to: Date!) { blocks(propertyId: $p, from: $from, to: $to) { from } }";

    let longest =
        graphql(&app, &galle.owner, query, json!({"p": galle.id, "from": galle.day(0), "to": galle.day(400)})).await;
    let too_long =
        graphql(&app, &galle.owner, query, json!({"p": galle.id, "from": galle.day(0), "to": galle.day(401)})).await;

    assert_eq!(longest["errors"], Value::Null, "{longest:?}");
    assert_eq!(longest["data"]["blocks"], json!([{"from": galle.day(1)}]));
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 400 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenant_sees_no_rooms_or_inventory(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;
    let stranger = app.signup_owner("stranger@example.com", "Other Hotels").await;

    let body = graphql(
        &app,
        &stranger,
        "query ($p: UUID!, $from: Date!, $to: Date!) {
           roomTypes(propertyId: $p) { id } rooms(propertyId: $p) { id }
           sections(propertyId: $p) { id } blockReasons(propertyId: $p) { id }
           inventory(propertyId: $p, from: $from, to: $to) { date } blocks(propertyId: $p, from: $from, to: $to) { id }
         }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(30)}),
    )
    .await;

    assert_eq!(
        body["data"],
        json!({"roomTypes": [], "rooms": [], "sections": [], "blockReasons": [], "inventory": [], "blocks": []})
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_scoped_staff_see_only_their_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let galle = Galle::new(&app, &superuser).await;
    let kandy = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let kandy = post(&app, &galle.owner, "/api/v1/properties", kandy).await.body["id"].clone();
    let desk = app.staff(&superuser, &galle.owner, "desk@example.com", "front_desk").await;
    // Narrow the tenant-wide grant `staff` gives to Kandy only.
    sqlx::query("update role_grant set property_id = $1 where role = 'front_desk'")
        .bind(common::uuid(&kandy))
        .execute(&superuser)
        .await
        .unwrap();

    let response = app
        .send(
            Method::POST,
            "/graphql",
            Some(&desk),
            Some(json!({"query": "query ($p: UUID!) { roomTypes(propertyId: $p) { id } }", "variables": {"p": galle.id}})),
        )
        .await;

    assert_eq!(response.body["errors"][0]["message"], "you do not have permission for this property");
}
