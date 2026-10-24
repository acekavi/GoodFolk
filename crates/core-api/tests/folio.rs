//! Folios over HTTP: charges, payments, invoices with their PDFs, POS bills, tax rules, exchange rates, the
//! permissions on each, tenant isolation, the `folios` read and its live updates.

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

/// A POST with the CSRF header and a fresh `Idempotency-Key`.
async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    post_with_key(app, cookie, path, &Uuid::now_v7().to_string(), body).await
}

async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
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

const FOLIOS: &str = "query ($p: UUID!, $r: UUID, $rr: UUID, $a: UUID) {
    folios(propertyId: $p, reservationId: $r, reservationRoomId: $rr, accountId: $a) {
        id kind status version reservationRoomId reservationId accountId
        lines { id source category description quantity net total currency baseTotal rate businessDate reverses
                taxes { code kind rateBp base amount presentation } }
        payments { id method amount currency baseAmount refundOf transferId }
        balances { currency amount baseAmount }
        invoices { id kind serial totalLkr pdfReady credits }
    }
}";

/// A property with two deluxe rooms (101, 102) sold on BAR in USD at 100.00 a night for 30 nights from the business
/// date, a USD rate of 300 on the business date, and its owner.
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
    async fn new(app: &TestApp, opts: PgConnectOptions, email: &str, code: &str) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner(email, code).await;
        let property = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        sqlx::query(
            "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
             select tenant_id, id, business_date, 'USD', 300, 'cbsl', business_date from property where id = $1",
        )
        .bind(id)
        .execute(&superuser)
        .await
        .unwrap();
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
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    /// Books one room for two nights from the business date (to `account` if given) and returns the reservation and
    /// its stay.
    async fn book(&self, app: &TestApp, account: Option<Uuid>) -> (Uuid, Uuid) {
        let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
        let guest = post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body;
        let mut booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
            "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                       "check_in": self.day(0), "check_out": self.day(2), "adults": 2}]});
        if let Some(account) = account {
            booking["account_id"] = json!(account);
        }
        let created = post(app, &self.owner, &format!("{}/reservations", self.path), booking).await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        (uuid(&created.body["id"]), uuid(&created.body["rooms"][0]["id"]))
    }

    async fn check_in(&self, app: &TestApp, stay: Uuid) {
        let path = format!("{}/reservation-rooms/{stay}/check-in", self.path);
        let checked_in = versioned(app, Method::POST, &self.owner, &path, 1, None).await;
        assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    }

    /// The stay's guest folio, as `folios` reads it.
    async fn stay_folio(&self, app: &TestApp, cookie: &str, stay: Uuid) -> Value {
        let read = graphql(app, cookie, FOLIOS, json!({"p": self.id, "rr": stay})).await;
        assert!(read["errors"].is_null(), "{read:?}");
        read["data"]["folios"][0].clone()
    }

    async fn audited(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }
}

/// Reservation, check-in, tonight's charges, a manual charge and a USD payment; then the invoice, whose PDF arrives
/// from the in-process media-svc and opens through its link; then a credit note and a new invoice.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_charged_paid_invoiced_credited_and_invoiced_again(_: PgPoolOptions, opts: PgConnectOptions) {
    let stack = common::media::stack(opts.clone()).await;
    let app = &stack.app;
    let hotel = Hotel::new(app, opts, "owner@example.com", "GAL").await;
    let (_, stay) = hotel.book(app, None).await;
    hotel.check_in(app, stay).await;

    let posted = post(app, &hotel.owner, &format!("{}/folio/post-charges", hotel.path), json!({})).await;
    assert_eq!(posted.status, StatusCode::OK, "{:?}", posted.body);
    assert_eq!((posted.body["stays"].as_i64(), posted.body["lines"].as_i64()), (Some(1), Some(1)));
    assert_eq!(posted.body["business_date"], hotel.day(0));
    let again = post(app, &hotel.owner, &format!("{}/folio/post-charges", hotel.path), json!({})).await;
    assert_eq!((again.body["lines"].as_i64(), again.body["skipped_existing"].as_i64()), (Some(0), Some(1)));

    let folio = uuid(&hotel.stay_folio(app, &hotel.owner, stay).await["id"]);
    let folio_path = format!("{}/folios/{folio}", hotel.path);
    let charge = json!({"category": "misc", "description": "Airport transfer", "unit_net": 2_000, "currency": "USD"});
    let charged = post(app, &hotel.owner, &format!("{folio_path}/charges"), charge).await;
    assert_eq!(charged.status, StatusCode::CREATED, "{:?}", charged.body);
    assert_eq!(charged.body["source"], "manual");
    assert_eq!(charged.body["rate"], "300.000000");

    let read = hotel.stay_folio(app, &hotel.owner, stay).await;
    let lines = read["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["source"], "ROOM");
    assert!(!lines[0]["taxes"].as_array().unwrap().is_empty(), "the room night is taxed");
    let due = read["balances"][0]["amount"].as_i64().unwrap();
    assert_eq!(read["balances"][0]["currency"], "USD");
    assert_eq!(due, lines.iter().map(|line| line["total"].as_i64().unwrap()).sum::<i64>());

    let refused = post(app, &hotel.owner, &format!("{folio_path}/invoice"), json!({})).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "an unsettled folio: {:?}", refused.body);
    let payment = json!({"method": "card_terminal", "details": {"approval": "A1B2", "last4": "4242", "brand": "Visa"},
                         "amount": due, "currency": "USD"});
    let paid = post(app, &hotel.owner, &format!("{folio_path}/payments"), payment).await;
    assert_eq!(paid.status, StatusCode::CREATED, "{:?}", paid.body);
    assert_eq!(paid.body["base_amount"], due * 300);

    let issued = post(app, &hotel.owner, &format!("{folio_path}/invoice"), json!({})).await;
    assert_eq!(issued.status, StatusCode::CREATED, "{:?}", issued.body);
    assert_eq!(issued.body["serial"], format!("GAL-{}-000001", hotel.business_date.year()));
    assert_eq!(issued.body["kind"], "invoice");
    assert!(issued.body.get("data").is_none() && issued.body.get("pdf_key").is_none(), "{:?}", issued.body);
    let invoice = uuid(&issued.body["id"]);

    // The PDF is rendered by media-svc and its key comes back through /internal/events.
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(30);
    loop {
        let read = hotel.stay_folio(app, &hotel.owner, stay).await;
        if read["invoices"][0]["pdfReady"] == true {
            assert_eq!(read["status"], "INVOICED");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the invoice PDF never arrived: {read:?}");
        tokio::time::sleep(StdDuration::from_millis(100)).await;
    }
    let link =
        app.send(Method::GET, &format!("{}/invoices/{invoice}/link", hotel.path), Some(&hotel.owner), None).await;
    assert_eq!(link.status, StatusCode::OK, "{:?}", link.body);
    let pdf = common::media::http().get(link.body["url"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(pdf.status(), reqwest::StatusCode::OK);
    assert!(pdf.bytes().await.unwrap().starts_with(b"%PDF"));
    assert_eq!(hotel.audited("invoice.viewed").await, 1);

    let charge = json!({"category": "misc", "description": "Late", "unit_net": 100, "currency": "USD"});
    let frozen = post(app, &hotel.owner, &format!("{folio_path}/charges"), charge).await;
    assert_eq!(frozen.status, StatusCode::CONFLICT, "an invoiced folio takes no lines: {:?}", frozen.body);

    let credited = post(
        app,
        &hotel.owner,
        &format!("{}/invoices/{invoice}/credit-note", hotel.path),
        json!({"reason": "Wrong purchaser"}),
    )
    .await;
    assert_eq!(credited.status, StatusCode::CREATED, "{:?}", credited.body);
    assert_eq!(credited.body["serial"], format!("CN-GAL-{}-000001", hotel.business_date.year()));
    assert_eq!(credited.body["credits"], json!(invoice));
    assert_eq!(credited.body["total_lkr"], -issued.body["total_lkr"].as_i64().unwrap());

    let reissued = post(app, &hotel.owner, &format!("{folio_path}/invoice"), json!({})).await;
    assert_eq!(reissued.status, StatusCode::CREATED, "{:?}", reissued.body);
    assert_eq!(reissued.body["serial"], format!("GAL-{}-000002", hotel.business_date.year()));
    let read = hotel.stay_folio(app, &hotel.owner, stay).await;
    let kinds: Vec<&str> = read["invoices"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["INVOICE", "CREDIT_NOTE", "INVOICE"]);
}

/// Every folio, POS, invoice, tax and exchange-rate route, with a body that would otherwise be accepted for parsing.
fn routes(hotel: &Hotel) -> Vec<(Method, String, Option<Value>, bool)> {
    let (p, id) = (&hotel.path, Uuid::now_v7());
    let reason = json!({"reason": "Because"});
    vec![
        (
            Method::POST,
            format!("{p}/folios/{id}/charges"),
            Some(json!({"category": "misc", "description": "x", "unit_net": 1, "currency": "LKR"})),
            false,
        ),
        (Method::POST, format!("{p}/folios/{id}/lines/{id}/reversal"), Some(reason.clone()), false),
        (
            Method::POST,
            format!("{p}/folios/{id}/lines/{id}/transfer"),
            Some(json!({"to_folio": id, "reason": "x"})),
            false,
        ),
        (
            Method::POST,
            format!("{p}/folios/{id}/payments"),
            Some(json!({"method": "cash", "amount": 1, "currency": "LKR"})),
            false,
        ),
        (Method::POST, format!("{p}/payments/{id}/refunds"), Some(json!({"amount": 1, "reason": "x"})), false),
        (Method::PUT, format!("{p}/reservations/{id}/routing"), Some(json!({"categories": []})), false),
        (Method::POST, format!("{p}/folio/post-charges"), Some(json!({})), false),
        (Method::POST, format!("{p}/folios/{id}/invoice"), Some(json!({})), false),
        (Method::POST, format!("{p}/invoices/{id}/credit-note"), Some(reason), false),
        (Method::GET, format!("{p}/invoices/{id}/link"), None, false),
        (
            Method::POST,
            format!("{p}/pos-bills"),
            Some(json!({"outlet": "BAR", "bill_no": "1", "currency": "LKR",
                        "items": [{"description": "Tea", "quantity": 1, "unit_net": 100}],
                        "target": {"settled": {"method": "cash"}}})),
            false,
        ),
        (Method::POST, format!("{p}/outlets"), Some(json!({"code": "BAR", "name": "Bar"})), false),
        (Method::PATCH, format!("{p}/outlets/{id}"), Some(json!({"name": "Bar"})), true),
        (
            Method::POST,
            format!("{p}/tax-rules"),
            Some(json!({"code": "X", "name": "X", "kind": "levy", "rate_bp": 1, "applies_to": ["*"],
                        "base": ["net"], "sequence": 50, "effective_from": hotel.day(0)})),
            false,
        ),
        (Method::PATCH, format!("{p}/tax-rules/{id}"), Some(json!({"name": "X"})), true),
        (Method::DELETE, format!("{p}/tax-rules/{id}"), None, true),
        (Method::POST, format!("{p}/tax-rules/apply-preset"), Some(json!({})), false),
        (Method::PATCH, format!("{p}/tax-settings"), Some(json!({"vat_registered": true})), true),
        (Method::PUT, format!("{p}/fx-rates/{}/USD", hotel.day(0)), Some(json!({"rate": "300"})), false),
    ]
}

async fn call(
    app: &TestApp,
    cookie: &str,
    (method, path, body, if_match): &(Method, String, Option<Value>, bool),
) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let mut headers = vec![("x-goodfolk-csrf", "1"), ("idempotency-key", key.as_str())];
    if *if_match {
        headers.push(("if-match", "\"1\""));
    }
    app.send_with(method.clone(), path, Some(cookie), body.clone(), &headers).await
}

/// A housekeeper is refused every folio, POS, invoice, tax and exchange-rate route and read; an accountant views
/// folios and issues invoices but posts nothing.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn each_role_gets_exactly_its_folio_permissions(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "acc@example.com", "accountant").await;

    for route in routes(&hotel) {
        let response = call(&app, &housekeeper, &route).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "housekeeper {} {}: {:?}", route.0, route.1, response.body);
    }
    for field in [
        "folios(propertyId: $p, accountId: $p) { id }",
        "taxRules(propertyId: $p) { id }",
        "outlets(propertyId: $p) { id }",
        "fxRates(propertyId: $p, from: \"2026-01-01\", to: \"2026-02-01\") { rate }",
        "fxAlert(propertyId: $p) { message }",
        "propertyTaxSettings(propertyId: $p) { tin }",
    ] {
        let query = format!("query ($p: UUID!) {{ {field} }}");
        let read = graphql(&app, &housekeeper, &query, json!({"p": hotel.id})).await;
        assert!(read["errors"][0]["message"].as_str().unwrap().contains("permission"), "{field}: {read:?}");
        let read = graphql(&app, &accountant, &query, json!({"p": hotel.id})).await;
        assert!(read["errors"].is_null(), "accountant {field}: {read:?}");
    }

    // Allowed to an accountant: past the permission check to the unknown id (404).
    let allowed = ["/invoice", "/link"];
    for route in routes(&hotel) {
        let response = call(&app, &accountant, &route).await;
        if allowed.iter().any(|end| route.1.ends_with(end)) {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "accountant {}: {:?}", route.1, response.body);
        } else {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "accountant {}: {:?}", route.1, response.body);
        }
    }
}

/// Another tenant's folios, lines, payments, invoices and stays are not found through the intruder's own property.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_folios_are_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts.clone(), "owner@example.com", "GAL").await;
    let (_, stay) = hotel.book(&app, None).await;
    hotel.check_in(&app, stay).await;
    let folio = hotel.stay_folio(&app, &hotel.owner, stay).await;
    let folio = uuid(&folio["id"]);
    let charge = json!({"category": "misc", "description": "Laundry", "unit_net": 1_000, "currency": "LKR"});
    let line = post(&app, &hotel.owner, &format!("{}/folios/{folio}/charges", hotel.path), charge.clone()).await;
    let line = uuid(&line.body["id"]);
    let cash = json!({"method": "cash", "amount": 500, "currency": "LKR"});
    let payment = post(&app, &hotel.owner, &format!("{}/folios/{folio}/payments", hotel.path), cash.clone()).await;
    let payment = uuid(&payment.body["id"]);

    let intruder = Hotel::new(&app, opts, "intruder@example.com", "EVL").await;
    let p = &intruder.path;
    let cookie = &intruder.owner;
    for (path, body) in [
        (format!("{p}/folios/{folio}/charges"), charge),
        (format!("{p}/folios/{folio}/payments"), cash.clone()),
        (format!("{p}/folios/{folio}/lines/{line}/reversal"), json!({"reason": "x"})),
        (format!("{p}/payments/{payment}/refunds"), json!({"amount": 100, "reason": "x"})),
        (format!("{p}/folios/{folio}/invoice"), json!({})),
    ] {
        let response = post(&app, cookie, &path, body).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}: {:?}", response.body);
    }
    let read = graphql(&app, cookie, FOLIOS, json!({"p": intruder.id, "rr": stay})).await;
    assert_eq!(read["data"]["folios"], json!([]), "{read:?}");
    // The victim's property is not in the intruder's tenant.
    let response = post(&app, cookie, &format!("{}/folios/{folio}/payments", hotel.path), cash).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND, "{:?}", response.body);
    let unchanged = hotel.stay_folio(&app, &hotel.owner, stay).await;
    assert_eq!(unchanged["payments"].as_array().unwrap().len(), 1);
}

/// A POS bill to a stay posts once: the same bill again returns it (200) and posts nothing; a different bill under
/// the same number is 409.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_replayed_pos_bill_posts_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let (_, stay) = hotel.book(&app, None).await;
    hotel.check_in(&app, stay).await;
    let outlet =
        post(&app, &hotel.owner, &format!("{}/outlets", hotel.path), json!({"code": "BAR", "name": "Bar"})).await;
    assert_eq!(outlet.status, StatusCode::CREATED, "{:?}", outlet.body);
    assert_eq!(outlet.headers["etag"], "\"1\"");

    let bill = json!({"outlet": "BAR", "bill_no": "B-17", "currency": "LKR",
                      "items": [{"description": "Lime juice", "quantity": 2, "unit_net": 50_000}],
                      "target": {"reservation_room": stay}});
    let posted = post(&app, &hotel.owner, &format!("{}/pos-bills", hotel.path), bill.clone()).await;
    assert_eq!(posted.status, StatusCode::CREATED, "{:?}", posted.body);
    let replayed = post(&app, &hotel.owner, &format!("{}/pos-bills", hotel.path), bill.clone()).await;
    assert_eq!(replayed.status, StatusCode::OK, "{:?}", replayed.body);
    assert_eq!(replayed.body["id"], posted.body["id"]);
    let mut changed = bill;
    changed["items"][0]["quantity"] = json!(3);
    let conflict = post(&app, &hotel.owner, &format!("{}/pos-bills", hotel.path), changed).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{:?}", conflict.body);

    let read = hotel.stay_folio(&app, &hotel.owner, stay).await;
    let lines = read["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1, "posted once: {lines:?}");
    assert_eq!(lines[0]["category"], "pos:BAR");
    let codes: Vec<&str> = lines[0]["taxes"].as_array().unwrap().iter().map(|t| t["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["SC", "SSCL", "TDL"], "a POS line carries the service charge");
}

/// A manual rate is set by an owner or manager only, for an open day, replaces the day's rate and is audited.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rate_override_is_audited(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "fd@example.com", "front_desk").await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "mgr@example.com", "manager").await;
    let path = format!("{}/fx-rates/{}/USD", hotel.path, hotel.day(0));

    let refused = app.send(Method::PUT, &path, Some(&front_desk), Some(json!({"rate": "301.5"}))).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let malformed = app.send(Method::PUT, &path, Some(&manager), Some(json!({"rate": "lots"}))).await;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);
    let base = format!("{}/fx-rates/{}/LKR", hotel.path, hotel.day(0));
    let base = app.send(Method::PUT, &base, Some(&manager), Some(json!({"rate": "2"}))).await;
    assert_eq!(base.status, StatusCode::UNPROCESSABLE_ENTITY, "the base currency's rate is always 1");
    let closed = format!("{}/fx-rates/{}/USD", hotel.path, hotel.day(-1));
    let closed = app.send(Method::PUT, &closed, Some(&manager), Some(json!({"rate": "301.5"}))).await;
    assert_eq!(closed.status, StatusCode::CONFLICT, "a day before the business date is closed");

    let set = app.send(Method::PUT, &path, Some(&manager), Some(json!({"rate": "301.5"}))).await;
    assert_eq!(set.status, StatusCode::NO_CONTENT, "{:?}", set.body);
    let audit: Value = sqlx::query_scalar("select data from audit_log where action = 'fx.overridden'")
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(audit["rate"], "301.5");
    assert_eq!(audit["previous"], "300");

    let query = "query ($p: UUID!, $from: Date!, $to: Date!) {
        fxRates(propertyId: $p, from: $from, to: $to) { currency rate source } }";
    let read =
        graphql(&app, &front_desk, query, json!({"p": hotel.id, "from": hotel.day(0), "to": hotel.day(1)})).await;
    assert_eq!(read["data"]["fxRates"], json!([{"currency": "USD", "rate": "301.500000", "source": "manual"}]));
}

/// Tax rules: an invalid set is 422, a valid rule is added and ended, and the preset replaces the rules from today.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tax_rules_are_checked_as_a_set_and_the_preset_applies(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let rules = format!("{}/tax-rules", hotel.path);
    let rule = |base: &str, from: i64| {
        json!({"code": "GREEN", "name": "Green levy", "kind": "levy", "rate_bp": 25, "applies_to": ["room"],
               "base": ["net", base], "sequence": 50, "effective_from": hotel.day(from)})
    };

    let unknown = post(&app, &hotel.owner, &rules, rule("NOPE", -5)).await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", unknown.body);
    assert!(unknown.body["detail"].as_str().unwrap().contains("NOPE"), "{:?}", unknown.body);

    let created = post(&app, &hotel.owner, &rules, rule("TDL", -5)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = uuid(&created.body["id"]);
    // In force since five days ago: its terms are what those days' lines were taxed with.
    let patch = |id: Uuid, version: i64, body: Value| {
        let (app, owner, path) = (&app, hotel.owner.clone(), format!("{rules}/{id}"));
        async move { versioned(app, Method::PATCH, &owner, &path, version, Some(body)).await }
    };
    let rebased = patch(id, 1, json!({"base": ["net"]})).await;
    assert_eq!(rebased.status, StatusCode::CONFLICT, "{:?}", rebased.body);
    assert!(rebased.body["detail"].as_str().unwrap().ends_with("End it and add a new rule"), "{:?}", rebased.body);
    let ended_yesterday = patch(id, 1, json!({"effective_to": hotel.day(-1)})).await;
    assert_eq!(ended_yesterday.status, StatusCode::CONFLICT, "an end in the past rewrites taxed days");
    let renamed = patch(id, 1, json!({"name": "Green tourism levy", "effective_to": hotel.day(30)})).await;
    assert_eq!(renamed.status, StatusCode::OK, "its name and a later end may change: {:?}", renamed.body);
    let ended = versioned(&app, Method::DELETE, &hotel.owner, &format!("{rules}/{id}"), 2, None).await;
    assert_eq!(ended.status, StatusCode::OK, "{:?}", ended.body);
    assert_eq!(ended.body["effective_to"], hotel.day(0));
    assert_eq!(hotel.audited("tax_rule.ended").await, 1);

    let future = post(&app, &hotel.owner, &rules, rule("TDL", 3)).await;
    assert_eq!(future.status, StatusCode::CREATED, "{:?}", future.body);
    // A rule not yet in force changes freely, checked within its set.
    let unknown_base = patch(uuid(&future.body["id"]), 1, json!({"base": ["NOPE"]})).await;
    assert_eq!(unknown_base.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", unknown_base.body);

    let applied = post(&app, &hotel.owner, &format!("{rules}/apply-preset"), json!({})).await;
    assert_eq!(applied.status, StatusCode::OK, "{:?}", applied.body);
    assert_eq!(applied.body["effective_from"], hotel.day(1), "the preset starts tomorrow");
    assert_eq!(applied.body["ended"], 4, "the four preset rules in force end tomorrow");
    assert_eq!(applied.body["removed"], 1, "the rule starting later was never in force");
    let query = "query ($p: UUID!) { taxRules(propertyId: $p) { code effectiveFrom effectiveTo } }";
    let read = graphql(&app, &hotel.owner, query, json!({"p": hotel.id})).await;
    let rows = read["data"]["taxRules"].as_array().unwrap();
    let open: Vec<&str> =
        rows.iter().filter(|rule| rule["effectiveTo"].is_null()).map(|rule| rule["code"].as_str().unwrap()).collect();
    assert_eq!(open, ["SC", "TDL", "SSCL", "VAT"]);
    assert!(rows.iter().filter(|rule| rule["effectiveTo"].is_null()).all(|rule| rule["effectiveFrom"] == hotel.day(1)));
    assert_eq!(rows.iter().filter(|rule| rule["effectiveTo"] == hotel.day(1)).count(), 4);
    let audit: Value = sqlx::query_scalar("select data from audit_log where action = 'tax_rule.preset_applied'")
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(audit["removed"][0]["id"], future.body["id"], "the removed rule is recorded in full: {audit}");
    assert_eq!(audit["removed"][0]["rate_bp"], 25);
    let previous: Vec<&Value> =
        audit["ended"].as_array().unwrap().iter().map(|e| &e["previous_effective_to"]).collect();
    assert_eq!(previous, [&Value::Null; 4], "each ended rule's previous end: {audit}");
    assert_eq!(hotel.audited("tax_rule.preset_applied").await, 1);

    let settings = versioned(
        &app,
        Method::PATCH,
        &hotel.owner,
        &format!("{}/tax-settings", hotel.path),
        1,
        Some(json!({"vat_registered": true, "tin": "123456789-7000", "accepted_currencies": ["USD"]})),
    )
    .await;
    assert_eq!(settings.status, StatusCode::UNPROCESSABLE_ENTITY, "the base currency stays accepted");
}

/// `folios` of a reservation reads its deposit, stay and account folios with each one's own lines and payments; a
/// charge sends `folio:<id>` to the property's live streams.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservations_folios_are_read_at_once_and_charges_are_live(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone(), Default::default())
        .await
        .unwrap();
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let account = json!({"kind": "company", "name": "Acme Travel", "currency": "USD"});
    let account = uuid(&post(&app, &hotel.owner, &format!("{}/accounts", hotel.path), account).await.body["id"]);
    let (reservation, stay) = hotel.book(&app, Some(account)).await;

    let read = graphql(&app, &hotel.owner, FOLIOS, json!({"p": hotel.id, "r": reservation})).await;
    let deposit = uuid(&read["data"]["folios"][0]["id"]);
    let paid = post(
        &app,
        &hotel.owner,
        &format!("{}/folios/{deposit}/payments", hotel.path),
        json!({"method": "bank_transfer", "details": {"reference": "TT-1"}, "amount": 5_000, "currency": "USD"}),
    )
    .await;
    assert_eq!(paid.status, StatusCode::CREATED, "{:?}", paid.body);
    hotel.check_in(&app, stay).await;
    let posted = post(&app, &hotel.owner, &format!("{}/folio/post-charges", hotel.path), json!({})).await;
    assert_eq!(posted.body["lines"], 1, "{:?}", posted.body);
    let guest = uuid(&hotel.stay_folio(&app, &hotel.owner, stay).await["id"]);

    let mut events = app.state.events.subscribe();
    let charge = json!({"category": "misc", "description": "Minibar", "unit_net": 700, "currency": "USD"});
    let charged = post(&app, &hotel.owner, &format!("{}/folios/{guest}/charges", hotel.path), charge).await;
    assert_eq!(charged.status, StatusCode::CREATED, "{:?}", charged.body);
    let key = format!("folio:{guest}");
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(5);
    let event = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let received = tokio::time::timeout(remaining, events.recv()).await.expect("no folio event arrived").unwrap();
        if let LiveEvent::Invalidate(event) = received
            && event.keys.contains(&key)
        {
            break event;
        }
    };
    assert_eq!(event.property_id, Some(hotel.id));

    let read = graphql(&app, &hotel.owner, FOLIOS, json!({"p": hotel.id, "r": reservation})).await;
    assert!(read["errors"].is_null(), "{read:?}");
    let folios = read["data"]["folios"].as_array().unwrap();
    let kinds: Vec<&str> = folios.iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["DEPOSIT", "GUEST", "ACCOUNT"]);
    let (deposit, guest, ledger) = (&folios[0], &folios[1], &folios[2]);
    let methods = |folio: &Value| -> Vec<String> {
        folio["payments"].as_array().unwrap().iter().map(|p| p["method"].as_str().unwrap().to_owned()).collect()
    };
    let categories = |folio: &Value| -> Vec<String> {
        folio["lines"].as_array().unwrap().iter().map(|l| l["category"].as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(methods(deposit), ["bank_transfer", "transfer"], "the deposit moved to the stay at check-in");
    assert!(categories(deposit).is_empty());
    assert_eq!(methods(guest), ["transfer"]);
    assert_eq!(categories(guest), ["misc"], "the minibar stays with the guest");
    assert_eq!(ledger["accountId"], json!(account));
    assert_eq!(ledger["lines"], json!([]), "a reservation reads its account's ledger without lines");
    let by_account = graphql(&app, &hotel.owner, FOLIOS, json!({"p": hotel.id, "a": account})).await;
    let ledger = &by_account["data"]["folios"][0];
    assert!(methods(ledger).is_empty());
    assert_eq!(categories(ledger), ["room"], "the room night is routed to the account");

    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let refused = graphql(&app, &housekeeper, FOLIOS, json!({"p": hotel.id, "r": reservation})).await;
    assert!(refused["data"].is_null() || refused["data"]["folios"].is_null(), "{refused:?}");
    assert!(!refused["errors"].as_array().unwrap().is_empty());
}

/// Staff never post a credit: a negative quantity or unit price, as a charge or a POS item, is 422.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn staff_charges_and_pos_items_are_never_credits(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "fd@example.com", "front_desk").await;
    let (_, stay) = hotel.book(&app, None).await;
    hotel.check_in(&app, stay).await;
    let folio = uuid(&hotel.stay_folio(&app, &hotel.owner, stay).await["id"]);
    let charges = format!("{}/folios/{folio}/charges", hotel.path);
    for (quantity, unit_net) in [(-1, 1_000), (0, 1_000), (1, -1_000), (10_000, 1)] {
        let charge = json!({"category": "misc", "description": "Refund?", "quantity": quantity, "unit_net": unit_net,
                            "currency": "LKR"});
        let refused = post(&app, &front_desk, &charges, charge).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{quantity} × {unit_net}: {:?}", refused.body);
    }
    let free = json!({"category": "misc", "description": "Welcome drink", "unit_net": 0, "currency": "LKR"});
    assert_eq!(post(&app, &front_desk, &charges, free).await.status, StatusCode::CREATED);

    // The module refuses them too, whoever calls it.
    let mut tx = db::begin(&app.pool, db::Scope::tenant(db::TenantId(tenant(&hotel).await))).await.unwrap();
    let charge = folio::NewCharge {
        source: folio::Source::Manual,
        category: "misc".into(),
        description: "Credit".into(),
        quantity: -1,
        unit_net: 1_000,
        currency: "LKR".into(),
        business_date: hotel.business_date,
        posting_key: None,
        inclusive: false,
    };
    let refused = folio::post_charge(&mut tx, db::TenantId(tenant(&hotel).await), None, hotel.id, folio, charge).await;
    assert!(matches!(refused, Err(folio::FolioError::Invalid(_))), "{refused:?}");
    drop(tx);

    assert_eq!(
        post(&app, &hotel.owner, &format!("{}/outlets", hotel.path), json!({"code": "BAR", "name": "Bar"}))
            .await
            .status,
        StatusCode::CREATED
    );
    let bill = |quantity: i64, unit_net: i64, business_date: Option<String>| {
        json!({"outlet": "BAR", "bill_no": Uuid::now_v7().to_string(), "currency": "LKR",
               "business_date": business_date,
               "items": [{"description": "Tea", "quantity": quantity, "unit_net": unit_net}],
               "target": {"reservation_room": stay}})
    };
    for (quantity, unit_net) in [(-2, 500), (0, 500), (1, -500)] {
        let refused =
            post(&app, &hotel.owner, &format!("{}/pos-bills", hotel.path), bill(quantity, unit_net, None)).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{quantity} × {unit_net}: {:?}", refused.body);
    }
    for (offset, status) in
        [(1, StatusCode::UNPROCESSABLE_ENTITY), (-8, StatusCode::UNPROCESSABLE_ENTITY), (-7, StatusCode::CREATED)]
    {
        let posted =
            post(&app, &hotel.owner, &format!("{}/pos-bills", hotel.path), bill(1, 500, Some(hotel.day(offset)))).await;
        assert_eq!(posted.status, status, "business date {offset}: {:?}", posted.body);
    }
}

async fn tenant(hotel: &Hotel) -> Uuid {
    sqlx::query_scalar("select tenant_id from property where id = $1")
        .bind(hotel.id)
        .fetch_one(&hotel.superuser)
        .await
        .unwrap()
}

/// An account folio is read by account with its newest 500 lines and a flag, and through a reservation with its
/// balances only; the balance covers every line either way.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn folio_reads_are_bounded(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let account = json!({"kind": "company", "name": "Acme Travel", "currency": "LKR"});
    let account = uuid(&post(&app, &hotel.owner, &format!("{}/accounts", hotel.path), account).await.body["id"]);
    let (reservation, _) = hotel.book(&app, Some(account)).await;
    let ledger = Uuid::now_v7();
    sqlx::query(
        "insert into folio (id, tenant_id, property_id, kind, account_id)
         select $1, tenant_id, id, 'account', $3 from property where id = $2",
    )
    .bind(ledger)
    .bind(hotel.id)
    .bind(account)
    .execute(&hotel.superuser)
    .await
    .unwrap();
    sqlx::query(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, quantity, net, total, currency,
                                 base_total, rate, rate_date, business_date, posted_at)
         select gen_random_uuid(), f.tenant_id, f.id, 'manual', 'misc', 'Charge ' || n, 1, 100, 100, 'LKR', 100, 1,
                p.business_date, p.business_date, now() - make_interval(secs => 600 - n)
         from folio f join property p on p.id = f.property_id, generate_series(1, 600) as n
         where f.id = $1",
    )
    .bind(ledger)
    .execute(&hotel.superuser)
    .await
    .unwrap();

    let read = graphql(&app, &hotel.owner, FOLIOS_FLAGS, json!({"p": hotel.id, "a": account})).await;
    let folio = &read["data"]["folios"][0];
    assert_eq!(folio["truncated"], true, "{read:?}");
    assert_eq!(folio["linesOmitted"], false);
    let lines = folio["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 500);
    assert_eq!(
        (lines[0]["description"].as_str(), lines[499]["description"].as_str()),
        (Some("Charge 101"), Some("Charge 600"))
    );
    assert_eq!(folio["balances"], json!([{"currency": "LKR", "amount": 60_000, "baseAmount": 60_000}]));

    let read = graphql(&app, &hotel.owner, FOLIOS_FLAGS, json!({"p": hotel.id, "r": reservation})).await;
    let folios = read["data"]["folios"].as_array().unwrap();
    let ledger_read = folios.iter().find(|f| f["kind"] == "ACCOUNT").expect("the account folio");
    assert_eq!(ledger_read["linesOmitted"], true);
    assert_eq!(ledger_read["lines"], json!([]));
    assert_eq!(ledger_read["payments"], json!([]));
    assert_eq!(ledger_read["balances"][0]["amount"], 60_000);
    assert!(
        folios.iter().filter(|f| f["kind"] != "ACCOUNT").all(|f| f["linesOmitted"] == false && f["truncated"] == false)
    );
}

/// A currency an open folio still owes or holds stays accepted until that folio is settled.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_currency_held_by_an_open_folio_stays_accepted(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let (_, stay) = hotel.book(&app, None).await;
    hotel.check_in(&app, stay).await;
    let folio = uuid(&hotel.stay_folio(&app, &hotel.owner, stay).await["id"]);
    let charge = json!({"category": "misc", "description": "Laundry", "unit_net": 1_000, "currency": "USD"});
    let line = post(&app, &hotel.owner, &format!("{}/folios/{folio}/charges", hotel.path), charge).await;
    assert_eq!(line.status, StatusCode::CREATED, "{:?}", line.body);
    let settings = |version: i64| {
        let path = format!("{}/tax-settings", hotel.path);
        let body = json!({"accepted_currencies": ["LKR", "EUR", "GBP"]});
        let (app, owner) = (&app, hotel.owner.clone());
        async move { versioned(app, Method::PATCH, &owner, &path, version, Some(body)).await }
    };

    let refused = settings(1).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    assert!(refused.body.to_string().contains("USD balance"), "{:?}", refused.body);

    let cash = json!({"method": "cash", "amount": line.body["total"], "currency": "USD"});
    let paid = post(&app, &hotel.owner, &format!("{}/folios/{folio}/payments", hotel.path), cash).await;
    assert_eq!(paid.status, StatusCode::CREATED, "{:?}", paid.body);
    let accepted = settings(1).await;
    assert_eq!(accepted.status, StatusCode::OK, "settled, USD can go: {:?}", accepted.body);
}

/// Money is `bigint` minor units; GraphQL's `Int` carries it past 2^31 (LKR 30,000,000.00 here) exactly.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn amounts_past_two_to_the_31_round_trip_through_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let (_, stay) = hotel.book(&app, None).await;
    hotel.check_in(&app, stay).await;
    let folio = uuid(&hotel.stay_folio(&app, &hotel.owner, stay).await["id"]);
    let charge =
        json!({"category": "misc", "description": "Banquet", "unit_net": 3_000_000_000_i64, "currency": "LKR"});
    let line = post(&app, &hotel.owner, &format!("{}/folios/{folio}/charges", hotel.path), charge).await;
    assert_eq!(line.status, StatusCode::CREATED, "{:?}", line.body);
    let total = line.body["total"].as_i64().unwrap();
    assert!(total > i64::from(i32::MAX) && line.body["net"] == 3_000_000_000_i64);

    let read = graphql(&app, &hotel.owner, FOLIOS, json!({"p": hotel.id, "rr": stay})).await;
    assert!(read["errors"].is_null(), "{read:?}");
    let folio = &read["data"]["folios"][0];
    let banquet = folio["lines"].as_array().unwrap().iter().find(|l| l["description"] == "Banquet").unwrap();
    assert_eq!((banquet["net"].as_i64(), banquet["total"].as_i64()), (Some(3_000_000_000), Some(total)));
    let balance = folio["balances"].as_array().unwrap().iter().find(|b| b["currency"] == "LKR").unwrap();
    assert!(balance["amount"].as_i64().unwrap() >= total, "{balance:?}");
}

const FOLIOS_FLAGS: &str = "query ($p: UUID!, $r: UUID, $a: UUID) {
    folios(propertyId: $p, reservationId: $r, accountId: $a) {
        kind linesOmitted truncated lines { description } payments { id } balances { currency amount baseAmount }
    }
}";

/// Another property of the same tenant: its ids sent through this property's paths are not found.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_propertys_ids_are_not_found_through_this_ones_paths(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let b = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let property = json!({"code": "KDY", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let a = uuid(&post(&app, &b.owner, "/api/v1/properties", property).await.body["id"]);
    let a_path = format!("/api/v1/properties/{a}");

    // Booked, not checked in: its folio has no nights to post before it is invoiced.
    let (_, stay) = b.book(&app, None).await;
    let folio = uuid(&b.stay_folio(&app, &b.owner, stay).await["id"]);
    let charge = json!({"category": "misc", "description": "Laundry", "unit_net": 1_000, "currency": "LKR"});
    let line = post(&app, &b.owner, &format!("{}/folios/{folio}/charges", b.path), charge.clone()).await.body;
    let cash = json!({"method": "cash", "amount": line["total"], "currency": "LKR"});
    let line = uuid(&line["id"]);
    let payment =
        uuid(&post(&app, &b.owner, &format!("{}/folios/{folio}/payments", b.path), cash.clone()).await.body["id"]);
    let invoice = post(&app, &b.owner, &format!("{}/folios/{folio}/invoice", b.path), json!({})).await;
    assert_eq!(invoice.status, StatusCode::CREATED, "{:?}", invoice.body);
    let invoice = uuid(&invoice.body["id"]);
    let outlet = post(&app, &b.owner, &format!("{}/outlets", b.path), json!({"code": "BAR", "name": "Bar"})).await;
    let outlet = uuid(&outlet.body["id"]);
    let rule =
        graphql(&app, &b.owner, "query ($p: UUID!) { taxRules(propertyId: $p) { id } }", json!({"p": b.id})).await;
    let rule = uuid(&rule["data"]["taxRules"][0]["id"]);
    // A folio of property A, so a `to_folio` or `from_folio` of B is the only thing wrong.
    let guest = json!({"first_name": "Ana", "last_name": "Fernando", "residency": "resident"});
    assert_eq!(post(&app, &b.owner, &format!("{a_path}/guests"), guest).await.status, StatusCode::CREATED);
    assert_eq!(
        post(&app, &b.owner, &format!("{a_path}/outlets"), json!({"code": "BAR", "name": "Bar"})).await.status,
        StatusCode::CREATED
    );

    let cases = [
        (format!("{a_path}/folios/{folio}/charges"), charge),
        (format!("{a_path}/folios/{folio}/payments"), cash),
        (format!("{a_path}/folios/{folio}/lines/{line}/reversal"), json!({"reason": "x"})),
        (format!("{a_path}/folios/{folio}/lines/{line}/transfer"), json!({"to_folio": folio, "reason": "x"})),
        (format!("{a_path}/payments/{payment}/refunds"), json!({"amount": 100, "reason": "x"})),
        (format!("{a_path}/payments/{payment}/refunds"), json!({"amount": 100, "reason": "x", "from_folio": folio})),
        (format!("{a_path}/folios/{folio}/invoice"), json!({})),
        (format!("{a_path}/invoices/{invoice}/credit-note"), json!({"reason": "x"})),
        (
            format!("{a_path}/pos-bills"),
            json!({"outlet": "BAR", "bill_no": "1", "currency": "LKR",
                   "items": [{"description": "Tea", "quantity": 1, "unit_net": 100}],
                   "target": {"reservation_room": stay}}),
        ),
    ];
    for (path, body) in cases {
        let response = post(&app, &b.owner, &path, body).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}: {:?}", response.body);
    }
    let link = app.send(Method::GET, &format!("{a_path}/invoices/{invoice}/link"), Some(&b.owner), None).await;
    assert_eq!(link.status, StatusCode::NOT_FOUND);
    for (method, path, body) in [
        (Method::PATCH, format!("{a_path}/outlets/{outlet}"), Some(json!({"name": "Pub"}))),
        (Method::PATCH, format!("{a_path}/tax-rules/{rule}"), Some(json!({"name": "Levy"}))),
        (Method::DELETE, format!("{a_path}/tax-rules/{rule}"), None),
    ] {
        let response = versioned(&app, method, &b.owner, &path, 1, body).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}: {:?}", response.body);
    }
    let routing = app
        .send(
            Method::PUT,
            &format!("{a_path}/reservations/{stay}/routing"),
            Some(&b.owner),
            Some(json!({"categories": []})),
        )
        .await;
    assert_eq!(routing.status, StatusCode::NOT_FOUND);
    let read = graphql(&app, &b.owner, FOLIOS, json!({"p": a, "rr": stay})).await;
    assert_eq!(read["data"]["folios"], json!([]));

    // This property's own line and payment, sent to a folio of the other property: as if that folio did not exist.
    let account = json!({"kind": "company", "name": "Acme Travel", "currency": "LKR"});
    let account = uuid(&post(&app, &b.owner, &format!("{}/accounts", b.path), account).await.body["id"]);
    let other_folio = Uuid::now_v7();
    sqlx::query("insert into folio (id, tenant_id, property_id, kind, account_id) values ($1, $2, $3, 'account', $4)")
        .bind(other_folio)
        .bind(tenant(&b).await)
        .bind(a)
        .bind(account)
        .execute(&b.superuser)
        .await
        .unwrap();
    let credit_path = format!("{}/invoices/{invoice}/credit-note", b.path);
    let credited = post(&app, &b.owner, &credit_path, json!({"reason": "x"})).await;
    assert_eq!(credited.status, StatusCode::CREATED, "the stay's folio is open again: {:?}", credited.body);
    for (path, body) in [
        (format!("{}/folios/{folio}/lines/{line}/transfer", b.path), json!({"to_folio": other_folio, "reason": "x"})),
        (
            format!("{}/payments/{payment}/refunds", b.path),
            json!({"amount": 100, "reason": "x", "from_folio": other_folio}),
        ),
    ] {
        let response = post(&app, &b.owner, &path, body).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}: {:?}", response.body);
    }
}

/// Folio and exchange-rate keys reach only streams of users who may view the property's folios.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn money_keys_reach_only_folio_viewers(_: PgPoolOptions, opts: PgConnectOptions) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let open = |cookie: String| {
        let router = app.router.clone();
        async move {
            let request = axum::http::Request::builder()
                .uri("/api/v1/events")
                .header(axum::http::header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap();
            let mut body = router.oneshot(request).await.unwrap().into_body();
            let ready = tokio::time::timeout(StdDuration::from_secs(1), body.frame()).await.unwrap().unwrap().unwrap();
            assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");
            body
        }
    };
    let mut owners = open(hotel.owner.clone()).await;
    let mut housekeepers = open(housekeeper).await;
    let tenant = db::TenantId(tenant(&hotel).await);
    let event =
        |keys: Vec<String>| LiveEvent::Invalidate(db::Event { tenant_id: tenant, property_id: Some(hotel.id), keys });
    let money = format!("folio:{}", Uuid::now_v7());
    app.state.events.send(event(vec![money.clone(), format!("fx:{}", hotel.id)])).unwrap();
    app.state.events.send(event(vec![money.clone(), "rooms:x".into()])).unwrap();

    assert!(next(&mut owners).await.contains(&money));
    assert!(next(&mut owners).await.contains("rooms:x"));
    let heard = next(&mut housekeepers).await;
    assert!(heard.contains("rooms:x") && !heard.contains("folio:"), "only the other key: {heard}");
    let silent = tokio::time::timeout(StdDuration::from_millis(300), housekeepers.frame()).await;
    assert!(silent.is_err(), "an event of money keys only is not sent at all");
}

/// A grant for one property hears that property's money keys, and not another property's.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn money_keys_follow_a_property_scoped_grant(_: PgPoolOptions, opts: PgConnectOptions) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let property = json!({"code": "KDY", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let other = uuid(&post(&app, &hotel.owner, "/api/v1/properties", property).await.body["id"]);
    let tenant = tenant(&hotel).await;
    let cookie = app.signup_owner("desk@example.com", "Own tenant").await;
    let user = uuid(&app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await.body["user_id"]);
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(tenant)
        .bind(user)
        .execute(&hotel.superuser)
        .await
        .unwrap();
    sqlx::query(
        "insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, 'front_desk')",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(user)
    .bind(hotel.id)
    .execute(&hotel.superuser)
    .await
    .unwrap();
    let switched =
        app.send(Method::PUT, "/api/v1/session/tenant", Some(&cookie), Some(json!({"tenant_id": tenant}))).await;
    assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);
    let request = axum::http::Request::builder()
        .uri("/api/v1/events")
        .header(axum::http::header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let mut stream = app.router.clone().oneshot(request).await.unwrap().into_body();
    let ready = tokio::time::timeout(StdDuration::from_secs(1), stream.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(ready.into_data().unwrap(), "event: ready\ndata: {}\n\n");

    let event = |property: Uuid, key: String| {
        LiveEvent::Invalidate(db::Event {
            tenant_id: db::TenantId(tenant),
            property_id: Some(property),
            keys: vec![key],
        })
    };
    let (theirs, ours) = (format!("folio:{}", Uuid::now_v7()), format!("folio:{}", Uuid::now_v7()));
    app.state.events.send(event(other, theirs.clone())).unwrap();
    app.state.events.send(event(hotel.id, ours.clone())).unwrap();

    let heard = next(&mut stream).await;
    assert!(heard.contains(&ours) && !heard.contains(&theirs), "only the granted property's key: {heard}");
}

/// The next frame of an event stream, as text.
async fn next(body: &mut axum::body::Body) -> String {
    use http_body_util::BodyExt;
    let frame = tokio::time::timeout(StdDuration::from_secs(2), body.frame()).await.unwrap().unwrap().unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}

/// A reservation's routing reads back as its account pays it, to folio viewers only.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservations_routing_is_read_by_folio_viewers(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let account = json!({"kind": "company", "name": "Acme Travel", "currency": "USD"});
    let account = uuid(&post(&app, &hotel.owner, &format!("{}/accounts", hotel.path), account).await.body["id"]);
    let (reservation, _) = hotel.book(&app, Some(account)).await;
    let query = "query ($p: UUID!, $r: UUID!) { routing(propertyId: $p, reservationId: $r) }";
    let vars = json!({"p": hotel.id, "r": reservation});

    let read = graphql(&app, &hotel.owner, query, vars.clone()).await;
    assert_eq!(read["data"]["routing"], json!(["meal_plan", "room"]), "{read:?}");
    let path = format!("{}/reservations/{reservation}/routing", hotel.path);
    let routed = app
        .send_with(
            Method::PUT,
            &path,
            Some(&hotel.owner),
            Some(json!({"categories": ["room", "laundry"]})),
            &[("x-goodfolk-csrf", "1")],
        )
        .await;
    assert_eq!(routed.status, StatusCode::NO_CONTENT, "{:?}", routed.body);
    let read = graphql(&app, &hotel.owner, query, vars.clone()).await;
    assert_eq!(read["data"]["routing"], json!(["laundry", "room"]));

    let accountant = app.staff(&hotel.superuser, &hotel.owner, "acc@example.com", "accountant").await;
    assert_eq!(graphql(&app, &accountant, query, vars.clone()).await["data"]["routing"], json!(["laundry", "room"]));
    let housekeeper = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let refused = graphql(&app, &housekeeper, query, vars).await;
    assert!(refused["errors"][0]["message"].as_str().unwrap().contains("permission"), "{refused:?}");
    let missing = graphql(&app, &hotel.owner, query, json!({"p": hotel.id, "r": Uuid::now_v7()})).await;
    assert!(!missing["errors"].as_array().unwrap().is_empty(), "{missing:?}");
}
