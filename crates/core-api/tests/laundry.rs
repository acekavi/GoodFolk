//! Linen and guest laundry over HTTP: the permission on each route and read, the ops alerts each role sees, replays,
//! stale versions, a delivery charged to the stay and heard on the front desk's stream, and property scoping.

mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use core_api::events::spawn_listener;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use tower::ServiceExt;
use uuid::Uuid;

/// A POST with the CSRF header and `Idempotency-Key: key`.
async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

/// A POST with the CSRF header and a fresh `Idempotency-Key`.
async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    post_with_key(app, cookie, path, &Uuid::now_v7().to_string(), body).await
}

/// A request with `If-Match: "<version>"`.
async fn versioned(
    app: &TestApp,
    method: Method,
    cookie: &str,
    path: &str,
    version: i64,
    body: Option<Value>,
) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(method, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

fn assert_status(response: &TestResponse, status: StatusCode, context: &str) {
    assert_eq!(response.status, status, "{context}: {:?}", response.body);
}

/// A refusal by permission: 403 as a problem document.
fn assert_forbidden(response: &TestResponse, context: &str) {
    assert_status(response, StatusCode::FORBIDDEN, context);
    assert_eq!(response.headers["content-type"], "application/problem+json", "{context}");
}

const STOCK: &str = "query ($p: UUID!) { linenStock(propertyId: $p) { locationId itemId quantity par low } }";
const BATCHES: &str = "query ($p: UUID!) {
    laundryBatches(propertyId: $p) { id status outstanding settled version lines { itemId sent returned outstanding } }
}";
const ORDERS: &str = "query ($p: UUID!) { laundryOrders(propertyId: $p) { id status total version } }";
const ORDERS_FOR_STAY: &str = "query ($p: UUID!, $rr: UUID!) {
    laundryOrdersForStay(propertyId: $p, reservationRoomId: $rr) { id status lines { position name quantity } }
}";
const PRICES: &str = "query ($p: UUID!) { laundryPrices(propertyId: $p) { id itemCode service price } }";
const OPS_ALERTS: &str = "query ($p: UUID!) { opsAlerts(propertyId: $p) { kind message } }";
const FOLIOS: &str = "query ($p: UUID!, $rr: UUID) {
    folios(propertyId: $p, reservationRoomId: $rr) { lines { source category description quantity net } }
}";

/// A property with two deluxe rooms (101, 102) sold on BAR in LKR for 30 nights from the business date, a linen
/// store, a section's pantry, a laundry and a towel item, set up by its owner, and one staff member per role.
struct Hotel {
    owner: String,
    manager: String,
    housekeeping: String,
    supervisor: String,
    front_desk: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    bar: Uuid,
    store: Uuid,
    pantry: Uuid,
    laundry: Uuid,
    towel: Uuid,
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
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        assert_status(&post(app, &owner, &format!("{path}/rooms/bulk"), range).await, StatusCode::CREATED, "rooms");
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "LKR", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let prices: Vec<Value> = (0..30)
            .map(|day| {
                json!({"room_type_id": deluxe, "date": (business_date + Duration::days(day)).to_string(),
                       "occupancy": 2, "amount": 1_000_000})
            })
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{path}/rate-plans/{bar}/prices"),
                Some(&owner),
                Some(json!({"prices": prices})),
            )
            .await;
        assert_status(&priced, StatusCode::NO_CONTENT, "prices");
        let section = uuid(&post(app, &owner, &format!("{path}/sections"), json!({"name": "East"})).await.body["id"]);
        let location =
            |name: &str, kind: &str, section: Option<Uuid>| json!({"name": name, "kind": kind, "section_id": section});
        let mut locations = Vec::new();
        for body in [
            location("Main store", "store", None),
            location("East pantry", "pantry", Some(section)),
            location("Town laundry", "laundry", None),
        ] {
            let created = post(app, &owner, &format!("{path}/linen-locations"), body).await;
            assert_status(&created, StatusCode::CREATED, "location");
            assert_eq!(created.headers["etag"], "\"1\"");
            locations.push(uuid(&created.body["id"]));
        }
        let towel =
            post(app, &owner, &format!("{path}/linen-items"), json!({"code": "BATH_TOWEL", "name": "Towel"})).await;
        assert_status(&towel, StatusCode::CREATED, "item");
        let manager = app.staff(&superuser, &owner, "mgr@example.com", "manager").await;
        let housekeeping = app.staff(&superuser, &owner, "hk@example.com", "housekeeping").await;
        let supervisor = app.staff(&superuser, &owner, "sup@example.com", "hk_supervisor").await;
        let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
        Self {
            owner,
            manager,
            housekeeping,
            supervisor,
            front_desk,
            superuser,
            id,
            path,
            business_date,
            deluxe,
            bar,
            store: locations[0],
            pantry: locations[1],
            laundry: locations[2],
            towel: uuid(&towel.body["id"]),
        }
    }

    fn at(&self, rest: &str) -> String {
        format!("{}/{rest}", self.path)
    }

    async fn movement(&self, app: &TestApp, cookie: &str, body: Value) -> TestResponse {
        post(app, cookie, &self.at("linen-movements"), body).await
    }

    /// Receives `quantity` towels into the store as the owner.
    async fn stock_store(&self, app: &TestApp, quantity: i32) {
        let body =
            json!({"item_id": self.towel, "kind": "receive", "quantity": quantity, "to_location_id": self.store});
        assert_status(&self.movement(app, &self.owner, body).await, StatusCode::CREATED, "receive");
    }

    fn batch_body(&self, quantity: i32) -> Value {
        json!({"from_location_id": self.store, "to_location_id": self.laundry,
               "items": [{"item_id": self.towel, "quantity": quantity}]})
    }

    /// Sends `quantity` towels from the store to the laundry as the owner; returns the batch.
    async fn send_batch(&self, app: &TestApp, quantity: i32) -> Value {
        let sent = post(app, &self.owner, &self.at("laundry-batches"), self.batch_body(quantity)).await;
        assert_status(&sent, StatusCode::CREATED, "send");
        sent.body
    }

    async fn price(&self, app: &TestApp) -> Uuid {
        let body = json!({"item_code": "SHIRT", "service": "wash", "name": "Shirt wash", "price": 50_000});
        let created = post(app, &self.manager, &self.at("laundry-prices"), body).await;
        assert_status(&created, StatusCode::CREATED, "price");
        uuid(&created.body["id"])
    }

    /// Books room 101 for two nights from the business date and checks it in; returns the stay.
    async fn in_house_stay(&self, app: &TestApp) -> Uuid {
        let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
        let guest = post(app, &self.owner, &self.at("guests"), guest).await.body;
        let booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
            "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                       "check_in": self.business_date.to_string(),
                       "check_out": (self.business_date + Duration::days(2)).to_string(), "adults": 2}]});
        let created = post(app, &self.owner, &self.at("reservations"), booking).await;
        assert_status(&created, StatusCode::CREATED, "reservation");
        let stay = uuid(&created.body["rooms"][0]["id"]);
        let checked_in =
            versioned(app, Method::POST, &self.owner, &self.at(&format!("reservation-rooms/{stay}/check-in")), 1, None)
                .await;
        assert_status(&checked_in, StatusCode::OK, "check-in");
        stay
    }

    async fn order(&self, app: &TestApp, cookie: &str, stay: Uuid, price: Uuid) -> TestResponse {
        let body = json!({"reservation_room_id": stay, "lines": [{"price_id": price, "quantity": 2}]});
        post(app, cookie, &self.at("laundry-orders"), body).await
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn linen_routes_follow_their_permissions(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let item = json!({"code": "PILLOW", "name": "Pillow case"});
    let location = json!({"name": "Back store", "kind": "vendor"});
    let par = json!({"items": [{"item_id": hotel.towel, "quantity": 2}]});
    let par_path = hotel.at(&format!("linen-par/{}", hotel.deluxe));
    let item_path = hotel.at(&format!("linen-items/{}", hotel.towel));
    let location_path = hotel.at(&format!("linen-locations/{}", hotel.laundry));

    // Setup needs LinenManage: housekeeping and front desk are refused, the supervisor is not.
    for cookie in [&hotel.housekeeping, &hotel.front_desk] {
        assert_forbidden(&post(&app, cookie, &hotel.at("linen-items"), item.clone()).await, "create item");
        assert_forbidden(&post(&app, cookie, &hotel.at("linen-locations"), location.clone()).await, "create location");
        let rename = Some(json!({"name": "Bath towel"}));
        assert_forbidden(&versioned(&app, Method::PATCH, cookie, &item_path, 1, rename.clone()).await, "update item");
        assert_forbidden(&versioned(&app, Method::PATCH, cookie, &location_path, 1, rename).await, "update location");
        assert_forbidden(&app.send(Method::PUT, &par_path, Some(cookie), Some(par.clone())).await, "par");
    }
    assert_status(&post(&app, &hotel.supervisor, &hotel.at("linen-items"), item).await, StatusCode::CREATED, "item");
    assert_status(
        &post(&app, &hotel.supervisor, &hotel.at("linen-locations"), location).await,
        StatusCode::CREATED,
        "location",
    );
    let renamed =
        versioned(&app, Method::PATCH, &hotel.supervisor, &item_path, 1, Some(json!({"name": "Bath towel"}))).await;
    assert_status(&renamed, StatusCode::OK, "rename item");
    assert_eq!(renamed.headers["etag"], "\"2\"");
    let renamed =
        versioned(&app, Method::PATCH, &hotel.supervisor, &location_path, 1, Some(json!({"name": "City laundry"})))
            .await;
    assert_status(&renamed, StatusCode::OK, "rename location");
    let set = app.send(Method::PUT, &par_path, Some(&hotel.supervisor), Some(par)).await;
    assert_status(&set, StatusCode::OK, "par");
    assert_eq!(set.body, json!([{"room_type_id": hotel.deluxe, "item_id": hotel.towel, "quantity": 2}]));

    // Housekeeping receives and transfers, but a write-off needs LinenManage; front desk moves nothing.
    let receive = json!({"item_id": hotel.towel, "kind": "receive", "quantity": 10, "to_location_id": hotel.store});
    let transfer = json!({"item_id": hotel.towel, "kind": "transfer", "quantity": 2,
                          "from_location_id": hotel.store, "to_location_id": hotel.pantry});
    let write_off = json!({"item_id": hotel.towel, "kind": "write_off", "quantity": 1,
                           "from_location_id": hotel.pantry, "reason": "Torn"});
    assert_forbidden(&hotel.movement(&app, &hotel.front_desk, receive.clone()).await, "desk receive");
    let received = hotel.movement(&app, &hotel.housekeeping, receive).await;
    assert_status(&received, StatusCode::CREATED, "receive");
    assert_eq!(received.body["kind"], "receive");
    assert_status(&hotel.movement(&app, &hotel.housekeeping, transfer).await, StatusCode::CREATED, "transfer");
    assert_forbidden(&hotel.movement(&app, &hotel.housekeeping, write_off.clone()).await, "housekeeping write-off");
    assert_status(&hotel.movement(&app, &hotel.supervisor, write_off).await, StatusCode::CREATED, "write-off");

    // Batches: housekeeping sends, receives and returns; a write-off settlement needs LinenManage.
    assert_forbidden(&post(&app, &hotel.front_desk, &hotel.at("laundry-batches"), hotel.batch_body(3)).await, "send");
    let sent = post(&app, &hotel.housekeeping, &hotel.at("laundry-batches"), hotel.batch_body(3)).await;
    assert_status(&sent, StatusCode::CREATED, "send");
    assert_eq!(sent.headers["etag"], "\"1\"");
    let batch = uuid(&sent.body["id"]);
    let receive_path = hotel.at(&format!("laundry-batches/{batch}/receive"));
    let counted = Some(json!({"items": [{"item_id": hotel.towel, "quantity": 1}]}));
    assert_forbidden(&versioned(&app, Method::POST, &hotel.front_desk, &receive_path, 1, counted.clone()).await, "rcv");
    let received = versioned(&app, Method::POST, &hotel.housekeeping, &receive_path, 1, counted).await;
    assert_status(&received, StatusCode::OK, "receive batch");
    assert_eq!(received.body["status"], "received");
    let settle_path = hotel.at(&format!("laundry-batches/{batch}/settle"));
    let returned = Some(json!({"how": "return", "items": [{"item_id": hotel.towel, "quantity": 1}]}));
    let lost = Some(json!({"how": "write_off", "reason": "Lost", "items": [{"item_id": hotel.towel, "quantity": 1}]}));
    assert_forbidden(&versioned(&app, Method::POST, &hotel.housekeeping, &settle_path, 2, lost.clone()).await, "lost");
    let settled = versioned(&app, Method::POST, &hotel.housekeeping, &settle_path, 2, returned).await;
    assert_status(&settled, StatusCode::OK, "settle return");
    let settled = versioned(&app, Method::POST, &hotel.supervisor, &settle_path, 3, lost).await;
    assert_status(&settled, StatusCode::OK, "settle write-off");
    assert_eq!(settled.headers["etag"], "\"4\"");

    // Reads need LinenView: housekeeping sees the stock and batches, front desk neither.
    let stock = graphql(&app, &hotel.housekeeping, STOCK, json!({"p": hotel.id})).await;
    assert!(stock["errors"].is_null(), "{stock:?}");
    let store = stock["data"]["linenStock"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["locationId"] == json!(hotel.store) && cell["itemId"] == json!(hotel.towel))
        .unwrap()
        .clone();
    assert_eq!(store["quantity"], 10 - 2 - 3 + 1 + 1);
    let batches = graphql(&app, &hotel.housekeeping, BATCHES, json!({"p": hotel.id})).await;
    assert_eq!(batches["data"]["laundryBatches"][0]["settled"], true, "{batches:?}");
    for query in [STOCK, BATCHES] {
        let refused = graphql(&app, &hotel.front_desk, query, json!({"p": hotel.id})).await;
        assert!(!refused["errors"].is_null(), "{refused:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn laundry_routes_follow_their_permissions(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let body = json!({"item_code": "SHIRT", "service": "wash", "name": "Shirt wash", "price": 50_000});
    for cookie in [&hotel.front_desk, &hotel.supervisor, &hotel.housekeeping] {
        assert_forbidden(&post(&app, cookie, &hotel.at("laundry-prices"), body.clone()).await, "create price");
    }
    let price = hotel.price(&app).await;
    let price_path = hotel.at(&format!("laundry-prices/{price}"));
    let change = Some(json!({"price": 60_000}));
    assert_forbidden(&versioned(&app, Method::PATCH, &hotel.front_desk, &price_path, 1, change.clone()).await, "price");
    let changed = versioned(&app, Method::PATCH, &hotel.manager, &price_path, 1, change).await;
    assert_status(&changed, StatusCode::OK, "update price");
    assert_eq!(changed.body["price"], 60_000);

    // Every role with LaundryOrders reads the price list.
    for cookie in [&hotel.front_desk, &hotel.housekeeping, &hotel.supervisor] {
        let prices = graphql(&app, cookie, PRICES, json!({"p": hotel.id})).await;
        assert_eq!(prices["data"]["laundryPrices"][0]["price"], 60_000, "{prices:?}");
    }

    // Front desk takes an order and moves it; housekeeping may too.
    let stay = hotel.in_house_stay(&app).await;
    let taken = hotel.order(&app, &hotel.front_desk, stay, price).await;
    assert_status(&taken, StatusCode::CREATED, "order");
    assert_eq!(taken.headers["etag"], "\"1\"");
    assert_eq!(taken.body["status"], "collected");
    assert_eq!(taken.body["total"], 120_000);
    let order = uuid(&taken.body["id"]);
    let lines = Some(json!({"lines": [{"price_id": price, "quantity": 3}], "note": "Starch"}));
    let edited =
        versioned(&app, Method::PATCH, &hotel.housekeeping, &hotel.at(&format!("laundry-orders/{order}")), 1, lines)
            .await;
    assert_status(&edited, StatusCode::OK, "edit lines");
    assert_eq!(edited.body["note"], "Starch");
    let status_path = hotel.at(&format!("laundry-orders/{order}/status"));
    let moved =
        versioned(&app, Method::POST, &hotel.front_desk, &status_path, 2, Some(json!({"status": "processing"}))).await;
    assert_status(&moved, StatusCode::OK, "processing");
    assert_eq!(moved.body["status"], "processing");
    let skipped =
        versioned(&app, Method::POST, &hotel.front_desk, &status_path, 3, Some(json!({"status": "delivered"}))).await;
    assert_status(&skipped, StatusCode::CONFLICT, "a skipped step");

    let orders = graphql(&app, &hotel.front_desk, ORDERS, json!({"p": hotel.id})).await;
    assert_eq!(orders["data"]["laundryOrders"][0]["status"], "PROCESSING", "{orders:?}");
    let for_stay = graphql(&app, &hotel.front_desk, ORDERS_FOR_STAY, json!({"p": hotel.id, "rr": stay})).await;
    assert_eq!(for_stay["data"]["laundryOrdersForStay"][0]["lines"][0]["quantity"], 3, "{for_stay:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn each_role_sees_only_the_ops_alerts_it_may(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let par = json!({"items": [{"item_id": hotel.towel, "quantity": 2}]});
    let set =
        app.send(Method::PUT, &hotel.at(&format!("linen-par/{}", hotel.deluxe)), Some(&hotel.owner), Some(par)).await;
    assert_status(&set, StatusCode::OK, "par");
    // Two rooms need four towels from the store; one is low.
    hotel.stock_store(&app, 1).await;
    sqlx::query(
        "insert into ops_alert (tenant_id, property_id, kind, message)
         select tenant_id, id, 'drift', 'Counters drifted' from property where id = $1",
    )
    .bind(hotel.id)
    .execute(&hotel.superuser)
    .await
    .unwrap();

    let kinds = |alerts: &Value| -> Vec<String> {
        let mut kinds: Vec<String> = alerts["data"]["opsAlerts"]
            .as_array()
            .unwrap_or_else(|| panic!("{alerts:?}"))
            .iter()
            .map(|alert| alert["kind"].as_str().unwrap().to_owned())
            .collect();
        kinds.sort();
        kinds
    };
    for cookie in [&hotel.owner, &hotel.manager] {
        assert_eq!(kinds(&graphql(&app, cookie, OPS_ALERTS, json!({"p": hotel.id})).await), ["DRIFT", "LINEN"]);
    }
    assert_eq!(kinds(&graphql(&app, &hotel.supervisor, OPS_ALERTS, json!({"p": hotel.id})).await), ["LINEN"]);
    for cookie in [&hotel.housekeeping, &hotel.front_desk] {
        let refused = graphql(&app, cookie, OPS_ALERTS, json!({"p": hotel.id})).await;
        assert!(!refused["errors"].is_null(), "no alerts for this role: {refused:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_replayed_batch_send_creates_one_batch(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.stock_store(&app, 10).await;
    let key = Uuid::now_v7().to_string();
    let first = post_with_key(&app, &hotel.housekeeping, &hotel.at("laundry-batches"), &key, hotel.batch_body(3)).await;
    assert_status(&first, StatusCode::CREATED, "send");
    let replay =
        post_with_key(&app, &hotel.housekeeping, &hotel.at("laundry-batches"), &key, hotel.batch_body(3)).await;
    assert_status(&replay, StatusCode::CREATED, "replay");
    assert_eq!(replay.body["id"], first.body["id"]);

    let batches = graphql(&app, &hotel.owner, BATCHES, json!({"p": hotel.id})).await;
    assert_eq!(batches["data"]["laundryBatches"].as_array().unwrap().len(), 1, "{batches:?}");
    let stock = graphql(&app, &hotel.owner, STOCK, json!({"p": hotel.id})).await;
    let store = stock["data"]["linenStock"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cell| cell["locationId"] == json!(hotel.store))
        .unwrap()
        .clone();
    assert_eq!(store["quantity"], 7, "sent once");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_on_receive_is_412(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.stock_store(&app, 10).await;
    let batch = hotel.send_batch(&app, 3).await;
    let path = hotel.at(&format!("laundry-batches/{}/receive", batch["id"].as_str().unwrap()));
    let counted = Some(json!({"items": [{"item_id": hotel.towel, "quantity": 3}]}));

    let stale = versioned(&app, Method::POST, &hotel.housekeeping, &path, 7, counted.clone()).await;
    assert_status(&stale, StatusCode::PRECONDITION_FAILED, "stale");
    assert_eq!(stale.headers["content-type"], "application/problem+json");
    let missing = app.send(Method::POST, &path, Some(&hotel.housekeeping), counted).await;
    assert_status(&missing, StatusCode::PRECONDITION_REQUIRED, "no If-Match");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_delivery_charges_the_stay_and_reaches_the_front_desks_stream(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone(), Default::default())
        .await
        .unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let price = hotel.price(&app).await;
    let stay = hotel.in_house_stay(&app).await;
    let taken = hotel.order(&app, &hotel.front_desk, stay, price).await;
    assert_status(&taken, StatusCode::CREATED, "order");
    let order = uuid(&taken.body["id"]);
    let status_path = hotel.at(&format!("laundry-orders/{order}/status"));
    for (version, status) in [(1, "processing"), (2, "ready")] {
        let moved =
            versioned(&app, Method::POST, &hotel.housekeeping, &status_path, version, Some(json!({"status": status})))
                .await;
        assert_status(&moved, StatusCode::OK, status);
    }

    let request = axum::http::Request::builder()
        .uri("/api/v1/events")
        .header(axum::http::header::COOKIE, &hotel.front_desk)
        .body(Body::empty())
        .unwrap();
    let mut stream = app.router.clone().oneshot(request).await.unwrap().into_body();
    let ready = tokio::time::timeout(StdDuration::from_secs(1), stream.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");

    let delivered =
        versioned(&app, Method::POST, &hotel.front_desk, &status_path, 3, Some(json!({"status": "delivered"}))).await;
    assert_status(&delivered, StatusCode::OK, "delivered");
    assert_eq!(delivered.body["status"], "delivered");

    let laundry_key = format!("laundry:{}", hotel.id);
    let mut heard = String::new();
    while !(heard.contains("folio:") && heard.contains(&laundry_key)) {
        let frame = tokio::time::timeout(StdDuration::from_secs(5), stream.frame())
            .await
            .unwrap_or_else(|_| panic!("the folio and laundry keys did not both arrive: {heard}"))
            .unwrap()
            .unwrap();
        heard.push_str(std::str::from_utf8(&frame.into_data().unwrap()).unwrap());
    }

    let folio = graphql(&app, &hotel.front_desk, FOLIOS, json!({"p": hotel.id, "rr": stay})).await;
    let lines = folio["data"]["folios"][0]["lines"].as_array().unwrap_or_else(|| panic!("{folio:?}")).clone();
    let laundry: Vec<&Value> = lines.iter().filter(|line| line["source"] == "LAUNDRY").collect();
    assert_eq!(laundry.len(), 1, "{lines:?}");
    assert_eq!(laundry[0]["category"], "laundry");
    assert_eq!(laundry[0]["quantity"], 2);
    assert_eq!(laundry[0]["net"], 100_000);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_propertys_batch_or_order_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let other = json!({"code": "KDY", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let other = uuid(&post(&app, &hotel.owner, "/api/v1/properties", other).await.body["id"]);
    let elsewhere = |rest: String| format!("/api/v1/properties/{other}/{rest}");
    hotel.stock_store(&app, 10).await;
    let batch = hotel.send_batch(&app, 3).await["id"].as_str().unwrap().to_owned();
    let price = hotel.price(&app).await;
    let stay = hotel.in_house_stay(&app).await;
    let order = uuid(&hotel.order(&app, &hotel.owner, stay, price).await.body["id"]);

    let counted = Some(json!({"items": [{"item_id": hotel.towel, "quantity": 3}]}));
    let received =
        versioned(&app, Method::POST, &hotel.owner, &elsewhere(format!("laundry-batches/{batch}/receive")), 1, counted)
            .await;
    assert_status(&received, StatusCode::NOT_FOUND, "batch elsewhere");
    let returned = Some(json!({"how": "return", "items": [{"item_id": hotel.towel, "quantity": 1}]}));
    let settled =
        versioned(&app, Method::POST, &hotel.owner, &elsewhere(format!("laundry-batches/{batch}/settle")), 1, returned)
            .await;
    assert_status(&settled, StatusCode::NOT_FOUND, "settle elsewhere");
    let moved = versioned(
        &app,
        Method::POST,
        &hotel.owner,
        &elsewhere(format!("laundry-orders/{order}/status")),
        1,
        Some(json!({"status": "processing"})),
    )
    .await;
    assert_status(&moved, StatusCode::NOT_FOUND, "order elsewhere");
    let lines = Some(json!({"lines": [{"price_id": price, "quantity": 1}]}));
    let edited =
        versioned(&app, Method::PATCH, &hotel.owner, &elsewhere(format!("laundry-orders/{order}")), 1, lines).await;
    assert_status(&edited, StatusCode::NOT_FOUND, "edit elsewhere");

    // Nor do the other property's reads show them.
    let orders = graphql(&app, &hotel.owner, ORDERS, json!({"p": other})).await;
    assert_eq!(orders["data"]["laundryOrders"], json!([]), "{orders:?}");
    let batches = graphql(&app, &hotel.owner, BATCHES, json!({"p": other})).await;
    assert_eq!(batches["data"]["laundryBatches"], json!([]), "{batches:?}");
}

/// Linen keys reach only linen viewers' streams, and guest laundry keys only those who take orders.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn linen_and_laundry_keys_reach_only_their_holders(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "acc@example.com", "accountant").await;
    let open = |cookie: String| {
        let router = app.router.clone();
        async move {
            let request = axum::http::Request::builder()
                .uri("/api/v1/events")
                .header(axum::http::header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap();
            let mut body = router.oneshot(request).await.unwrap().into_body();
            let ready = tokio::time::timeout(StdDuration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
            assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");
            body
        }
    };
    let mut housekeepers = open(hotel.housekeeping.clone()).await;
    let mut desks = open(hotel.front_desk.clone()).await;
    let mut accountants = open(accountant).await;
    let tenant: Uuid = sqlx::query_scalar("select tenant_id from property where id = $1")
        .bind(hotel.id)
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    let (linen, laundry) = (format!("linen:{}", hotel.id), format!("laundry:{}", hotel.id));
    let event = db::Event {
        tenant_id: db::TenantId(tenant),
        property_id: Some(hotel.id),
        keys: vec![linen.clone(), laundry.clone()],
    };
    app.state.events.send(core_api::events::LiveEvent::Invalidate(event)).unwrap();

    let heard = next(&mut housekeepers).await;
    assert!(heard.contains(&linen) && heard.contains(&laundry), "{heard}");
    let heard = next(&mut desks).await;
    assert!(heard.contains(&laundry) && !heard.contains(&linen), "{heard}");
    let silent = tokio::time::timeout(StdDuration::from_millis(300), accountants.frame()).await;
    assert!(silent.is_err(), "the accountant hears neither: {silent:?}");
}

/// The stream's next frame, as text.
async fn next(body: &mut Body) -> String {
    let frame = tokio::time::timeout(StdDuration::from_secs(2), body.frame()).await.unwrap().unwrap().unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}
