//! The booking site's public API: resolving a site by code or custom host, its limits, CORS and caching, and booking:
//! the bot checks, idempotency and what a booking writes.

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
use std::sync::{Arc, Mutex, OnceLock};
use time::{Date, Duration};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

/// The socket peer of every request unless a test names another.
const PEER: &str = "192.0.2.10:50000";

struct Hotel {
    owner: String,
    superuser: PgPool,
    path: String,
    /// The site's custom host: `stay.galle.example` for `GAL`, `stay.<code>.example` for any other code.
    host: String,
    business_date: Date,
    deluxe: Uuid,
}

impl Hotel {
    /// A property coded `code` with a `DLX` room type and one room, and its booking site saved as `enabled` with the
    /// custom host (see `host`).
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
        let host = if code == "GAL" {
            "stay.galle.example".to_owned()
        } else {
            format!("stay.{}.example", code.to_lowercase())
        };
        let hotel = Self { owner, superuser, path, host, business_date, deluxe: room_type };
        hotel.save_site(app, 0, enabled).await;
        hotel
    }

    async fn save_site(&self, app: &TestApp, version: i64, enabled: bool) {
        let site = json!({"enabled": enabled, "custom_host": self.host, "description": "By the sea"});
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

    /// An `IBE` plan for anyone in LKR selling `DLX` room only at 10 000 a night for 30 days; its id.
    async fn sell(&self, app: &TestApp) -> String {
        let plan = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                          "currency": "LKR", "allowed_meal_plans": ["RO"], "room_type_ids": [self.deluxe]});
        let plan = uuid(&post(app, &self.owner, &format!("{}/rate-plans", self.path), plan).await["id"]).to_string();
        let prices: Vec<Value> = (0..30)
            .map(|day| {
                json!({"room_type_id": self.deluxe, "date": self.business_date + Duration::days(day), "occupancy": 1,
                       "amount": 10_000})
            })
            .collect();
        let path = format!("{}/rate-plans/{plan}/prices", self.path);
        let priced = app.send(Method::PUT, &path, Some(&self.owner), Some(json!({"prices": prices}))).await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        plan
    }

    /// A booking of one `DLX` room on `plan` for two adults, residents, for 2 nights from D + 1, by `email`, at the
    /// totals the search shows, with the Turnstile token `turnstile`.
    async fn booking(&self, app: &TestApp, plan: &str, email: &str, turnstile: &str) -> Value {
        let search = get(app, &self.search("GAL", 1)).await;
        let total = &search.body["room_types"][0]["plans"][0]["meal_plans"][0]["total"];
        assert!(total.is_i64(), "{:?}", search.body);
        json!({
            "turnstile_token": turnstile,
            "residency": "resident",
            "booker": {"first_name": "Kamala", "last_name": "Perera", "email": email, "phone": "+94 77 123 4567",
                       "country": "LK", "arrival_time": "14:30"},
            "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": plan, "meal_plan": "RO", "adults": 2,
                       "children": 0}],
            "check_in": self.business_date + Duration::days(1),
            "check_out": self.business_date + Duration::days(3),
            "special_requests": "",
            "expected_totals": {"LKR": total},
            "agreed": true,
        })
    }

    async fn reservations(&self) -> i64 {
        sqlx::query_scalar("select count(*) from reservation").fetch_one(&self.superuser).await.unwrap()
    }
}

/// A new token the Turnstile stand-in accepts once.
fn passing() -> String {
    format!("pass-{}", Uuid::now_v7())
}

/// A stand-in for Turnstile's verification endpoint that accepts each [`passing`] token once, as Turnstile spends a
/// token on its first check, and the app verifying against it.
async fn with_turnstile(app: &TestApp) -> TestApp {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    let verify = axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
        let token = body["response"].as_str().unwrap_or_default().to_owned();
        let fresh = seen.lock().unwrap().insert(token.clone());
        axum::Json(json!({"success": fresh && token.starts_with("pass-") && body["secret"].is_string()}))
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, axum::Router::new().route("/siteverify", verify)).await.unwrap() });
    verifying_at(app, &format!("http://{addr}/siteverify"))
}

fn verifying_at(app: &TestApp, url: &str) -> TestApp {
    let state = app.state.clone().with_ibe(IbeConfig { turnstile_url: url.to_owned(), ..IbeConfig::local() });
    TestApp { router: router(state.clone()), state, pool: app.pool.clone() }
}

/// Posts `body` as a booking to the site `GAL` from `peer`, with the `Idempotency-Key` `key` when given.
async fn book(app: &TestApp, body: &Value, key: Option<&str>, peer: &str) -> Answer {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/public/v1/ibe/GAL/bookings")
        .header(header::CONTENT_TYPE, "application/json")
        .header("origin", "http://localhost:5174");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    let mut request = builder.body(Body::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let response = app.router.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    Answer { status, headers, body }
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

/// Sets `ip`'s count under `scope` to `hits` in the current minute's window and the next two, so the count stands
/// whichever minute the next request lands in: a test of a limit does not race the clock.
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

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_61st_search_in_a_minute_from_one_ip_is_429_with_retry_after(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let path = hotel.search("GAL", 1);
    seed_hits(&hotel.superuser, "search", "192.0.2.10", 60).await;
    seed_hits(&hotel.superuser, "search", "198.51.100.8", 59).await;

    let refused = get(&app, &path).await;
    let sixtieth = call(&app, Method::GET, &path, "198.51.100.8:40000", &[]).await;
    let sixty_first = call(&app, Method::GET, &path, "198.51.100.8:40000", &[]).await;
    let elsewhere = call(&app, Method::GET, &path, "198.51.100.7:40000", &[]).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", refused.body);
    let retry_after: u64 = refused.headers[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(sixtieth.status, StatusCode::OK, "the 60th search is allowed: {:?}", sixtieth.body);
    assert_eq!(sixty_first.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(elsewhere.status, StatusCode::OK, "another address has its own count");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cached_reads_allow_any_origin_with_a_star(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let from = |origin: &'static str| [("origin", origin)];
    let allowed = |answer: &Answer| answer.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).cloned();

    let reads = [
        call(&app, Method::GET, "/api/public/v1/ibe/GAL/site", PEER, &from("http://localhost:5174")).await,
        call(&app, Method::GET, &hotel.search("GAL", 1), PEER, &from("https://evil.example")).await,
        call(&app, Method::GET, "/api/public/v1/ibe/by-host/stay.galle.example", PEER, &from("https://x.example"))
            .await,
        call(&app, Method::GET, "/api/public/v1/ibe/XYZ/site", PEER, &from("https://evil.example")).await,
    ];
    let preflight = call(
        &app,
        Method::OPTIONS,
        &hotel.search("GAL", 1),
        PEER,
        &[("origin", "https://evil.example"), ("access-control-request-method", "GET")],
    )
    .await;

    for answer in reads.iter().chain([&preflight]) {
        assert_eq!(allowed(answer).unwrap(), "*", "{:?}", answer.body);
        assert!(answer.headers.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
        assert!(answer.headers.get(header::SET_COOKIE).is_none());
    }
    assert_eq!(preflight.status, StatusCode::OK);
    assert_eq!(reads[3].status, StatusCode::NOT_FOUND, "an error is readable too");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn commands_allow_the_booking_domain_and_enabled_custom_hosts_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let path = hotel.search("GAL", 1);
    let preflight = async |origin: &str| {
        let headers = [
            ("origin", origin),
            ("access-control-request-method", "POST"),
            ("access-control-request-headers", "content-type,idempotency-key"),
        ];
        call(&app, Method::OPTIONS, &path, PEER, &headers).await
    };
    let allowed = |answer: &Answer| answer.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).cloned();

    let booking_domain = preflight("http://localhost:5174").await;
    let custom_host = preflight("https://stay.galle.example").await;
    let other = preflight("https://evil.example").await;
    let plain_http = preflight("http://stay.galle.example").await;
    hotel.save_site(&app, 1, false).await;
    let disabled_host = preflight("https://stay.galle.example").await;

    assert_eq!(booking_domain.status, StatusCode::OK);
    assert_eq!(allowed(&booking_domain).unwrap(), "http://localhost:5174");
    assert!(
        booking_domain.headers[header::ACCESS_CONTROL_ALLOW_METHODS].to_str().unwrap().contains("POST"),
        "{:?}",
        booking_domain.headers
    );
    assert_eq!(allowed(&custom_host).unwrap(), "https://stay.galle.example");
    for refused in [&other, &plain_http, &disabled_host] {
        assert!(allowed(refused).is_none(), "{:?}", refused.headers);
    }
    assert!(booking_domain.headers.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
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

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_is_201_and_writes_its_confirmation_mail_job(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;

    let booked = book(&app, &body, Some("booking-key-0001"), PEER).await;

    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    let confirmation_no = booked.body["confirmation_no"].as_str().unwrap();
    assert!(confirmation_no.starts_with("GAL-"), "{confirmation_no}");
    assert_eq!(booked.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5174");
    let manage_url = booked.body["manage_url"].as_str().unwrap();
    let token =
        manage_url.strip_prefix("http://localhost:5174/GAL/manage?token=").unwrap_or_else(|| panic!("{manage_url}"));
    let (reservation, source, hash): (Uuid, String, Vec<u8>) = sqlx::query_as(
        "select r.id, r.source, t.sha256 from reservation r
         join reservation_access_token t on t.reservation_id = r.id where r.confirmation_no = $1",
    )
    .bind(confirmation_no)
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(source, "ibe");
    assert_eq!(hash, ibe::token_hash(token));
    let jobs: Vec<Value> = sqlx::query_scalar("select payload from outbox where topic = 'mail-send'")
        .fetch_all(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let job: mail::MailJob = serde_json::from_value(jobs[0].clone()).unwrap();
    let tenant: Uuid = sqlx::query_scalar("select tenant_id from reservation where id = $1")
        .bind(reservation)
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(
        job,
        mail::MailJob::IbeConfirmation { tenant_id: tenant, reservation_id: reservation, token: token.into() }
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_retry_with_the_spent_turnstile_token_replays_the_201_without_the_manage_link(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;

    let first = book(&app, &body, Some("booking-key-0001"), PEER).await;
    let retry = book(&app, &body, Some("booking-key-0001"), PEER).await;
    let new_key = book(&app, &body, Some("booking-key-0002"), PEER).await;

    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert!(first.body["manage_url"].is_string(), "{:?}", first.body);
    assert_eq!(retry.status, StatusCode::CREATED, "{:?}", retry.body);
    assert_eq!(retry.body, json!({"confirmation_no": first.body["confirmation_no"]}), "the link is in the email");
    assert_eq!(new_key.status, StatusCode::BAD_REQUEST, "the stand-in refuses a spent token: {:?}", new_key.body);
    assert_eq!(hotel.reservations().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_stored_booking_answer_keeps_no_manage_token(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;

    let first = book(&app, &body, Some("booking-key-0001"), PEER).await;

    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    let manage_url = first.body["manage_url"].as_str().unwrap();
    let token = manage_url.rsplit_once("token=").unwrap().1;
    let stored: Vec<u8> =
        sqlx::query_scalar("select response_body from idempotency_key where key like '%/booking-key-0001'")
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    let stored: Value = serde_json::from_slice(&stored).unwrap();
    assert_eq!(stored, json!({"confirmation_no": first.body["confirmation_no"]}));
    let rows: i64 =
        sqlx::query_scalar("select count(*) from idempotency_key where response_body::text like '%' || $1 || '%'")
            .bind(token)
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_same_idempotency_key_with_another_body_is_409(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;
    let mut other = body.clone();
    other["special_requests"] = json!("A late check-in");

    let first = book(&app, &body, Some("booking-key-0001"), PEER).await;
    // Its token is spent: a 409 here means Turnstile was not asked.
    let reused = book(&app, &other, Some("booking-key-0001"), PEER).await;
    let without_key = book(&app, &other, None, PEER).await;

    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert_eq!(reused.status, StatusCode::CONFLICT, "{:?}", reused.body);
    assert_eq!(without_key.status, StatusCode::BAD_REQUEST, "{:?}", without_key.body);
    assert_eq!(hotel.reservations().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_missing_or_failed_turnstile_token_is_400(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let failing = hotel.booking(&app, &plan, "kamala@example.com", "a-bot-token").await;
    let mut missing = failing.clone();
    missing.as_object_mut().unwrap().remove("turnstile_token");

    let failed = book(&app, &failing, Some("booking-key-0001"), PEER).await;
    let absent = book(&app, &missing, Some("booking-key-0002"), PEER).await;

    for answer in [&failed, &absent] {
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{:?}", answer.body);
    }
    assert_eq!(hotel.reservations().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_refused_turnstile_token_keeps_nothing_under_the_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let refused = hotel.booking(&app, &plan, "kamala@example.com", "a-bot-token").await;
    let mut fresh = refused.clone();
    fresh["turnstile_token"] = json!(passing());

    let first = book(&app, &refused, Some("booking-key-0001"), PEER).await;
    let retry = book(&app, &fresh, Some("booking-key-0001"), PEER).await;

    assert_eq!(first.status, StatusCode::BAD_REQUEST, "{:?}", first.body);
    assert_eq!(retry.status, StatusCode::CREATED, "the key was released: {:?}", retry.body);
    assert_eq!(hotel.reservations().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn turnstile_out_of_reach_is_503_never_a_pass(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    // A port nothing listens on.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let app = verifying_at(&app, &format!("http://{closed}/siteverify"));
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;

    let unreachable = book(&app, &body, Some("booking-key-0001"), PEER).await;

    assert_eq!(unreachable.status, StatusCode::SERVICE_UNAVAILABLE, "{:?}", unreachable.body);
    assert_eq!(hotel.reservations().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_6th_booking_in_a_minute_from_one_ip_is_429(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", "a-bot-token").await;

    seed_hits(&hotel.superuser, "booking", "192.0.2.10", 5).await;
    seed_hits(&hotel.superuser, "booking", "198.51.100.8", 4).await;

    let refused = book(&app, &body, Some("booking-key-0006"), PEER).await;
    let fifth = book(&app, &body, Some("booking-key-0007"), "198.51.100.8:40000").await;
    let sixth = book(&app, &body, Some("booking-key-0008"), "198.51.100.8:40000").await;
    let elsewhere = book(&app, &body, Some("booking-key-0009"), "198.51.100.7:40000").await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", refused.body);
    assert!(refused.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(fifth.status, StatusCode::BAD_REQUEST, "the 5th booking is allowed (and refused by Turnstile)");
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(elsewhere.status, StatusCode::BAD_REQUEST, "another address has its own count");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_honeypot_gets_a_decoy_201_and_nothing_is_kept(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    // Turnstile out of reach: the decoy never asks it.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let app = verifying_at(&app, &format!("http://{closed}/siteverify"));
    let mut body = hotel.booking(&app, &plan, "bot@example.com", "a-bot-token").await;
    body["website"] = json!("https://spam.example");

    let decoy = book(&app, &body, Some("booking-key-0001"), PEER).await;
    let again = book(&app, &body, Some("booking-key-0001"), PEER).await;
    let other_key = book(&app, &body, Some("booking-key-0002"), PEER).await;
    let without_key = book(&app, &body, None, PEER).await;

    assert_eq!(decoy.status, StatusCode::CREATED, "{:?}", decoy.body);
    assert_eq!((again.status, &again.body), (StatusCode::CREATED, &decoy.body), "the same key, the same answer");
    assert_ne!(other_key.body["confirmation_no"], decoy.body["confirmation_no"]);
    assert_eq!(without_key.status, StatusCode::BAD_REQUEST, "as a real booking without a key: {:?}", without_key.body);
    let confirmation_no = decoy.body["confirmation_no"].as_str().unwrap();
    let (code, number) = confirmation_no.split_once('-').unwrap();
    assert_eq!(code, "GAL");
    assert!(number.len() == 6 && number.bytes().all(|b| b.is_ascii_digit()), "{confirmation_no}");
    let manage_url = decoy.body["manage_url"].as_str().unwrap();
    let token =
        manage_url.strip_prefix("http://localhost:5174/GAL/manage?token=").unwrap_or_else(|| panic!("{manage_url}"));
    assert_eq!(token.len(), 43);
    assert_eq!(hotel.reservations().await, 0);
    let kept: (i64, i64, i64) = sqlx::query_as(
        "select (select count(*) from guest), (select count(*) from outbox where topic = 'mail-send'),
                (select count(*) from idempotency_key where key like '%/booking-key-0001')",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(kept, (0, 0, 0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_changed_price_and_a_sold_out_room_are_409_with_their_reason(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let body = hotel.booking(&app, &plan, "kamala@example.com", &passing()).await;
    let total = body["expected_totals"]["LKR"].as_i64().unwrap();
    let mut cheaper = body.clone();
    cheaper["expected_totals"]["LKR"] = json!(total - 100);

    let fresh = |mut body: Value| {
        body["turnstile_token"] = json!(passing());
        body
    };

    let changed = book(&app, &fresh(cheaper), Some("booking-key-0001"), PEER).await;
    let booked = book(&app, &fresh(body.clone()), Some("booking-key-0002"), PEER).await;
    let sold_out = book(&app, &fresh(body), Some("booking-key-0003"), PEER).await;

    assert_eq!(changed.status, StatusCode::CONFLICT, "{:?}", changed.body);
    assert_eq!((&changed.body["reason"], &changed.body["totals"]), (&json!("price_changed"), &json!({"LKR": total})));
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    assert_eq!(sold_out.status, StatusCode::CONFLICT, "{:?}", sold_out.body);
    assert_eq!(sold_out.body["reason"], "sold_out");
    assert_eq!(sold_out.body["detail"], "That room was just booked; here is what is still available");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_foreign_origin_preflight_for_post_bookings_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    Hotel::new(&app, opts, "GAL", true).await;
    let preflight = async |origin: &str| {
        let headers = [
            ("origin", origin),
            ("access-control-request-method", "POST"),
            ("access-control-request-headers", "content-type,idempotency-key"),
        ];
        call(&app, Method::OPTIONS, "/api/public/v1/ibe/GAL/bookings", PEER, &headers).await
    };

    let foreign = preflight("https://evil.example").await;
    let own = preflight("http://localhost:5174").await;

    assert!(foreign.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(), "{:?}", foreign.headers);
    assert_eq!(own.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5174");
    assert!(own.headers.get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
}

/// Posts the JSON `body` to `path` from `peer` as the booking site's server would: no cookie and no CSRF header.
async fn post_public(app: &TestApp, path: &str, peer: &str, body: &str) -> Answer {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("origin", "http://localhost:5174")
        .body(Body::from(body.to_owned()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let response = app.router.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    Answer { status, headers, body }
}

/// Books one room on the site `GAL` through the API (`app` verifies Turnstile); the manage link's token.
async fn booked_token(app: &TestApp, hotel: &Hotel, plan: &str) -> String {
    let body = hotel.booking(app, plan, "kamala@example.com", &passing()).await;
    let booked = book(app, &body, Some(&format!("booking-{}", Uuid::now_v7())), PEER).await;
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    let url = booked.body["manage_url"].as_str().unwrap();
    url.split_once("?token=").unwrap().1.to_owned()
}

fn manage_path(code: &str, token: &str) -> String {
    format!("/api/public/v1/ibe/{code}/bookings/manage?token={token}")
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_manage_page_is_never_cached_and_every_bad_token_is_the_same_410(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts.clone(), "GAL", true).await;
    Hotel::new(&app, opts, "COL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let token = booked_token(&app, &hotel, &plan).await;
    let from = [("origin", "https://reader.example")];

    let page = call(&app, Method::GET, &manage_path("GAL", &token), PEER, &from).await;
    let wrong = call(&app, Method::GET, &manage_path("GAL", "not-a-token"), "192.0.2.11:1", &from).await;
    let elsewhere = call(&app, Method::GET, &manage_path("COL", &token), "192.0.2.11:1", &from).await;
    let none = call(&app, Method::GET, "/api/public/v1/ibe/GAL/bookings/manage", "192.0.2.11:1", &from).await;
    sqlx::query("update reservation_access_token set expires_at = now() - interval '1 second' where sha256 = $1")
        .bind(ibe::token_hash(&token))
        .execute(&hotel.superuser)
        .await
        .unwrap();
    let late = call(&app, Method::GET, &manage_path("GAL", &token), "192.0.2.11:1", &from).await;

    assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
    assert_eq!(page.body["status"], "confirmed");
    assert_eq!(page.body["cancellable"], true);
    assert_eq!(page.body["property_name"], "GAL");
    for internal in ["notes", "created_by", "guest_id", "booker", "token"] {
        assert!(page.body.get(internal).is_none(), "{internal}: {:?}", page.body);
    }
    for answer in [&page, &wrong, &late, &elsewhere, &none] {
        assert_eq!(answer.headers[header::CACHE_CONTROL], "no-store", "{:?}", answer.body);
        assert_eq!(answer.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }
    for answer in [&wrong, &late, &elsewhere, &none] {
        assert_eq!(answer.status, StatusCode::GONE, "{:?}", answer.body);
        assert_eq!(answer.body, wrong.body, "one body for every bad token");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_cancels_online_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;
    let plan = hotel.sell(&app).await;
    let app = with_turnstile(&app).await;
    let token = booked_token(&app, &hotel, &plan).await;
    let path = "/api/public/v1/ibe/GAL/bookings/manage/cancel";

    let wrong = post_public(&app, path, PEER, r#"{"token":"not-a-token"}"#).await;
    let malformed = post_public(&app, path, PEER, "{").await;
    let shapeless = post_public(&app, path, PEER, r#"{"token":5}"#).await;
    let cancelled = post_public(&app, path, PEER, &json!({"token": token}).to_string()).await;
    let again = post_public(&app, path, PEER, &json!({"token": token}).to_string()).await;
    let page = call(&app, Method::GET, &manage_path("GAL", &token), "192.0.2.12:1", &[]).await;

    assert_eq!(wrong.status, StatusCode::GONE);
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(shapeless.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(cancelled.status, StatusCode::OK, "{:?}", cancelled.body);
    assert_eq!(cancelled.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:5174");
    assert_eq!(
        (cancelled.body["status"].as_str(), cancelled.body["cancellable"].as_bool()),
        (Some("cancelled"), Some(false))
    );
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.body["reason"], "already_cancelled");
    assert_eq!(page.body["status"], "cancelled", "the page still shows it");
    let jobs: Vec<Value> = sqlx::query_scalar("select payload from outbox where topic = 'mail-send' order by id")
        .fetch_all(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 2, "the confirmation and one cancellation");
    assert!(jobs[1].get("IbeCancellation").is_some(), "{:?}", jobs[1]);
    assert!(!jobs[1].to_string().contains(&token));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_6th_manage_request_in_a_minute_from_one_ip_is_429(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "GAL", true).await;

    seed_hits(&hotel.superuser, "manage", "192.0.2.10", 5).await;
    seed_hits(&hotel.superuser, "manage", "198.51.100.8", 4).await;

    let refused = call(&app, Method::GET, &manage_path("GAL", "x"), PEER, &[]).await;
    let cancel = post_public(&app, "/api/public/v1/ibe/GAL/bookings/manage/cancel", PEER, r#"{"token":"x"}"#).await;
    let fifth = call(&app, Method::GET, &manage_path("GAL", "x"), "198.51.100.8:40000", &[]).await;
    let sixth = call(&app, Method::GET, &manage_path("GAL", "x"), "198.51.100.8:40000", &[]).await;
    let elsewhere = call(&app, Method::GET, &manage_path("GAL", "x"), "198.51.100.7:40000", &[]).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(refused.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(cancel.status, StatusCode::TOO_MANY_REQUESTS, "views and cancels share the count");
    assert_eq!(fifth.status, StatusCode::GONE, "the 5th request is allowed");
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(elsewhere.status, StatusCode::GONE, "another address has its own count");
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs, once, a global subscriber at `DEBUG`, where the request tracing layer logs each request's URI, recording
/// every test's logs in this binary. A per-test subscriber is unreliable here: tests run in parallel.
fn captured_logs() -> &'static CapturedLogs {
    static LOGS: OnceLock<CapturedLogs> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = CapturedLogs::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).with_writer(logs.clone()).finish(),
        )
        .expect("no other global subscriber in this test binary");
        logs
    })
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_manage_token_in_the_query_stays_out_of_the_logs(_: PgPoolOptions, opts: PgConnectOptions) {
    let logs = captured_logs();
    let app = TestApp::new(opts.clone()).await;
    Hotel::new(&app, opts, "GAL", true).await;
    let (token, marker) = ("s3cr3t-manage-token-9f2c", "marker-query-7d41");

    let manage = call(&app, Method::GET, &manage_path("GAL", token), PEER, &[]).await;
    let slash =
        call(&app, Method::GET, &format!("/api/public/v1/ibe/GAL/bookings/manage/?token={token}"), PEER, &[]).await;
    let cased =
        call(&app, Method::GET, &format!("/api/public/v1/ibe/GAL/Bookings/Manage?Token={token}"), PEER, &[]).await;
    let encoded =
        call(&app, Method::GET, &format!("/api/public/v1/ibe/GAL/bookings/manage?t%6Fken={token}"), PEER, &[]).await;
    let control = call(&app, Method::GET, &format!("/api/public/v1/ibe/GAL/site?x={marker}"), PEER, &[]).await;

    assert_eq!((manage.status, control.status), (StatusCode::GONE, StatusCode::OK));
    assert_eq!(slash.status, StatusCode::NOT_FOUND, "a path no route has");
    assert_eq!(cased.status, StatusCode::NOT_FOUND, "a path no route has");
    assert_eq!(encoded.status, StatusCode::GONE, "a percent-encoded name still reaches the route");
    let text = logs.text();
    assert!(text.contains(marker), "the control query is logged, so requests are: {text}");
    assert!(text.contains("/bookings/manage"), "the manage request is logged, by its path: {text}");
    assert!(!text.contains(token), "the token is in a log line: {text}");
}
