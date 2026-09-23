mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::TestApp;
use core_api::events::{LiveEvent, spawn_listener};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use tower::ServiceExt;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_a_property_pushes_an_invalidation_to_the_tenants_stream(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let mut listener = PgListener::connect_with(&sqlx::PgPool::connect_with(opts).await.unwrap()).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    spawn_listener(listener, app.state.events.clone());
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let request = Request::builder().uri("/api/v1/events").header(header::COOKIE, &owner).body(Body::empty()).unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream");
    let mut body = response.into_body();
    let ready = tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");

    app.send_with(
        Method::POST,
        "/api/v1/properties",
        Some(&owner),
        Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
        &[("x-goodfolk-csrf", "1"), ("idempotency-key", "key-00000001")],
    )
    .await;

    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert_eq!(text, "event: invalidate\ndata: [\"properties\"]\n\n");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_malformed_query_is_a_400_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = app.send(Method::GET, "/api/v1/events?property=not-a-uuid", Some(&owner), None).await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{:?}", response.body);
    assert_eq!(response.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn losing_the_listener_connection_tells_every_stream_to_resync(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = sqlx::PgPool::connect_with(opts).await.unwrap();
    let mut listener = PgListener::connect_with(&superuser).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    let mut events = app.state.events.subscribe();
    spawn_listener(listener, app.state.events.clone());

    let terminated: Vec<bool> = sqlx::query_scalar(
        "select pg_terminate_backend(pid) from pg_stat_activity
         where datname = current_database() and query like 'LISTEN%'",
    )
    .fetch_all(&superuser)
    .await
    .unwrap();
    assert_eq!(terminated, vec![true]);

    let received = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
    assert_eq!(received, LiveEvent::Resync);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_resync_reaches_the_stream_as_a_resync_event(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let request = Request::builder().uri("/api/v1/events").header(header::COOKIE, &owner).body(Body::empty()).unwrap();
    let mut body = app.router.clone().oneshot(request).await.unwrap().into_body();
    tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();

    app.state.events.send(LiveEvent::Resync).unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), "event: resync\ndata: {}\n\n");
}
