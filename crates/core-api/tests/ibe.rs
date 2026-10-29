//! The booking site's public API: resolving a site by code or custom host, its limits, CORS and caching.

mod common;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use common::{TestApp, uuid};
use core_api::config::IbeConfig;
use core_api::{AppState, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::net::SocketAddr;
use time::{Date, Duration};
use tower::ServiceExt;
use uuid::Uuid;

/// The socket peer of every request unless a test names another.
const PEER: &str = "192.0.2.10:50000";

struct Hotel {
    owner: String,
    superuser: PgPool,
    path: String,
    business_date: Date,
}

impl Hotel {
    /// A property coded `code` with a `DLX` room type and one room, and its booking site saved as `enabled` with the
    /// custom host `stay.galle.example`.
    async fn new(app: &TestApp, opts: PgConnectOptions, code: &str, enabled: bool) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner(&format!("owner@{}.example", code.to_lowercase()), code).await;
        let property = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let created = post(app, &owner, "/api/v1/properties", property).await;
        let path = format!("/api/v1/properties/{}", uuid(&created["id"]));
        let business_date = Date::parse(
            created["business_date"].as_str().unwrap(),
            &time::format_description::well_known::Iso8601::DATE,
        )
        .unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let room_type = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await["id"]);
        post(app, &owner, &format!("{path}/rooms"), json!({"room_type_id": room_type, "number": "101"})).await;
        let hotel = Self { owner, superuser, path, business_date };
        hotel.save_site(app, 0, enabled).await;
        hotel
    }

    async fn save_site(&self, app: &TestApp, version: i64, enabled: bool) {
        let site = json!({"enabled": enabled, "custom_host": "stay.galle.example", "description": "By the sea"});
        let if_match = format!("\"{version}\"");
        let saved = app
            .send_with(
                Method::PUT,
                &format!("{}/ibe-site", self.path),
                Some(&self.owner),
                Some(site),
                &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)],
            )
            .await;
        assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    }

    /// The search path for two adults, residents, for 2 nights from the business date plus `from`.
    fn search(&self, code: &str, from: i64) -> String {
        let (check_in, check_out) =
            (self.business_date + Duration::days(from), self.business_date + Duration::days(from + 2));
        format!(
            "/api/public/v1/ibe/{code}/search?check_in={check_in}&check_out={check_out}&residency=resident&rooms=2-0"
        )
    }
}

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> Value {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
    response.body
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// Sends an anonymous request from the socket peer `peer`, as a browser or the site's server would: no cookie and no
/// CSRF header.
async fn call(app: &TestApp, method: Method, path: &str, peer: &str, headers: &[(&str, &str)]) -> Answer {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let response = app.router.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    Answer { status, headers, body }
}

async fn get(app: &TestApp, path: &str) -> Answer {
    call(app, Method::GET, path, PEER, &[]).await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_disabled_site_is_404_by_code_and_by_custom_host(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;

    let site = get(&app, "/api/public/v1/ibe/GAL/site").await;
    let lowercase = get(&app, "/api/public/v1/ibe/gal/site").await;
    let search = get(&app, &hotel.search("GAL", 1)).await;
    let by_host = get(&app, "/api/public/v1/ibe/by-host/stay.galle.example").await;
    let unknown = get(&app, "/api/public/v1/ibe/XYZ/site").await;
    hotel.save_site(&app, 1, false).await;
    let disabled = [
        get(&app, "/api/public/v1/ibe/GAL/site").await,
        get(&app, &hotel.search("GAL", 1)).await,
        get(&app, "/api/public/v1/ibe/by-host/stay.galle.example").await,
    ];

    assert_eq!(site.status, StatusCode::OK, "{:?}", site.body);
    assert_eq!((&site.body["code"], &site.body["description"]), (&json!("GAL"), &json!("By the sea")));
    assert_eq!(site.body["room_types"][0]["name"], "Deluxe");
    assert_eq!(lowercase.status, StatusCode::OK);
    assert_eq!(search.status, StatusCode::OK, "{:?}", search.body);
    assert_eq!(search.body["room_types"], json!([]), "no IBE plan yet");
    assert_eq!(by_host.status, StatusCode::OK, "{:?}", by_host.body);
    assert_eq!(by_host.body, json!({"code": "GAL"}));
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    for answer in disabled {
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{:?}", answer.body);
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_search_outside_the_limits_is_422_and_a_malformed_one_400(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;

    let past = get(&app, &hotel.search("GAL", -1)).await;
    let rooms = get(&app, &hotel.search("GAL", 1).replace("rooms=2-0", "rooms=2-0,2-0,2-0,2-0,2-0")).await;
    let malformed = get(&app, &hotel.search("GAL", 1).replace("rooms=2-0", "rooms=two")).await;
    let no_residency = get(&app, &hotel.search("GAL", 1).replace("&residency=resident", "")).await;

    assert_eq!(past.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", past.body);
    assert_eq!(rooms.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", rooms.body);
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{:?}", malformed.body);
    assert_eq!(no_residency.status, StatusCode::BAD_REQUEST, "no price before the residency answer");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_61st_search_in_a_minute_from_one_ip_is_429_with_retry_after(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let path = hotel.search("GAL", 1);

    for n in 1..=60 {
        let answer = get(&app, &path).await;
        assert_eq!(answer.status, StatusCode::OK, "search {n}: {:?}", answer.body);
    }
    let refused = get(&app, &path).await;
    let elsewhere = call(&app, Method::GET, &path, "198.51.100.7:40000", &[]).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", refused.body);
    let retry_after: u64 = refused.headers[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(elsewhere.status, StatusCode::OK, "another address has its own count");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cors_allows_the_booking_domain_and_enabled_custom_hosts_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let path = "/api/public/v1/ibe/GAL/site";
    let from = |origin: &'static str| [("origin", origin)];
    let allowed = |answer: &Answer| answer.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).cloned();

    let booking_domain = call(&app, Method::GET, path, PEER, &from("http://localhost:5174")).await;
    let custom_host = call(&app, Method::GET, path, PEER, &from("https://stay.galle.example")).await;
    let other = call(&app, Method::GET, path, PEER, &from("https://evil.example")).await;
    let plain_http = call(&app, Method::GET, path, PEER, &from("http://stay.galle.example")).await;
    let preflight = call(
        &app,
        Method::OPTIONS,
        &hotel.search("GAL", 1),
        PEER,
        &[("origin", "http://localhost:5174"), ("access-control-request-method", "GET")],
    )
    .await;
    hotel.save_site(&app, 1, false).await;
    let disabled_host =
        call(&app, Method::GET, "/api/public/v1/ibe/by-host/x.example", PEER, &from("https://stay.galle.example"))
            .await;

    assert_eq!(allowed(&booking_domain).unwrap(), "http://localhost:5174");
    assert_eq!(allowed(&custom_host).unwrap(), "https://stay.galle.example");
    assert!(allowed(&other).is_none());
    assert!(allowed(&plain_http).is_none());
    assert!(allowed(&disabled_host).is_none());
    assert_eq!(preflight.status, StatusCode::OK);
    assert_eq!(allowed(&preflight).unwrap(), "http://localhost:5174");
    assert!(preflight.headers.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
    assert!(booking_domain.headers.get(header::SET_COOKIE).is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_site_and_the_search_are_cached_at_the_edge(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;

    let site = get(&app, "/api/public/v1/ibe/GAL/site").await;
    let search = get(&app, &hotel.search("GAL", 1)).await;

    assert_eq!(site.headers[header::CACHE_CONTROL], "public, s-maxage=300");
    assert_eq!(search.headers[header::CACHE_CONTROL], "public, s-maxage=30");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cf_connecting_ip_counts_only_when_trusted(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let trusted_state = AppState::new(app.pool.clone(), false, db::testing::guest_id_keys())
        .with_ibe(IbeConfig { trust_cloudflare_ip: true, ..IbeConfig::local() });
    let trusted = TestApp { router: router(trusted_state.clone()), state: trusted_state, pool: app.pool.clone() };
    let path = hotel.search("GAL", 1);
    let cloudflare = [("cf-connecting-ip", "203.0.113.9")];

    let untrusted = call(&app, Method::GET, &path, "192.0.2.20:1000", &cloudflare).await;
    let counted = call(&trusted, Method::GET, &path, "192.0.2.30:1000", &cloudflare).await;
    let without_header = call(&trusted, Method::GET, &path, "192.0.2.40:1000", &[]).await;

    for answer in [&untrusted, &counted, &without_header] {
        assert_eq!(answer.status, StatusCode::OK, "{:?}", answer.body);
    }
    let mut ips: Vec<String> = sqlx::query_scalar("select host(ip) from ibe_rate_hit where scope = 'search'")
        .fetch_all(&hotel.superuser)
        .await
        .unwrap();
    ips.sort();
    assert_eq!(ips, ["192.0.2.20", "192.0.2.40", "203.0.113.9"]);
}
