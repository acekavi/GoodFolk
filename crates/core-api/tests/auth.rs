mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, session_cookie};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_sets_a_secure_session_cookie_and_returns_the_profile(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "owner@example.com", "password": "a long enough password",
                        "display_name": "Owner", "tenant_name": "Lagoon Hotels"})),
        )
        .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let cookie = response.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with("gf_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert_eq!(response.body["email"], "owner@example.com");
    assert_eq!(response.body["tenants"][0]["name"], "Lagoon Hotels");
    assert_eq!(response.body["grants"][0]["role"], "owner");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_rejects_short_passwords_and_duplicate_emails(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;

    let short = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "new@example.com", "password": "short", "display_name": "N", "tenant_name": "B"})),
        )
        .await;
    let duplicate = app
        .send(
            Method::POST,
            "/api/v1/auth/signup",
            None,
            Some(json!({"email": "OWNER@example.com", "password": "a long enough password",
                        "display_name": "N", "tenant_name": "B"})),
        )
        .await;

    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(short.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn login_me_and_logout(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;

    let wrong = app
        .send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "owner@example.com", "password": "not the password"})),
        )
        .await;
    let login = app
        .send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "owner@example.com", "password": "a long enough password"})),
        )
        .await;
    let cookie = session_cookie(&login.headers);
    let me = app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;
    let logout = app.send(Method::POST, "/api/v1/auth/logout", Some(&cookie), None).await;
    let after = app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;

    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(me.body["email"], "owner@example.com");
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn state_changing_requests_need_the_csrf_header(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app
        .send_with(
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"email": "a@example.com", "password": "x"})),
            &[],
        )
        .await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_cannot_switch_into_a_tenant_they_do_not_belong_to(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
    let bobs_tenant = app.send(Method::GET, "/api/v1/me", Some(&bob), None).await.body["current_tenant"].clone();

    let response =
        app.send(Method::PUT, "/api/v1/session/tenant", Some(&alice), Some(json!({"tenant_id": bobs_tenant}))).await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

fn is_problem_json(response: &common::TestResponse) -> bool {
    response.headers.get(header::CONTENT_TYPE).is_some_and(|v| v == "application/problem+json")
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_json_is_a_400_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let syntax = app.send_raw(Method::POST, "/api/v1/auth/login", None, Some("application/json"), "{\"email\":").await;
    let no_content_type = app.send_raw(Method::POST, "/api/v1/auth/login", None, None, "{}").await;

    assert_eq!(syntax.status, StatusCode::BAD_REQUEST, "{:?}", syntax.body);
    assert!(is_problem_json(&syntax), "{:?}", syntax.headers);
    assert_eq!(no_content_type.status, StatusCode::BAD_REQUEST, "{:?}", no_content_type.body);
    assert!(is_problem_json(&no_content_type), "{:?}", no_content_type.headers);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_body_missing_a_field_is_a_422_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::POST, "/api/v1/auth/login", None, Some(json!({"email": "a@example.com"}))).await;

    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
    assert!(is_problem_json(&response), "{:?}", response.headers);
    assert!(response.body["detail"].as_str().unwrap().contains("password"), "{:?}", response.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_session_whose_tenant_membership_was_removed_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = sqlx::PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    sqlx::query("delete from membership").execute(&superuser).await.unwrap();
    let response =
        app.send(Method::POST, "/graphql", Some(&owner), Some(json!({"query": "{ properties { code } }"}))).await;

    assert_eq!(response.status, StatusCode::FORBIDDEN, "{:?}", response.body);
    assert_eq!(response.body["detail"], "no tenant selected");
}

async fn login(app: &TestApp, email: &str, password: &str) -> common::TestResponse {
    app.send(Method::POST, "/api/v1/auth/login", None, Some(json!({"email": email, "password": password}))).await
}

/// Sends `count` sign-ins one after another and returns their statuses.
async fn login_times(app: &TestApp, email: &str, password: &str, count: usize) -> Vec<StatusCode> {
    let mut statuses = Vec::with_capacity(count);
    for _ in 0..count {
        statuses.push(login(app, email, password).await.status);
    }
    statuses
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn five_failed_logins_lock_the_email_until_the_window_passes(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    app.signup_owner("owner@example.com", "A").await;
    app.signup_owner("other@example.com", "B").await;

    let failures: Vec<StatusCode> = login_times(&app, "owner@example.com", "not the password", 5).await;
    let locked = login(&app, "Owner@Example.com", "a long enough password").await;
    let other = login(&app, "other@example.com", "a long enough password").await;
    sqlx::query("update login_failure set at = at - interval '15 minutes'").execute(&superuser).await.unwrap();
    let after_window = login(&app, "owner@example.com", "a long enough password").await;

    assert_eq!(failures, vec![StatusCode::UNAUTHORIZED; 5]);
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", locked.body);
    assert!(is_problem_json(&locked), "{:?}", locked.headers);
    assert_eq!(other.status, StatusCode::OK);
    assert_eq!(after_window.status, StatusCode::OK, "{:?}", after_window.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_throttled_unknown_email_looks_like_a_throttled_account(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;
    login_times(&app, "owner@example.com", "wrong", 5).await;
    login_times(&app, "nobody@example.com", "wrong", 5).await;

    let known = login(&app, "owner@example.com", "wrong").await;
    let unknown = login(&app, "nobody@example.com", "wrong").await;

    assert_eq!(known.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(unknown.status, known.status);
    assert_eq!(unknown.body, known.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_successful_login_clears_earlier_failures(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    app.signup_owner("owner@example.com", "A").await;

    let before = login_times(&app, "owner@example.com", "wrong", 4).await;
    let success = login(&app, "owner@example.com", "a long enough password").await;
    let after = login_times(&app, "owner@example.com", "wrong", 4).await;
    let still_allowed = login(&app, "owner@example.com", "a long enough password").await;

    assert_eq!(before, vec![StatusCode::UNAUTHORIZED; 4]);
    assert_eq!(success.status, StatusCode::OK);
    assert_eq!(after, vec![StatusCode::UNAUTHORIZED; 4]);
    assert_eq!(still_allowed.status, StatusCode::OK);
}
