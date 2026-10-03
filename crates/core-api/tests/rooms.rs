mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

/// Sends a create command with a fresh idempotency key.
async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// An owner with one property; returns the owner's cookie and the property's API path.
async fn hotel(app: &TestApp) -> (String, String) {
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let created = post(app, &owner, "/api/v1/properties", property).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    (owner, format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap()))
}

fn deluxe() -> Value {
    json!({
        "code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2, "max_children": 1, "max_occupancy": 3,
        "bed_config": [{"kind": "king", "count": 1}], "amenities": ["Sea view"],
    })
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_type_is_created_updated_and_reordered(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;

    let created = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await;
    let standard = json!({"code": "STD", "name": "Standard", "base_occupancy": 1, "max_adults": 2,
                          "max_children": 0, "max_occupancy": 2});
    let standard = post(&app, &owner, &format!("{property}/room-types"), standard).await;
    let dlx = format!("{property}/room-types/{}", created.body["id"].as_str().unwrap());
    let updated = patch(&app, &owner, &dlx, 1, json!({"name": "Deluxe Sea View", "amenities": []})).await;
    let order = json!({"ids": [standard.body["id"], created.body["id"]]});
    let reordered = app.send(Method::PUT, &format!("{property}/room-types/order"), Some(&owner), Some(order)).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(created.body["bed_config"], json!([{"kind": "king", "count": 1}]));
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (updated.body["name"].as_str(), updated.body["amenities"].clone()),
        (Some("Deluxe Sea View"), json!([]))
    );
    assert_eq!(reordered.status, StatusCode::NO_CONTENT, "{:?}", reordered.body);
    let codes: Vec<String> =
        sqlx::query_scalar("select code from room_type order by sort_order").fetch_all(&superuser).await.unwrap();
    assert_eq!(codes, ["STD", "DLX"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_type_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let path = format!("{property}/room-types");
    post(&app, &owner, &path, deluxe()).await;

    let duplicate = post(&app, &owner, &path, deluxe()).await;
    let too_many_guests = post(
        &app,
        &owner,
        &path,
        json!({"code": "TWN", "name": "Twin", "base_occupancy": 2,
        "max_adults": 2, "max_children": 0, "max_occupancy": 3}),
    )
    .await;
    let lower_case = post(
        &app,
        &owner,
        &path,
        json!({"code": "dlx", "name": "D", "base_occupancy": 1,
        "max_adults": 1, "max_children": 0, "max_occupancy": 1}),
    )
    .await;
    let unknown_property =
        post(&app, &owner, &format!("/api/v1/properties/{}/room-types", Uuid::now_v7()), deluxe()).await;
    let overbooked = post(
        &app,
        &owner,
        &path,
        json!({"code": "OVR", "name": "Over", "base_occupancy": 1,
        "max_adults": 1, "max_children": 0, "max_occupancy": 1, "overbooking": 21}),
    )
    .await;

    assert_eq!(duplicate.status, StatusCode::CONFLICT, "{:?}", duplicate.body);
    assert_eq!(too_many_guests.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", too_many_guests.body);
    assert_eq!(lower_case.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", lower_case.body);
    assert_eq!(unknown_property.status, StatusCode::NOT_FOUND, "{:?}", unknown_property.body);
    assert_eq!(overbooked.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", overbooked.body);
    assert_eq!(duplicate.headers[header::CONTENT_TYPE], "application/problem+json");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_overbooking_allowance_is_set_and_bounded(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let mut body = deluxe();
    body["overbooking"] = json!(2);
    let created = post(&app, &owner, &format!("{property}/room-types"), body).await;
    let dlx = format!("{property}/room-types/{}", created.body["id"].as_str().unwrap());

    let updated = patch(&app, &owner, &dlx, 1, json!({"overbooking": 5})).await;
    let refused = patch(&app, &owner, &dlx, 2, json!({"overbooking": 21})).await;

    assert_eq!(created.body["overbooking"], json!(2), "{:?}", created.body);
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.body["overbooking"], json!(5), "{:?}", updated.body);
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_are_added_one_at_a_time_or_as_a_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let section = post(&app, &owner, &format!("{property}/sections"), json!({"name": "East"})).await;

    let single = post(
        &app,
        &owner,
        &format!("{property}/rooms"),
        json!({"room_type_id": room_type, "number": "100", "floor": "G", "section_id": section.body["id"]}),
    )
    .await;
    let range = json!({"room_type_id": room_type, "first": 101, "last": 105, "floor": "1"});
    let key = [("x-goodfolk-csrf", "1"), ("idempotency-key", "bulk-rooms-0001")];
    let bulk =
        app.send_with(Method::POST, &format!("{property}/rooms/bulk"), Some(&owner), Some(range.clone()), &key).await;
    let retry =
        app.send_with(Method::POST, &format!("{property}/rooms/bulk"), Some(&owner), Some(range.clone()), &key).await;
    let overlapping = post(
        &app,
        &owner,
        &format!("{property}/rooms/bulk"),
        json!({"room_type_id": room_type, "first": 105, "last": 106}),
    )
    .await;
    let no_key = app
        .send(
            Method::POST,
            &format!("{property}/rooms"),
            Some(&owner),
            Some(json!({"room_type_id": room_type, "number": "200"})),
        )
        .await;

    assert_eq!(section.status, StatusCode::CREATED, "{:?}", section.body);
    assert_eq!(single.status, StatusCode::CREATED, "{:?}", single.body);
    assert_eq!(single.body["section_id"], section.body["id"]);
    assert_eq!(bulk.status, StatusCode::CREATED, "{:?}", bulk.body);
    let numbers: Vec<&str> = bulk.body.as_array().unwrap().iter().map(|r| r["number"].as_str().unwrap()).collect();
    assert_eq!(numbers, ["101", "102", "103", "104", "105"]);
    assert_eq!(retry.body, bulk.body);
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(overlapping.body["detail"], "room 105 already exists");
    assert_eq!(no_key.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_update_needs_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let room = post(
        &app,
        &owner,
        &format!("{property}/rooms"),
        json!({"room_type_id": room_type, "number": "101", "floor": "1"}),
    )
    .await;
    let path = format!("{property}/rooms/{}", room.body["id"].as_str().unwrap());

    let cleared = patch(&app, &owner, &path, 1, json!({"floor": null, "number": "101A"})).await;
    let stale = patch(&app, &owner, &path, 1, json!({"active": false})).await;
    let missing = app.send(Method::PATCH, &path, Some(&owner), Some(json!({"active": false}))).await;

    assert_eq!(cleared.status, StatusCode::OK, "{:?}", cleared.body);
    assert_eq!((cleared.body["floor"].clone(), cleared.body["number"].clone()), (Value::Null, json!("101A")));
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_owners_and_managers_manage_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&superuser, &owner, "hk@example.com", "housekeeping").await;

    let by_manager = post(&app, &manager, &format!("{property}/room-types"), deluxe()).await;
    let by_desk = post(&app, &front_desk, &format!("{property}/sections"), json!({"name": "East"})).await;
    let by_housekeeping = post(&app, &housekeeping, &format!("{property}/room-types"), deluxe()).await;

    assert_eq!(by_manager.status, StatusCode::CREATED, "{:?}", by_manager.body);
    assert_eq!(by_desk.status, StatusCode::FORBIDDEN);
    assert_eq!(by_housekeeping.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_tells_screens_to_refetch_rooms_and_inventory(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let mut events = app.state.events.subscribe();

    post(&app, &owner, &format!("{property}/rooms"), json!({"room_type_id": room_type, "number": "101"})).await;

    let property_id = uuid(&json!(property.trim_start_matches("/api/v1/properties/")));
    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    assert_eq!(event.property_id, Some(property_id));
    assert_eq!(event.keys[0], format!("rooms:{property_id}"));
    // Physical counts change from the business date for the whole 730-day window: 24 to 26 months (24 when a leap
    // day inside the window pulls its last night back before the same month two years on).
    let months = event.keys.iter().filter(|key| key.starts_with(&format!("inventory:{property_id}:"))).count();
    assert!((24..=26).contains(&months), "{:?}", event.keys);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_and_sections_cannot_be_added_to_another_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap().to_owned();
    let room = json!({"room_type_id": room_type, "number": "666"});
    let range = json!({"room_type_id": room_type, "first": 600, "last": 601});

    let into_their_property = post(&app, &intruder, &format!("{property}/rooms"), room.clone()).await;
    let range_into_their_property = post(&app, &intruder, &format!("{property}/rooms/bulk"), range).await;
    let section_in_their_property =
        post(&app, &intruder, &format!("{property}/sections"), json!({"name": "Intruders"})).await;
    let with_their_room_type = post(&app, &intruder, &format!("/api/v1/properties/{own}/rooms"), room).await;

    assert_eq!(into_their_property.status, StatusCode::NOT_FOUND, "{:?}", into_their_property.body);
    assert_eq!(range_into_their_property.status, StatusCode::NOT_FOUND, "{:?}", range_into_their_property.body);
    assert_eq!(section_in_their_property.status, StatusCode::NOT_FOUND, "{:?}", section_in_their_property.body);
    assert_eq!(with_their_room_type.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", with_their_room_type.body);
    let (rooms, sections): (i64, i64) =
        sqlx::query_as("select (select count(*) from room), (select count(*) from housekeeping_section)")
            .fetch_one(&superuser)
            .await
            .unwrap();
    assert_eq!((rooms, sections), (0, 0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_update_that_changes_nothing_is_a_422_and_keeps_the_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let room = post(&app, &owner, &format!("{property}/rooms"), json!({"room_type_id": room_type, "number": "101"}))
        .await
        .body["id"]
        .clone();
    let reason: Uuid =
        sqlx::query_scalar("select id from block_reason where code = 'OTHER'").fetch_one(&superuser).await.unwrap();

    let responses = [
        patch(&app, &owner, &format!("{property}/room-types/{}", room_type.as_str().unwrap()), 1, json!({})).await,
        patch(&app, &owner, &format!("{property}/rooms/{}", room.as_str().unwrap()), 1, json!({})).await,
        patch(&app, &owner, &format!("{property}/block-reasons/{reason}"), 1, json!({"label": null})).await,
    ];

    for response in responses {
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
        assert_eq!(response.body["detail"], "send at least one field to change");
    }
    let versions: (i32, i32, i32) = sqlx::query_as(
        "select (select version from room_type), (select version from room),
                (select version from block_reason where code = 'OTHER')",
    )
    .fetch_one(&superuser)
    .await
    .unwrap();
    assert_eq!(versions, (1, 1, 1));
}
