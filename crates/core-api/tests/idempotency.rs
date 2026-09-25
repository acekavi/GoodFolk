mod common;

use axum::http::{Method, StatusCode, Uri, header};
use common::{TestApp, TestResponse};
use core_api::idempotency::request_hash;
use db::{Scope, TenantId, UserId};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn galle() -> Value {
    json!({"code": "GAL", "name": "Galle Fort Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"})
}

async fn create(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

/// Signs up an owner and returns their cookie, user and tenant.
async fn owner(app: &TestApp) -> (String, UserId, TenantId) {
    let cookie = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let me = app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await.body;
    let user = UserId(Uuid::parse_str(me["user_id"].as_str().unwrap()).unwrap());
    let tenant = TenantId(Uuid::parse_str(me["current_tenant"].as_str().unwrap()).unwrap());
    (cookie, user, tenant)
}

/// Stores an unfinished claim for creating Galle, as a request that is still running (or died) leaves it.
async fn claim(app: &TestApp, user: UserId, tenant: TenantId, key: &str, age_secs: f64) {
    let body = serde_json::to_vec(&galle()).unwrap();
    let hash = request_hash(user, &Method::POST, &Uri::from_static("/api/v1/properties"), &body);
    let mut tx = db::begin(&app.pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(
        "insert into idempotency_key (tenant_id, key, request_hash, created_at)
         values ($1, $2, $3, now() - make_interval(secs => $4))",
    )
    .bind(tenant.0)
    .bind(key)
    .bind(hash)
    .bind(age_secs)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_request_still_in_progress_answers_retries_with_409(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (cookie, user, tenant) = owner(&app).await;
    claim(&app, user, tenant, "key-00000001", 0.0).await;

    let retry = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;

    assert_eq!(retry.status, StatusCode::CONFLICT, "{:?}", retry.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_abandoned_claim_is_taken_over_by_the_next_request(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (cookie, user, tenant) = owner(&app).await;
    claim(&app, user, tenant, "key-00000001", 120.0).await;

    let retry = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
    let replay = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;

    assert_eq!(retry.status, StatusCode::CREATED, "{:?}", retry.body);
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.body, retry.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_same_key_with_a_different_query_is_a_different_request(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (cookie, _, _) = owner(&app).await;

    let first = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
    let other_query = create(&app, &cookie, "/api/v1/properties?dry_run=1", "key-00000001", galle()).await;

    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(other_query.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", other_query.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_server_error_is_not_stored_so_the_request_can_be_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (cookie, _, _) = owner(&app).await;

    sqlx::query("alter table property rename to property_unavailable").execute(&superuser).await.unwrap();
    let failed = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
    let stored: i64 = sqlx::query_scalar("select count(*) from idempotency_key").fetch_one(&superuser).await.unwrap();
    sqlx::query("alter table property_unavailable rename to property").execute(&superuser).await.unwrap();
    let retry = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;

    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(stored, 0);
    assert_eq!(retry.status, StatusCode::CREATED, "{:?}", retry.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_replayed_create_carries_the_etag_of_the_first_response(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (cookie, _, _) = owner(&app).await;

    let first = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
    let replay = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;

    assert_eq!(first.headers[header::ETAG], "\"1\"");
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.headers.get(header::ETAG), first.headers.get(header::ETAG));
}
