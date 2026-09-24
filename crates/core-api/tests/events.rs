mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::TestApp;
use core_api::events::{LiveEvent, spawn_listener};
use db::{Event, Scope, TenantId};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

/// Opens the event stream as `cookie` and reads its `ready` event.
async fn open_stream(app: &TestApp, cookie: &str) -> Body {
    let request = Request::builder().uri("/api/v1/events").header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    let mut body = app.router.clone().oneshot(request).await.unwrap().into_body();
    let ready = tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");
    body
}

/// A superuser connection to the maintenance database, so it can act on the test database from outside.
struct Admin {
    conn: PgConnection,
    database: String,
}

impl Admin {
    async fn connect(opts: &PgConnectOptions) -> Self {
        let database = opts.get_database().expect("sqlx::test names the database").to_owned();
        let conn = PgConnection::connect_with(&opts.clone().database("postgres")).await.unwrap();
        Self { conn, database }
    }

    /// Ends the listener's database session, as a network failure or a database restart would.
    async fn terminate_listener(&mut self) {
        let terminated: Vec<bool> = sqlx::query_scalar(
            "select pg_terminate_backend(pid) from pg_stat_activity where datname = $1 and query like 'LISTEN%'",
        )
        .bind(&self.database)
        .fetch_all(&mut self.conn)
        .await
        .unwrap();
        assert_eq!(terminated, vec![true]);
    }

    /// While disallowed, every new connection to the test database fails, as during an outage.
    async fn allow_connections(&mut self, allow: bool) {
        let sql = format!("alter database \"{}\" allow_connections {allow}", self.database);
        sqlx::query(AssertSqlSafe(sql)).execute(&mut self.conn).await.unwrap();
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn creating_a_property_pushes_an_invalidation_to_the_tenants_stream(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
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
    let mut admin = Admin::connect(&opts).await;
    let mut events = app.state.events.subscribe();
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();

    admin.terminate_listener().await;

    let received = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
    assert_eq!(received, LiveEvent::Resync);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_resync_reaches_the_stream_as_a_resync_event(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let mut body = open_stream(&app, &owner).await;

    app.state.events.send(LiveEvent::Resync).unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), "event: resync\ndata: {}\n\n");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_stream_stays_silent(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    let mut alices_stream = open_stream(&app, &alice).await;
    let mut bobs_stream = open_stream(&app, &bob).await;

    let created = app
        .send_with(
            Method::POST,
            "/api/v1/properties",
            Some(&alice),
            Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
            &[("x-goodfolk-csrf", "1"), ("idempotency-key", "key-00000001")],
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let alices = tokio::time::timeout(Duration::from_secs(5), alices_stream.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(alices.into_data().unwrap(), "event: invalidate\ndata: [\"properties\"]\n\n");
    let bobs = tokio::time::timeout(Duration::from_secs(1), bobs_stream.frame()).await;
    assert!(bobs.is_err(), "Bob's stream received {bobs:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_delayed_reconnect_resyncs_again_once_events_flow(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let mut admin = Admin::connect(&opts).await;
    let mut events = app.state.events.subscribe();
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();

    admin.allow_connections(false).await;
    admin.terminate_listener().await;
    let lost = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
    let while_down = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
    admin.allow_connections(true).await;
    let back = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();

    assert_eq!(lost, LiveEvent::Resync);
    assert!(while_down.is_err(), "resynced before the listener was back: {while_down:?}");
    assert_eq!(back, LiveEvent::Resync);
    let tenant = TenantId(Uuid::now_v7());
    let event = Event { tenant_id: tenant, property_id: None, keys: vec!["properties".into()] };
    let mut tx = db::begin(&app.pool, Scope::tenant(tenant)).await.unwrap();
    db::notify(&mut tx, &event).await.unwrap();
    tx.commit().await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
    assert_eq!(received, LiveEvent::Invalidate(event));
}
