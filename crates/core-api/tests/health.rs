mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn liveness_and_readiness_report_ok(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    assert_eq!(app.send(Method::GET, "/healthz", None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(Method::GET, "/readyz", None, None).await.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn responses_carry_a_request_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::GET, "/healthz", None, None).await;

    assert!(response.headers.contains_key("x-request-id"));
}
