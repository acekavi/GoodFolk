mod common;

use axum::http::{Method, StatusCode, header};
use common::TestApp;
use core_api::graphql::build_schema;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

async fn create(app: &TestApp, cookie: &str, code: &str) {
    let body =
        json!({"code": code, "name": format!("Hotel {code}"), "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let key = format!("key-create-{code}");
    let response = app
        .send_with(
            Method::POST,
            "/api/v1/properties",
            Some(cookie),
            Some(body),
            &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)],
        )
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
}

async fn list(app: &TestApp, cookie: &str) -> Value {
    let query = json!({"query": "{ properties { code name } }"});
    let response = app.send(Method::POST, "/graphql", Some(cookie), Some(query)).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body["data"]["properties"].clone()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn properties_are_listed_by_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    create(&app, &owner, "KAN").await;
    create(&app, &owner, "GAL").await;

    assert_eq!(
        list(&app, &owner).await,
        json!([{"code": "GAL", "name": "Hotel GAL"}, {"code": "KAN", "name": "Hotel KAN"}])
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenants_never_see_each_others_properties(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;

    create(&app, &alice, "GAL").await;

    assert_eq!(list(&app, &bob).await, json!([]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn graphql_requires_a_session(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::POST, "/graphql", None, Some(json!({"query": "{ properties { code } }"}))).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_malformed_graphql_request_is_a_400_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let response = app.send_raw(Method::POST, "/graphql", Some(&owner), Some("application/json"), "{\"query\":").await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{:?}", response.body);
    assert_eq!(response.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
}

#[tokio::test]
async fn production_hides_the_schema_from_introspection() {
    let query = "{ __schema { queryType { name } } }";

    let development = build_schema(false).execute(query).await.data.into_json().unwrap();
    let production = build_schema(true).execute(query).await.data.into_json().unwrap();

    assert_eq!(development, json!({"__schema": {"queryType": {"name": "Query"}}}));
    assert_eq!(production, json!({"__schema": null}));
}
