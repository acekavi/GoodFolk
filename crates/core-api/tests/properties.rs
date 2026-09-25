mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

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

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_a_property_writes_an_audit_row(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let me = app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body;

    let created = create(&app, &owner, "key-00000001", galle()).await;

    let id = Uuid::parse_str(created.body["id"].as_str().unwrap()).unwrap();
    let (action, entity, actor, data): (String, String, Uuid, Value) = sqlx::query_as(
        "select action, entity, actor_user_id, data from audit_log where entity_id = $1 and action <> 'tenant.created'",
    )
    .bind(id)
    .fetch_one(&superuser)
    .await
    .unwrap();
    assert_eq!(action, "property.created");
    assert_eq!(entity, "property");
    assert_eq!(actor.to_string(), me["user_id"].as_str().unwrap());
    assert_eq!(data, json!({"code": "GAL", "name": "Galle Fort Hotel"}));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn after_switching_tenant_a_create_lands_in_the_new_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    let alice_id = app.send(Method::GET, "/api/v1/me", Some(&alice), None).await.body["user_id"].clone();
    let bobs_tenant = app.send(Method::GET, "/api/v1/me", Some(&bob), None).await.body["current_tenant"].clone();
    let (alice_id, bobs_tenant) =
        (Uuid::parse_str(alice_id.as_str().unwrap()).unwrap(), Uuid::parse_str(bobs_tenant.as_str().unwrap()).unwrap());
    // Staff invitations arrive in Phase 8; until then a second membership is added directly.
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(bobs_tenant)
        .bind(alice_id)
        .execute(&superuser)
        .await
        .unwrap();
    sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, 'owner')")
        .bind(Uuid::now_v7())
        .bind(bobs_tenant)
        .bind(alice_id)
        .execute(&superuser)
        .await
        .unwrap();

    let switched =
        app.send(Method::PUT, "/api/v1/session/tenant", Some(&alice), Some(json!({"tenant_id": bobs_tenant}))).await;
    let created = create(&app, &alice, "key-00000001", galle()).await;

    assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let owner: Uuid =
        sqlx::query_scalar("select tenant_id from property where code = 'GAL'").fetch_one(&superuser).await.unwrap();
    assert_eq!(owner, bobs_tenant);
}

async fn patch(app: &TestApp, cookie: &str, path: &str, if_match: Option<&str>, body: Value) -> TestResponse {
    let mut headers = vec![("x-goodfolk-csrf", "1")];
    if let Some(version) = if_match {
        headers.push(("if-match", version));
    }
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &headers).await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_settings_are_updated_with_if_match(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = create(&app, &owner, "key-00000001", galle()).await;
    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());

    let updated =
        patch(&app, &owner, &path, Some("\"1\""), json!({"check_in_time": "15:00", "name": "Galle Fort"})).await;

    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(created.body["check_in_time"], "14:00");
    assert_eq!(created.body["check_out_time"], "12:00");
    assert_eq!(created.body["business_date"].as_str().unwrap().len(), "2026-09-24".len());
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.headers[header::ETAG], "\"2\"");
    assert_eq!(updated.body["version"], 2);
    assert_eq!(updated.body["check_in_time"], "15:00");
    assert_eq!(updated.body["check_out_time"], "12:00");
    assert_eq!(updated.body["name"], "Galle Fort");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_412_and_a_missing_one_428(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = create(&app, &owner, "key-00000001", galle()).await;
    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
    patch(&app, &owner, &path, Some("\"1\""), json!({"name": "First edit"})).await;

    let stale = patch(&app, &owner, &path, Some("\"1\""), json!({"name": "Second edit"})).await;
    let missing = patch(&app, &owner, &path, None, json!({"name": "Third edit"})).await;
    let malformed = patch(&app, &owner, &path, Some("2"), json!({"name": "Fourth edit"})).await;
    let unknown =
        patch(&app, &owner, &format!("/api/v1/properties/{}", Uuid::now_v7()), Some("\"1\""), json!({"name": "X"}))
            .await;

    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(stale.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{:?}", malformed.body);
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_empty_update_is_a_422_and_keeps_the_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = create(&app, &owner, "key-00000001", galle()).await;
    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());

    let empty = patch(&app, &owner, &path, Some("\"1\""), json!({})).await;
    let then = patch(&app, &owner, &path, Some("\"1\""), json!({"name": "Galle Fort"})).await;

    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty.body);
    assert_eq!(empty.body["detail"], "send at least one field to change");
    assert_eq!(then.status, StatusCode::OK, "{:?}", then.body);
    assert_eq!(then.body["version"], 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_settings_are_validated(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = create(&app, &owner, "key-00000001", galle()).await;
    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());

    let bad_time = patch(&app, &owner, &path, Some("\"1\""), json!({"check_in_time": "25:00"})).await;
    let empty_name = patch(&app, &owner, &path, Some("\"1\""), json!({"name": ""})).await;

    assert_eq!(bad_time.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", bad_time.body);
    assert_eq!(empty_name.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty_name.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_owners_and_managers_change_property_settings(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = create(&app, &owner, "key-00000001", galle()).await;
    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;

    let by_desk = patch(&app, &front_desk, &path, Some("\"1\""), json!({"check_out_time": "11:00"})).await;
    let by_manager = patch(&app, &manager, &path, Some("\"1\""), json!({"check_out_time": "11:00"})).await;

    assert_eq!(by_desk.status, StatusCode::FORBIDDEN);
    assert_eq!(by_manager.status, StatusCode::OK, "{:?}", by_manager.body);
}
