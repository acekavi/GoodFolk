//! The night audit over HTTP: the review's row actions, the close through an in-process jobs-svc, its retry, the
//! permissions on each, the run history and statistics reads, the `audit:` event keys and the audit time setting.

mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use core_api::events::LiveEvent;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

/// A POST with the CSRF header and a fresh `Idempotency-Key`.
async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

/// A request with `If-Match: "<version>"`.
async fn versioned(
    app: &TestApp,
    method: Method,
    cookie: &str,
    path: &str,
    version: i64,
    body: Option<Value>,
) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(method, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const AUDIT_RUNS: &str = "query ($p: UUID!) {
    auditRuns(propertyId: $p, first: 10) {
        id businessDate mode status startedBy attempts error failure steps { step completedAt detail }
    }
}";

const NIGHT_AUDIT: &str = "query ($p: UUID!) {
    nightAudit(propertyId: $p) {
        businessDate blocking
        arrivals { reservationRoomId confirmation guest status total currency noShowPenalty version }
        departures { reservationRoomId departure balances { currency amount } version }
        posBills { outlet totalBase bills { billNo total } }
        bookingBills { nights { reservationRoomId date roomAmount problem alreadyPosted } posted { category lines } }
        payments { method currency count amount baseAmount refunds flagged }
    }
}";

const DAILY_STATS: &str = "query ($p: UUID!, $from: Date!, $to: Date!) {
    dailyStats(propertyId: $p, from: $from, to: $to) {
        businessDate roomsSold roomsAvailable roomRevenue taxes { key amount } payments { key amount }
        breakdown { dimension key roomNights roomRevenue revenue }
    }
}";

const OPS_ALERTS: &str = "query ($p: UUID!) { opsAlerts(propertyId: $p) { kind message raisedAt } }";

/// A property with two deluxe rooms (101, 102) sold on BAR in USD at 100.00 a night for 30 nights from the business
/// date, and its owner. A USD rate of 300 on the business date only when `with_rate`.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions, email: &str, code: &str, with_rate: bool) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner(email, code).await;
        let property = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        assert_eq!(post(app, &owner, &format!("{path}/rooms/bulk"), range).await.status, StatusCode::CREATED);
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel = Self { owner, superuser, id, path, business_date, deluxe, bar };
        let prices: Vec<Value> = (0..30)
            .map(|day| json!({"room_type_id": deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{}/rate-plans/{}/prices", hotel.path, hotel.bar),
                Some(&hotel.owner),
                Some(json!({"prices": prices})),
            )
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        if with_rate {
            hotel.set_usd_rate(app).await;
        }
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    async fn set_usd_rate(&self, app: &TestApp) {
        let path = format!("{}/fx-rates/{}/USD", self.path, self.day(0));
        let set = app.send(Method::PUT, &path, Some(&self.owner), Some(json!({"rate": "300"}))).await;
        assert_eq!(set.status, StatusCode::NO_CONTENT, "{:?}", set.body);
    }

    /// Books one room for `[D + from, D + to)` and returns its stay.
    async fn book(&self, app: &TestApp, from: i64, to: i64) -> Uuid {
        let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
        let guest = post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body;
        let booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
            "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                       "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]});
        let created = post(app, &self.owner, &format!("{}/reservations", self.path), booking).await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        uuid(&created.body["rooms"][0]["id"])
    }

    async fn check_in(&self, app: &TestApp, stay: Uuid) {
        let path = format!("{}/reservation-rooms/{stay}/check-in", self.path);
        let checked_in = versioned(app, Method::POST, &self.owner, &path, 1, None).await;
        assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    }

    fn room_action(&self, stay: Uuid, action: &str) -> String {
        format!("{}/reservation-rooms/{stay}/{action}", self.path)
    }

    async fn close(&self, app: &TestApp, cookie: &str) -> TestResponse {
        app.send(Method::POST, &format!("{}/night-audit/close", self.path), Some(cookie), None).await
    }

    /// Starts the close as the owner and returns its run.
    async fn start_close(&self, app: &TestApp) -> Uuid {
        let started = self.close(app, &self.owner).await;
        assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
        uuid(&started.body["run_id"])
    }

    /// Polls `auditRuns` for up to 60 s until `run` reaches `status`; returns it.
    async fn wait_for(&self, app: &TestApp, run: Uuid, status: &str) -> Value {
        let deadline = tokio::time::Instant::now() + StdDuration::from_secs(60);
        loop {
            let read = graphql(app, &self.owner, AUDIT_RUNS, json!({"p": self.id})).await;
            assert!(read["errors"].is_null(), "{read:?}");
            let found = read["data"]["auditRuns"].as_array().unwrap().iter().find(|r| uuid(&r["id"]) == run).cloned();
            if let Some(found) = found.filter(|found| found["status"] == status) {
                return found;
            }
            assert!(tokio::time::Instant::now() < deadline, "run {run} is not {status}: {read:?}");
            tokio::time::sleep(StdDuration::from_millis(50)).await;
        }
    }

    async fn business_date_now(&self, app: &TestApp) -> String {
        let read = graphql(app, &self.owner, "{ properties { id businessDate auditTime } }", json!({})).await;
        read["data"]["properties"][0]["businessDate"].as_str().unwrap().to_owned()
    }
}

/// The event stream as `cookie`, past its `ready` event.
async fn open_stream(app: &TestApp, cookie: &str) -> Body {
    let request = axum::http::Request::builder()
        .uri("/api/v1/events")
        .header(axum::http::header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let mut body = app.router.clone().oneshot(request).await.unwrap().into_body();
    let ready = tokio::time::timeout(StdDuration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");
    body
}

async fn next(body: &mut Body) -> String {
    let frame = tokio::time::timeout(StdDuration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_route_checks_its_permission(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = common::jobs::app(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", true).await;
    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "books@example.com", "accountant").await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;
    let stay = hotel.book(&app, 0, 2).await;

    for action in ["no-show", "delay-arrival", "extend"] {
        let refused = versioned(&app, Method::POST, &housekeeper, &hotel.room_action(stay, action), 1, None).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{action}: {:?}", refused.body);
    }
    let retry = format!("{}/night-audit/runs/{}/retry", hotel.path, Uuid::now_v7());
    for cookie in [&housekeeper, &accountant] {
        assert_eq!(hotel.close(&app, cookie).await.status, StatusCode::FORBIDDEN);
        assert_eq!(app.send(Method::POST, &retry, Some(cookie), None).await.status, StatusCode::FORBIDDEN);
    }
    for (cookie, allowed) in [(&housekeeper, false), (&accountant, false), (&front_desk, true)] {
        let read = graphql(&app, cookie, NIGHT_AUDIT, json!({"p": hotel.id})).await;
        assert_eq!(read["errors"].is_null(), allowed, "{read:?}");
    }
    for (cookie, allowed) in [(&housekeeper, false), (&accountant, true), (&front_desk, true)] {
        let runs = graphql(&app, cookie, AUDIT_RUNS, json!({"p": hotel.id})).await;
        assert_eq!(runs["errors"].is_null(), allowed, "{runs:?}");
        let stats =
            graphql(&app, cookie, DAILY_STATS, json!({"p": hotel.id, "from": hotel.day(-7), "to": hotel.day(0)})).await;
        assert_eq!(stats["errors"].is_null(), allowed, "{stats:?}");
    }
    for (cookie, allowed) in [(&front_desk, false), (&accountant, false), (&manager, true), (&hotel.owner, true)] {
        let alerts = graphql(&app, cookie, OPS_ALERTS, json!({"p": hotel.id})).await;
        assert_eq!(alerts["errors"].is_null(), allowed, "{alerts:?}");
    }

    // Front desk resolves the arrival and closes the day.
    let no_show = versioned(&app, Method::POST, &front_desk, &hotel.room_action(stay, "no-show"), 1, None).await;
    assert_eq!(no_show.status, StatusCode::OK, "{:?}", no_show.body);
    let started = hotel.close(&app, &front_desk).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{:?}", started.body);
    let run = hotel.wait_for(&app, uuid(&started.body["run_id"]), "COMPLETED").await;
    assert_eq!(run["mode"], "MANUAL");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_review_lists_an_arrival_until_it_is_resolved(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = common::jobs::app(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", true).await;
    let stay = hotel.book(&app, 0, 2).await;

    let review = graphql(&app, &hotel.owner, NIGHT_AUDIT, json!({"p": hotel.id})).await;
    assert!(review["errors"].is_null(), "{review:?}");
    let review = &review["data"]["nightAudit"];
    assert_eq!((review["businessDate"].as_str(), review["blocking"].as_i64()), (Some(&*hotel.day(0)), Some(1)));
    assert_eq!(review["arrivals"][0]["reservationRoomId"], json!(stay));
    assert_eq!(
        (review["arrivals"][0]["status"].as_str(), review["arrivals"][0]["total"].as_i64()),
        (Some("CONFIRMED"), Some(20_000))
    );

    let refused = hotel.close(&app, &hotel.owner).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert_eq!(refused.body["detail"], "Resolve every arrival and departure first (1 left)");

    let stale = versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(stay, "no-show"), 9, None).await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    let missing = app.send(Method::POST, &hotel.room_action(stay, "no-show"), Some(&hotel.owner), None).await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);

    let delayed = versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(stay, "delay-arrival"), 1, None).await;
    assert_eq!(delayed.status, StatusCode::OK, "{:?}", delayed.body);
    assert_eq!(
        (delayed.body["check_in"].as_str(), delayed.body["check_out"].as_str()),
        (Some(&*hotel.day(1)), Some(&*hotel.day(2)))
    );
    assert_eq!(delayed.headers["etag"], "\"2\"");
    let review = graphql(&app, &hotel.owner, NIGHT_AUDIT, json!({"p": hotel.id})).await;
    assert_eq!(review["data"]["nightAudit"]["blocking"], 0, "{review:?}");

    // A one-night stay cannot arrive later; marked a no-show, it is resolved.
    let one_night = hotel.book(&app, 0, 1).await;
    let refused =
        versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(one_night, "delay-arrival"), 1, None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert_eq!(refused.body["detail"], "Change the dates with Modify");
    let no_show = versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(one_night, "no-show"), 1, None).await;
    assert_eq!(no_show.status, StatusCode::OK, "{:?}", no_show.body);
    assert_eq!((no_show.body["id"].clone(), no_show.body["currency"].as_str()), (json!(one_night), Some("USD")));
    let again = versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(one_night, "no-show"), 2, None).await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);
    let audited: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action in ('reservation_room.no_show', 'reservation_room.arrival_delayed')",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(audited, 2);
}

/// The close through jobs-svc: 202 at once, then `auditRuns` shows it completed, and the business date is D + 1. A
/// stay due out then is extended a night.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_close_runs_through_jobs_svc_and_moves_the_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = common::jobs::app(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", true).await;
    let stay = hotel.book(&app, 0, 1).await;
    hotel.check_in(&app, stay).await;

    let run = hotel.start_close(&app).await;
    let completed = hotel.wait_for(&app, run, "COMPLETED").await;
    assert_eq!(completed["businessDate"], hotel.day(0));
    let steps: Vec<&str> = completed["steps"].as_array().unwrap().iter().map(|s| s["step"].as_str().unwrap()).collect();
    assert_eq!(steps, ["RESOLVE", "POST_CHARGES", "LOCK_FX", "SNAPSHOT_STATS", "ADVANCE_BUSINESS_DATE"]);
    assert_eq!(hotel.business_date_now(&app).await, hotel.day(1));
    let stats =
        graphql(&app, &hotel.owner, DAILY_STATS, json!({"p": hotel.id, "from": hotel.day(0), "to": hotel.day(1)}))
            .await;
    assert!(stats["errors"].is_null(), "{stats:?}");
    let days = stats["data"]["dailyStats"].as_array().unwrap();
    assert_eq!(days.len(), 1, "{days:?}");
    assert_eq!((days[0]["businessDate"].as_str(), days[0]["roomsSold"].as_i64()), (Some(&*hotel.day(0)), Some(1)));
    assert_eq!(days[0]["roomRevenue"], 10_000 * 300);

    // The stay is now due out on the business date, and blocks the next close until it is extended.
    let review = graphql(&app, &hotel.owner, NIGHT_AUDIT, json!({"p": hotel.id})).await;
    let departure = &review["data"]["nightAudit"]["departures"][0];
    assert_eq!(departure["reservationRoomId"], json!(stay), "{review:?}");
    let version = departure["version"].as_i64().unwrap();
    let stale =
        versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(stay, "extend"), version - 1, None).await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    let extended = versioned(&app, Method::POST, &hotel.owner, &hotel.room_action(stay, "extend"), version, None).await;
    assert_eq!(extended.status, StatusCode::OK, "{:?}", extended.body);
    assert_eq!(extended.body["check_out"], hotel.day(2));
    assert_eq!(extended.body["total"], 20_000);
    let review = graphql(&app, &hotel.owner, NIGHT_AUDIT, json!({"p": hotel.id})).await;
    assert_eq!(review["data"]["nightAudit"]["blocking"], 0, "{review:?}");
}

/// A close that fails for want of a USD rate completes once the rate is set and it is retried; another property's run
/// cannot be retried through this one.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_failed_close_is_retried_after_the_rate_is_set(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = common::jobs::app(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", false).await;
    let stay = hotel.book(&app, 0, 2).await;
    hotel.check_in(&app, stay).await;

    let run = hotel.start_close(&app).await;
    let failed = hotel.wait_for(&app, run, "FAILED").await;
    assert_eq!(failed["failure"]["step"], "post_charges", "{failed:?}");
    assert_eq!(hotel.business_date_now(&app).await, hotel.day(0));

    let other = post(
        &app,
        &hotel.owner,
        "/api/v1/properties",
        json!({"code": "KDY", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"}),
    )
    .await;
    let elsewhere = format!("/api/v1/properties/{}/night-audit/runs/{run}/retry", other.body["id"].as_str().unwrap());
    let refused = app.send(Method::POST, &elsewhere, Some(&hotel.owner), None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{:?}", refused.body);
    let unknown = format!("{}/night-audit/runs/{}/retry", hotel.path, Uuid::now_v7());
    assert_eq!(app.send(Method::POST, &unknown, Some(&hotel.owner), None).await.status, StatusCode::NOT_FOUND);

    hotel.set_usd_rate(&app).await;
    let retry = format!("{}/night-audit/runs/{run}/retry", hotel.path);
    let retried = app.send(Method::POST, &retry, Some(&hotel.owner), None).await;
    assert_eq!(retried.status, StatusCode::ACCEPTED, "{:?}", retried.body);
    assert_eq!(retried.body["run_id"], json!(run));
    hotel.wait_for(&app, run, "COMPLETED").await;
    assert_eq!(hotel.business_date_now(&app).await, hotel.day(1));
    let again = app.send(Method::POST, &retry, Some(&hotel.owner), None).await;
    assert_eq!(again.status, StatusCode::CONFLICT, "a completed run: {:?}", again.body);
}

/// Another tenant's failed run, retried through this tenant's property, is not found, and stays failed.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenant_s_run_cannot_be_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = common::jobs::app(opts.clone()).await;
    let ours = Hotel::new(&app, opts.clone(), "owner@example.com", "GAL", true).await;
    let theirs = Hotel::new(&app, opts, "other@example.com", "KDY", false).await;
    let stay = theirs.book(&app, 0, 2).await;
    theirs.check_in(&app, stay).await;
    let run = theirs.start_close(&app).await;
    theirs.wait_for(&app, run, "FAILED").await;

    let retry = format!("{}/night-audit/runs/{run}/retry", ours.path);
    let refused = app.send(Method::POST, &retry, Some(&ours.owner), None).await;

    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{:?}", refused.body);
    theirs.wait_for(&app, run, "FAILED").await;
}

/// `audit:` keys reach a front desk stream and not a housekeeping one.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn audit_keys_reach_only_those_who_audit_or_view_folios(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", true).await;
    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "books@example.com", "accountant").await;
    let mut desks = open_stream(&app, &front_desk).await;
    let mut accountants = open_stream(&app, &accountant).await;
    let mut housekeepers = open_stream(&app, &housekeeper).await;
    let tenant: Uuid = sqlx::query_scalar("select tenant_id from property where id = $1")
        .bind(hotel.id)
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    let event = |keys: Vec<String>| {
        LiveEvent::Invalidate(db::Event { tenant_id: db::TenantId(tenant), property_id: Some(hotel.id), keys })
    };
    let audit = format!("audit:{}", hotel.id);
    app.state.events.send(event(vec![audit.clone()])).unwrap();
    app.state.events.send(event(vec![audit.clone(), "reservations".into()])).unwrap();

    assert!(next(&mut desks).await.contains(&audit));
    assert!(next(&mut accountants).await.contains(&audit));
    let heard = next(&mut housekeepers).await;
    assert!(heard.contains("reservations") && !heard.contains("audit:"), "only the other key: {heard}");
    let silent = tokio::time::timeout(StdDuration::from_millis(300), housekeepers.frame()).await;
    assert!(silent.is_err(), "nothing more reaches housekeeping: {silent:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_audit_time_is_set_like_the_check_in_time(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", false).await;

    let invalid =
        versioned(&app, Method::PATCH, &hotel.owner, &hotel.path, 1, Some(json!({"audit_time": "25:00"}))).await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", invalid.body);
    let set = versioned(&app, Method::PATCH, &hotel.owner, &hotel.path, 1, Some(json!({"audit_time": "23:30"}))).await;
    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(set.body["audit_time"], "23:30");
    let read = graphql(&app, &hotel.owner, "{ properties { auditTime } }", json!({})).await;
    assert_eq!(read["data"]["properties"][0]["auditTime"], "23:30");
    let data: Value = sqlx::query_scalar("select data from audit_log where action = 'property.updated'")
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(data["audit_time"], "23:30");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_close_is_503_while_jobs_svc_is_down(_: PgPoolOptions, opts: PgConnectOptions) {
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let app = common::jobs::app_with(opts.clone(), closed).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL", true).await;

    let refused = hotel.close(&app, &hotel.owner).await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE, "{:?}", refused.body);
    assert_eq!(refused.body["detail"], "The jobs service is unavailable");
}
