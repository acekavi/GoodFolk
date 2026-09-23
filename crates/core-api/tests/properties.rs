mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn galle() -> Value {
    json!({"code": "GAL", "name": "Galle Fort Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"})
}

async fn create(app: &TestApp, cookie: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(
        Method::POST,
        "/api/v1/properties",
        Some(cookie),
        Some(body),
        &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)],
    )
    .await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_owner_creates_a_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let created = create(&app, &owner, "key-00000001", galle()).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["code"], "GAL");
    assert_eq!(created.body["base_currency"], "LKR");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_codes_are_unique_within_a_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    create(&app, &alice, "key-00000001", galle()).await;

    let same_tenant = create(&app, &alice, "key-00000002", galle()).await;
    let other_tenant = create(&app, &bob, "key-00000001", galle()).await;

    assert_eq!(same_tenant.status, StatusCode::CONFLICT);
    assert_eq!(other_tenant.status, StatusCode::CREATED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn retrying_with_the_same_idempotency_key_replays_the_first_response(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let first = create(&app, &owner, "key-00000001", galle()).await;
    let retry = create(&app, &owner, "key-00000001", galle()).await;
    let kandy = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let reused = create(&app, &owner, "key-00000001", kandy).await;

    assert_eq!(retry.status, StatusCode::CREATED);
    assert_eq!(retry.body, first.body);
    assert_eq!(reused.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_requires_an_idempotency_key_and_valid_fields(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let no_key = app.send(Method::POST, "/api/v1/properties", Some(&owner), Some(galle())).await;
    let bad_code = json!({"code": "g", "name": "G", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let bad_code = create(&app, &owner, "key-00000001", bad_code).await;
    let bad_zone = json!({"code": "GAL", "name": "G", "timezone": "Mars/Base", "base_currency": "LKR"});
    let bad_zone = create(&app, &owner, "key-00000002", bad_zone).await;

    assert_eq!(no_key.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_code.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad_zone.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_replayed_error_keeps_its_problem_json_content_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let bad_zone = json!({"code": "GAL", "name": "G", "timezone": "Mars/Base", "base_currency": "LKR"});

    let first = create(&app, &owner, "key-00000001", bad_zone.clone()).await;
    let retry = create(&app, &owner, "key-00000001", bad_zone).await;

    assert_eq!(first.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(retry.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(retry.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
    assert_eq!(retry.body, first.body);
}
