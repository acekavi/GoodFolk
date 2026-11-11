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
async fn reads_that_audit_their_answer_need_the_csrf_header_too(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "A").await;
    let (property, id) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());

    for path in [
        format!("/api/v1/properties/{property}/guest-documents/{id}/link?variant=original"),
        format!("/api/v1/properties/{property}/invoices/{id}/link"),
        format!("/api/v1/properties/{property}/reports/{id}"),
    ] {
        // A cross-site navigation carries the cookie (SameSite=Lax) but cannot add the header.
        let navigated = app.send_with(Method::GET, &path, Some(&owner), None, &[]).await;
        let from_the_app = app.send(Method::GET, &path, Some(&owner), None).await;

        assert_eq!(navigated.status, StatusCode::FORBIDDEN, "{path}: {:?}", navigated.body);
        assert_eq!(navigated.body["detail"], "missing x-goodfolk-csrf header");
        assert_ne!(from_the_app.status, StatusCode::FORBIDDEN, "{path}: {:?}", from_the_app.body);
    }
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

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn concurrent_failed_logins_cannot_exceed_the_limit(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    app.signup_owner("owner@example.com", "A").await;

    let attempts = (0..12).map(|_| login(&app, "owner@example.com", "not the password"));
    let statuses: Vec<StatusCode> =
        futures::future::join_all(attempts).await.into_iter().map(|response| response.status).collect();

    let rejected = statuses.iter().filter(|status| **status == StatusCode::UNAUTHORIZED).count();
    let throttled = statuses.iter().filter(|status| **status == StatusCode::TOO_MANY_REQUESTS).count();
    assert_eq!((rejected, throttled), (5, 7), "{statuses:?}");
    let recorded: i64 = sqlx::query_scalar("select count(*) from login_failure where email = 'owner@example.com'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(recorded, 5);
}

/// Sets `ip`'s count under `scope` to `hits` in the current minute's window and the next two, so the count stands
/// whichever minute the next request lands in.
async fn seed_hits(pool: &PgPool, scope: &str, ip: &str, hits: i32) {
    for minutes in 0..3 {
        sqlx::query(
            "insert into ibe_rate_hit (scope, ip, window_start, hits)
             values ($1, $2::inet, date_trunc('minute', now()) + make_interval(mins => $3), $4)",
        )
        .bind(scope)
        .bind(ip)
        .bind(minutes)
        .bind(hits)
        .execute(pool)
        .await
        .unwrap();
    }
}

fn signup_body(email: &str) -> serde_json::Value {
    json!({"email": email, "password": "a long enough password", "display_name": "Owner", "tenant_name": "Hotel"})
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_21st_sign_in_in_a_minute_from_one_ip_is_429_and_counts_against_no_email(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    app.signup_owner("owner@example.com", "A").await;
    seed_hits(&superuser, "login", "192.0.2.10", 19).await;
    let body = || Some(json!({"email": "owner@example.com", "password": "a long enough password"}));

    let twentieth = app.send_from("192.0.2.10:1000", Method::POST, "/api/v1/auth/login", body()).await;
    let twenty_first = app.send_from("192.0.2.10:1000", Method::POST, "/api/v1/auth/login", body()).await;
    let elsewhere = app.send_from("192.0.2.11:1000", Method::POST, "/api/v1/auth/login", body()).await;

    assert_eq!(twentieth.status, StatusCode::OK, "{:?}", twentieth.body);
    assert_eq!(twenty_first.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", twenty_first.body);
    assert!(twenty_first.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(elsewhere.status, StatusCode::OK, "{:?}", elsewhere.body);
    let failures: i64 = sqlx::query_scalar("select count(*) from login_failure").fetch_one(&superuser).await.unwrap();
    assert_eq!(failures, 0, "an attempt refused by the IP limit is not an attempt on the email");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_6th_sign_up_in_a_minute_from_one_ip_is_429_and_creates_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    seed_hits(&superuser, "signup", "2001:db8:1:2::/64", 4).await;

    let fifth = app
        .send_from("[2001:db8:1:2::1]:1000", Method::POST, "/api/v1/auth/signup", Some(signup_body("a@example.com")))
        .await;
    // The same /64: an IPv6 client counts by its network.
    let sixth = app
        .send_from("[2001:db8:1:2::99]:1000", Method::POST, "/api/v1/auth/signup", Some(signup_body("b@example.com")))
        .await;

    assert_eq!(fifth.status, StatusCode::CREATED, "{:?}", fifth.body);
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", sixth.body);
    assert!(sixth.headers.contains_key(header::RETRY_AFTER), "{:?}", sixth.headers);
    let users: Vec<String> =
        sqlx::query_scalar("select email::text from app_user").fetch_all(&superuser).await.unwrap();
    assert_eq!(users, ["a@example.com"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invitation_links_are_limited_per_ip(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    seed_hits(&superuser, "token", "192.0.2.10", 20).await;
    let token = || Some(json!({"token": "not-a-token"}));

    let preview = app.send_from("192.0.2.10:1000", Method::POST, "/api/v1/invitations/preview", token()).await;
    let accept = app.send_from("192.0.2.10:1000", Method::POST, "/api/v1/invitations/accept", token()).await;
    let elsewhere = app.send_from("192.0.2.11:1000", Method::POST, "/api/v1/invitations/preview", token()).await;

    assert_eq!(preview.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", preview.body);
    assert_eq!(accept.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", accept.body);
    assert_eq!(elsewhere.status, StatusCode::GONE, "{:?}", elsewhere.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn auth_ip_limits_off_counts_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let state = app.state.clone().with_auth_ip_limits(false);
    let off = TestApp { router: core_api::router(state.clone()), state, pool: app.pool.clone() };
    seed_hits(&superuser, "signup", "192.0.2.10", 5).await;

    let signup =
        off.send_from("192.0.2.10:1000", Method::POST, "/api/v1/auth/signup", Some(signup_body("a@example.com"))).await;

    assert_eq!(signup.status, StatusCode::CREATED, "{:?}", signup.body);
    let hits: i32 = sqlx::query_scalar("select max(hits) from ibe_rate_hit where scope = 'signup'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(hits, 5);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn staff_api_answers_are_never_stored_and_public_ones_keep_their_own_caching(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "A").await;

    let me = app.send(Method::GET, "/api/v1/me", Some(&owner), None).await;
    let refused = app.send(Method::GET, "/api/v1/me", None, None).await;
    let graphql =
        app.send(Method::POST, "/graphql", Some(&owner), Some(json!({"query": "{ properties { code } }"}))).await;
    let public = app.send(Method::GET, "/api/public/v1/ibe/NOPE/site", None, None).await;

    for answer in [&me, &refused, &graphql] {
        assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store", "{:?}", answer.body);
    }
    assert_eq!(public.status, StatusCode::NOT_FOUND);
    assert_ne!(public.headers.get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap()), Some("no-store"));
}
