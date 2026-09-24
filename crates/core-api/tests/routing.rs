mod common;

use axum::http::{Method, StatusCode, header};
use common::TestApp;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_route_is_a_404_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::GET, "/api/v1/no-such-thing", None, None).await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(response.body["status"], 404);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_known_route_with_the_wrong_method_is_a_405_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::DELETE, "/api/v1/me", None, None).await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(response.body["status"], 405);
}
