mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse};
use core_api::persisted;
use core_api::{AppState, router};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

const PROPERTIES: &str = "{ properties { code } }";

fn properties_document() -> (&'static String, &'static String) {
    let (id, document) = persisted::documents()
        .iter()
        .find(|(_, document)| document.text.starts_with("query Properties "))
        .expect("Properties document");
    (id, &document.text)
}

/// An app whose API runs as in production, sharing `app`'s database.
fn production(app: &TestApp) -> TestApp {
    let state =
        AppState::new(app.pool.clone(), true, db::testing::guest_id_keys(), reservations::CheckInPolicy::default());
    TestApp { router: router(state.clone()), state, pool: app.pool.clone() }
}

async fn post(app: &TestApp, cookie: &str, body: Value) -> TestResponse {
    app.send(Method::POST, "/graphql", Some(cookie), Some(body)).await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn production_runs_a_known_document_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = production(&TestApp::new(opts).await);
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (id, _) = properties_document();

    let response = post(&app, &owner, json!({"documentId": id})).await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    assert_eq!(response.body["data"]["properties"], json!([]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn production_refuses_an_unknown_document_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = production(&TestApp::new(opts).await);
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = post(&app, &owner, json!({"documentId": persisted::document_id(PROPERTIES)})).await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{:?}", response.body);
    assert_eq!(response.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
    assert_eq!(response.body["detail"], "PersistedQueryNotFound");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn production_refuses_a_raw_query(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = production(&TestApp::new(opts).await);
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = post(&app, &owner, json!({"query": PROPERTIES})).await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{:?}", response.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn production_refuses_a_query_that_is_not_the_documents_text(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = production(&TestApp::new(opts).await);
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (id, _) = properties_document();

    let response = post(&app, &owner, json!({"documentId": id, "query": PROPERTIES})).await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{:?}", response.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn production_accepts_the_documents_own_text_beside_its_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = production(&TestApp::new(opts).await);
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let (id, text) = properties_document();

    let response = post(&app, &owner, json!({"documentId": id, "query": text})).await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn development_runs_a_raw_query(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = post(&app, &owner, json!({"query": PROPERTIES})).await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    assert_eq!(response.body["data"]["properties"], json!([]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn development_runs_a_query_sent_beside_an_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = post(&app, &owner, json!({"documentId": "sha256:stale", "query": PROPERTIES})).await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
}

#[test]
fn every_persisted_id_is_the_hash_of_its_text() {
    assert!(!persisted::documents().is_empty());
    for (id, document) in persisted::documents() {
        assert_eq!(
            id,
            &persisted::document_id(&document.text),
            "stale persisted-documents.json: run `bun run codegen`"
        );
    }
}
