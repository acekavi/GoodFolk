//! The `pos` role: it posts bills and reads the outlets and the in-house stays a till needs, and nothing else.

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use identity::{ALL_PERMISSIONS, Grant, Permission, Role, allows};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn send(app: &TestApp, method: Method, cookie: &str, path: &str, body: Option<Value>) -> TestResponse {
    app.send_with(
        method,
        path,
        Some(cookie),
        body,
        &[("x-goodfolk-csrf", "1"), ("idempotency-key", &Uuid::now_v7().to_string()), ("if-match", "\"1\"")],
    )
    .await
}

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    send(app, Method::POST, cookie, path, Some(body)).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// A property with one checked-in guest (Ada Silva in room 101), an outlet, and the owner's cookie.
struct Hotel {
    owner: String,
    id: Uuid,
    path: String,
    stay: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp) -> Self {
        let owner = app.signup_owner("owner@example.com", "GAL").await;
        let property = json!({"code": "GAL", "name": "GAL", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let today = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 101});
        assert_eq!(post(app, &owner, &format!("{path}/rooms/bulk"), range).await.status, StatusCode::CREATED);
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "LKR", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let prices: Vec<Value> = (0..3)
            .map(|day| {
                json!({"room_type_id": deluxe, "date": (today + Duration::days(day)).to_string(),
                       "occupancy": 2, "amount": 10_000})
            })
            .collect();
        let priced =
            send(app, Method::PUT, &owner, &format!("{path}/rate-plans/{bar}/prices"), Some(json!({"prices": prices})))
                .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
        let guest = post(app, &owner, &format!("{path}/guests"), guest).await.body;
        let booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
            "rooms": [{"room_type_id": deluxe, "rate_plan_id": bar, "meal_plan": "RO",
                       "check_in": today.to_string(), "check_out": (today + Duration::days(2)).to_string(),
                       "adults": 2}]});
        let created = post(app, &owner, &format!("{path}/reservations"), booking).await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        let stay = uuid(&created.body["rooms"][0]["id"]);
        let check_in = format!("{path}/reservation-rooms/{stay}/check-in");
        let checked_in = send(app, Method::POST, &owner, &check_in, None).await;
        assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
        let outlet = post(app, &owner, &format!("{path}/outlets"), json!({"code": "BAR", "name": "Bar"})).await;
        assert_eq!(outlet.status, StatusCode::CREATED, "{:?}", outlet.body);
        Self { owner, id, path, stay }
    }
}

/// A `pos` user posts a bill to a checked-in stay and a settled bill, reads the in-house stays and the outlets, and
/// is refused one route or field from every other group.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_pos_user_posts_bills_and_reads_what_a_till_needs_and_nothing_else(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let hotel = Hotel::new(&app).await;
    let pos = app.staff(&superuser, &hotel.owner, "till@example.com", "pos").await;
    let p = &hotel.path;

    let to_stay = json!({"outlet": "BAR", "bill_no": "B-1", "currency": "LKR",
                         "items": [{"description": "Lime juice", "quantity": 2, "unit_net": 5_000}],
                         "target": {"reservation_room": hotel.stay}});
    let posted = post(&app, &pos, &format!("{p}/pos-bills"), to_stay).await;
    assert_eq!(posted.status, StatusCode::CREATED, "{:?}", posted.body);
    let settled = json!({"outlet": "BAR", "bill_no": "B-2", "currency": "LKR",
                         "items": [{"description": "Tea", "quantity": 1, "unit_net": 1_000}],
                         "target": {"settled": {"method": "cash"}}});
    assert_eq!(post(&app, &pos, &format!("{p}/pos-bills"), settled).await.status, StatusCode::CREATED);

    let in_house = graphql(
        &app,
        &pos,
        "query ($p: UUID!) { posInHouse(propertyId: $p) { reservationRoomId roomNumber guestName } }",
        json!({"p": hotel.id}),
    )
    .await;
    assert!(in_house["errors"].is_null(), "{in_house:?}");
    assert_eq!(
        in_house["data"]["posInHouse"],
        json!([{"reservationRoomId": hotel.stay, "roomNumber": "101", "guestName": "Ada Silva"}])
    );
    let outlets =
        graphql(&app, &pos, "query ($p: UUID!) { outlets(propertyId: $p) { code } }", json!({"p": hotel.id})).await;
    assert!(outlets["errors"].is_null(), "{outlets:?}");
    assert_eq!(outlets["data"]["outlets"], json!([{"code": "BAR"}]));

    // The in-house list is for the till only: a front-desk user, who can see folios, may not read it.
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let refused =
        graphql(&app, &desk, "query ($p: UUID!) { posInHouse(propertyId: $p) { guestName } }", json!({"p": hotel.id}))
            .await;
    assert!(refused["errors"][0]["message"].as_str().unwrap().contains("permission"), "{refused:?}");

    let id = Uuid::now_v7();
    let commands = [
        ("reservations", "reservations", json!({"booker_guest_id": id, "source": "front_desk", "rooms": []})),
        (
            "folio",
            &format!("folios/{id}/charges"),
            json!({"category": "misc", "description": "x", "unit_net": 1, "currency": "LKR"}),
        ),
        (
            "rates",
            "rate-plans",
            json!({"code": "X", "name": "X", "kind": "standard", "segment": "IBE", "currency": "LKR", "room_type_ids": []}),
        ),
        ("rooms", "rooms/bulk", json!({"room_type_id": id, "first": 201, "last": 202})),
        ("housekeeping", "room-issues", json!({"room_id": id, "severity": "low", "description": "x"})),
        ("linen", "linen-items", json!({"code": "SHEET", "name": "Sheet"})),
        ("laundry", "laundry-orders", json!({"reservation_room_id": id, "lines": [{"price_id": id, "quantity": 1}]})),
        ("night audit", "night-audit/close", json!({})),
        (
            "settings",
            "tax-rules",
            json!({"code": "VAT", "name": "VAT", "kind": "tax", "rate_bp": 1800, "applies_to": ["*"],
                   "base": ["net"], "sequence": 1, "effective_from": "2026-01-01"}),
        ),
        ("settings", "outlets", json!({"code": "SPA", "name": "Spa"})),
    ];
    for (group, route, body) in commands {
        let response = post(&app, &pos, &format!("{p}/{route}"), body).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{group} {route}: {:?}", response.body);
    }
    let patched = send(&app, Method::PATCH, &pos, &format!("{p}/tax-settings"), Some(json!({}))).await;
    assert_eq!(patched.status, StatusCode::FORBIDDEN, "{:?}", patched.body);

    for (group, query) in [
        ("reservations", "reservations(propertyId: $p) { __typename }"),
        ("folio", &format!(r#"folios(propertyId: $p, reservationRoomId: "{}") {{ id }}"#, hotel.stay)),
        ("rates", "ratePlans(propertyId: $p) { id }"),
        ("rooms", "rooms(propertyId: $p) { id }"),
        ("night audit", "nightAudit(propertyId: $p) { businessDate }"),
        ("settings", "taxRules(propertyId: $p) { id }"),
    ] {
        let read = graphql(&app, &pos, &format!("query ($p: UUID!) {{ {query} }}"), json!({"p": hotel.id})).await;
        let message = read["errors"][0]["message"].as_str().unwrap_or_default();
        assert!(message.contains("permission"), "{group}: {read:?}");
    }

    // Staff management: the role holds no permission but PosPost, so no staff route can admit it.
    let grants = [Grant { property_id: None, role: Role::Pos }];
    for permission in ALL_PERMISSIONS {
        assert_eq!(allows(&grants, permission, Some(hotel.id)), permission == Permission::PosPost, "{permission:?}");
    }
    assert!(!allows(&grants, Permission::StaffManage, None));
}
