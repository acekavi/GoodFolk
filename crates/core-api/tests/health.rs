mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;

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

#[tokio::test]
async fn readiness_reports_503_when_the_database_is_unreachable() {
    let unreachable = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(500))
        .connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
        .unwrap();
    let app = TestApp::with_pool(unreachable);

    assert_eq!(app.send(Method::GET, "/healthz", None, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(Method::GET, "/readyz", None, None).await.status, StatusCode::SERVICE_UNAVAILABLE);
}
