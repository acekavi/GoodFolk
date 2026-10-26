//! The everyday reports and daily lists over GraphQL: `reportSummary` needs `ReportsView`, and `dailyList` also
//! accepts `ReservationsView`. The liability reports (`tdlReport`, `ssclReport`, `vatReport`, `serviceChargeReport`)
//! and the VAT schedule downloads need `ReportsLiability`. Every report renders as a PDF, on demand or queued by the
//! close, through the in-process media-svc. A rendered weekly or monthly report is mailed once to each owner and
//! manager who has not turned it off (`reportEmails`, `PUT …/report-emails/{kind}`).

mod common;

use axum::http::{Method, StatusCode, header};
use common::media::{Hotel as MediaHotel, SECRET, envelope, http, internal_app, post, push, stack};
use common::{TestApp, uuid};
use db::{Scope, TenantId};
use mail::MailJob;
use media_types::{MediaResult, ReportTemplate};
use reports::ReportKind;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use time::format_description::well_known::Iso8601;
use time::macros::date;
use uuid::Uuid;

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const REPORT_SUMMARY: &str = "query ($p: UUID!, $from: Date!, $to: Date!) {
    reportSummary(propertyId: $p, from: $from, to: $to) {
        closedFrom closedTo notClosed
        headline { occupancyBp adr revpar roomNights totalRevenue }
        lastYear { roomNights }
        days { businessDate }
        breakdowns { dimension keys { key } }
    }
}";

const DAILY_LIST: &str = "query ($p: UUID!, $date: Date!, $kind: DailyListKind!) {
    dailyList(propertyId: $p, date: $date, kind: $kind) { confirmation guest roomNumber status }
}";

fn denied(body: &Value) -> bool {
    body["errors"][0]["message"].as_str().is_some_and(|message| message.contains("permission"))
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn front_desk_reads_daily_lists_but_not_reports(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = app
        .send_with(
            Method::POST,
            "/api/v1/properties",
            Some(&owner),
            Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
            &[("x-goodfolk-csrf", "1"), ("idempotency-key", &Uuid::now_v7().to_string())],
        )
        .await;
    let property = uuid(&created.body["id"]);
    let date = created.body["business_date"].as_str().unwrap().to_owned();
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
    let summary = json!({"p": property, "from": date, "to": date});
    let list = json!({"p": property, "date": date, "kind": "IN_HOUSE"});

    let by_desk = graphql(&app, &front_desk, DAILY_LIST, list.clone()).await;
    assert_eq!(by_desk["data"]["dailyList"], json!([]), "{by_desk}");
    let refused = graphql(&app, &front_desk, REPORT_SUMMARY, summary.clone()).await;
    assert!(denied(&refused), "{refused}");

    let by_manager = graphql(&app, &manager, DAILY_LIST, list).await;
    assert_eq!(by_manager["data"]["dailyList"], json!([]), "{by_manager}");
    let report = graphql(&app, &manager, REPORT_SUMMARY, summary).await;
    assert_eq!(
        report["data"]["reportSummary"],
        json!({
            "closedFrom": null,
            "closedTo": null,
            "notClosed": [date],
            "headline": {"occupancyBp": 0, "adr": 0, "revpar": 0, "roomNights": 0, "totalRevenue": 0},
            "lastYear": null,
            "days": [],
            "breakdowns": [],
        }),
        "{report}"
    );
}

const TDL_REPORT: &str = "query ($p: UUID!) {
    tdlReport(propertyId: $p, fyStartYear: 2026, quarter: 2) { periodLabel totalTurnover tdl notClosed }
}";

const SSCL_REPORT: &str = "query ($p: UUID!) {
    ssclReport(propertyId: $p, year: 2026, quarter: 3) { months { month levy due } quarterTotals { levy } returnDue }
}";

const VAT_REPORT: &str = "query ($p: UUID!, $period: String!) {
    vatReport(propertyId: $p, period: $period) { periodCode valueOfSupply netVat schedule01 { invoiceNo } }
}";

const SERVICE_CHARGE_REPORT: &str = "query ($p: UUID!) {
    serviceChargeReport(propertyId: $p, year: 2026, month: 8) { byOutlet { outlet } total { collected absorbed } }
}";

/// A property of a new tenant, with its owner's cookie and the superuser pool.
async fn hotel(app: &TestApp, opts: PgConnectOptions) -> (PgPool, String, Uuid) {
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let created = app
        .send_with(
            Method::POST,
            "/api/v1/properties",
            Some(&owner),
            Some(json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"})),
            &[("x-goodfolk-csrf", "1"), ("idempotency-key", &Uuid::now_v7().to_string())],
        )
        .await;
    (superuser, owner, uuid(&created.body["id"]))
}

async fn set_vat(superuser: &PgPool, property: Uuid, tin: Option<&str>) {
    sqlx::query("update property set vat_registered = true, tin = $2 where id = $1")
        .bind(property)
        .bind(tin)
        .execute(superuser)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn liability_reports_are_for_owners_and_accountants(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let (superuser, owner, property) = hotel(&app, opts).await;
    set_vat(&superuser, property, Some("300767028")).await;
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let accountant = app.staff(&superuser, &owner, "accountant@example.com", "accountant").await;
    let p = json!({"p": property});
    let vat = json!({"p": property, "period": "2630"});
    let queries = [(TDL_REPORT, &p), (SSCL_REPORT, &p), (VAT_REPORT, &vat), (SERVICE_CHARGE_REPORT, &p)];
    let schedule01 = format!("/api/v1/properties/{property}/reports/vat/2630/schedule01.csv");
    let schedule04 = format!("/api/v1/properties/{property}/reports/vat/2630/schedule04.csv");

    for (query, variables) in queries {
        let refused = graphql(&app, &manager, query, variables.clone()).await;
        assert!(denied(&refused), "{refused}");
        let read = graphql(&app, &accountant, query, variables.clone()).await;
        assert!(read["errors"].is_null(), "{read}");
    }
    let report = graphql(&app, &accountant, VAT_REPORT, vat).await;
    assert_eq!(
        report["data"]["vatReport"],
        json!({"periodCode": "2630", "valueOfSupply": 0, "netVat": 0, "schedule01": []}),
        "{report}"
    );
    for path in [&schedule01, &schedule04] {
        let refused = app.send(Method::GET, path, Some(&manager), None).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }

    let csv = app.send(Method::GET, &schedule01, Some(&accountant), None).await;
    assert_eq!(csv.status, StatusCode::OK, "{:?}", csv.body);
    assert_eq!(csv.headers[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    let disposition = csv.headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename=\"300767028_VAT_SCHEDULE01_2630_"), "{disposition}");
    assert!(disposition.ends_with("_ORIGINAL_V1.csv\""), "{disposition}");
    assert_eq!(
        csv.body,
        "Serial No,Invoice Date,Tax Invoice No,Purchaser's TIN,Name of the Purchaser,Description,Value of supply,\
         VAT Amount\r\n"
    );
    let notes = app.send(Method::GET, &schedule04, Some(&owner), None).await;
    assert_eq!(notes.status, StatusCode::OK, "{:?}", notes.body);
    let disposition = notes.headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.contains("300767028_VAT_SCHEDULE04_2630_"), "{disposition}");
}

/// A property that is not VAT registered, has no TIN, or files the other kind of period gets a 422 and no file; over
/// GraphQL, the same refusal.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn vat_schedules_are_refused_without_registration_or_tin(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let (superuser, owner, property) = hotel(&app, opts).await;
    let schedule01 = |period: &str| format!("/api/v1/properties/{property}/reports/vat/{period}/schedule01.csv");

    let refused = graphql(&app, &owner, VAT_REPORT, json!({"p": property, "period": "2630"})).await;
    assert_eq!(refused["errors"][0]["message"], "This property is not VAT registered", "{refused}");
    let unregistered = app.send(Method::GET, &schedule01("2630"), Some(&owner), None).await;
    assert_eq!(unregistered.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(unregistered.body["detail"], "This property is not VAT registered");
    assert!(unregistered.headers.get(header::CONTENT_DISPOSITION).is_none());

    set_vat(&superuser, property, None).await;
    let no_tin = app.send(Method::GET, &schedule01("2630"), Some(&owner), None).await;
    assert_eq!(no_tin.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(no_tin.body["detail"], "Set the property's TIN on Settings → Taxes and invoices first");
    assert!(no_tin.headers.get(header::CONTENT_DISPOSITION).is_none());

    set_vat(&superuser, property, Some("300767028")).await;
    let month = app.send(Method::GET, &schedule01("2632"), Some(&owner), None).await;
    assert_eq!(month.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(month.body["detail"], "This property files its VAT return quarterly");
    let malformed = app.send(Method::GET, &schedule01("26Q3"), Some(&owner), None).await;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// The same weekly report queued twice is one report and one render job; an on-demand report is new every time.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn queue_render_is_idempotent(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let (superuser, owner, property) = hotel(&app, opts).await;
    let tenant = TenantId(uuid(&app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body["current_tenant"]));
    let (from, to) = (date!(2026 - 10 - 19), date!(2026 - 10 - 25));
    let queue = |kind: ReportKind| {
        let pool = app.pool.clone();
        async move {
            let mut tx = db::begin(&pool, Scope::tenant(tenant)).await.unwrap();
            let id = reports::queue_render(
                &mut tx,
                tenant,
                property,
                kind,
                from,
                to,
                ReportTemplate::Summary,
                json!({"title": "Weekly report"}),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            id
        }
    };
    let count = |sql: &'static str| {
        let superuser = superuser.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&superuser).await.unwrap() }
    };

    let first = queue(ReportKind::Weekly).await;
    assert_eq!(queue(ReportKind::Weekly).await, first, "the same key gives the same report");
    assert_eq!(count("select count(*) from report").await, 1);
    assert_eq!(count("select count(*) from outbox where topic = 'report-render'").await, 1);
    let (status, data): (String, Value) = sqlx::query_as("select status, data from report where id = $1")
        .bind(first)
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(status, "rendering");
    assert_eq!(data, json!({"template": "summary", "data": {"title": "Weekly report"}}));
    let job: Value = sqlx::query_scalar("select payload from outbox where topic = 'report-render'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(
        job,
        json!({"tenant_id": tenant.0, "report_id": first, "template": "summary", "data": {"title": "Weekly report"}})
    );

    assert_ne!(queue(ReportKind::OnDemand).await, queue(ReportKind::OnDemand).await, "on demand is always new");
    assert_eq!(count("select count(*) from report").await, 3);
    assert_eq!(count("select count(*) from outbox where topic = 'report-render'").await, 3);
}

/// Polls the report until it is no longer rendering, and returns its last read.
async fn settled(app: &TestApp, cookie: &str, path: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let read = app.send(Method::GET, path, Some(cookie), None).await;
        assert_eq!(read.status, StatusCode::OK, "{:?}", read.body);
        if read.body["status"] != "rendering" {
            return read.body;
        }
        assert!(tokio::time::Instant::now() < deadline, "the report never rendered: {:?}", read.body);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every report, rendered on demand by the in-process media-svc: 202 with its id, then ready with a link that works
/// for a few minutes, each link audited.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn on_demand_render_round_trip(_: PgPoolOptions, opts: PgConnectOptions) {
    let stack = stack(opts.clone()).await;
    let app = &stack.app;
    let hotel = MediaHotel::new(app, "owner@example.com", "GAL").await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    set_vat(&superuser, hotel.id, Some("300767028")).await;
    let requests = [
        json!({"report": "summary", "params": {"from": "2026-10-19", "to": "2026-10-25"}}),
        json!({"report": "daily_list", "params": {"date": "2026-10-26", "kind": "arrivals"}}),
        json!({"report": "tdl", "params": {"fy_start_year": 2026, "quarter": 2}}),
        json!({"report": "sscl", "params": {"year": 2026, "quarter": 3}}),
        json!({"report": "vat", "params": {"period": "2630"}}),
        json!({"report": "service_charge", "params": {"year": 2026, "month": 8}}),
    ];
    for request in requests {
        let queued = post(app, &hotel.owner, &format!("{}/reports/render", hotel.path), request.clone()).await;
        assert_eq!(queued.status, StatusCode::ACCEPTED, "{request}: {:?}", queued.body);
        let path = format!("{}/reports/{}", hotel.path, uuid(&queued.body["id"]));
        let ready = settled(app, &hotel.owner, &path).await;
        assert_eq!(ready["status"], "ready", "{request}: {ready}");
        let pdf = http().get(ready["url"].as_str().unwrap()).send().await.unwrap();
        assert_eq!(pdf.status(), reqwest::StatusCode::OK);
        assert!(pdf.bytes().await.unwrap().starts_with(b"%PDF-"), "{request}");
    }
    let viewed: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'report.viewed'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(viewed, 6, "each link issued is audited");
    let stored: i64 = sqlx::query_scalar(
        "select count(*) from report where kind = 'on_demand' and status = 'ready' and rendered_at is not null",
    )
    .fetch_one(&superuser)
    .await
    .unwrap();
    assert_eq!(stored, 6);
}

const FOLIOS: &str = "query ($p: UUID!, $rr: UUID) {
    folios(propertyId: $p, reservationRoomId: $rr) { id balances { currency amount } }
}";

/// A day of trade, closed: a stay of a guest with a TIN, its room night, a POS bill at the bar, payments in USD and
/// LKR, a tax invoice and its credit note. Every report renders from it through the in-process media-svc, so each row
/// the templates lay out is one `document` built from real data.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_report_renders_from_a_closed_day_of_trade(_: PgPoolOptions, opts: PgConnectOptions) {
    let stack = stack(opts.clone()).await;
    let app = &stack.app;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let created = post(app, &owner, "/api/v1/properties", property).await.body;
    let (id, path) = (uuid(&created["id"]), format!("/api/v1/properties/{}", created["id"].as_str().unwrap()));
    let day = time::Date::parse(created["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
    for sql in [
        "update property set vat_registered = true, tin = '300767028', legal_name = 'Galle Fort Hotels (Pvt) Ltd'
         where id = $1",
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         select tenant_id, id, business_date, 'USD', 300, 'cbsl', business_date from property where id = $1",
    ] {
        sqlx::query(sql).bind(id).execute(&superuser).await.unwrap();
    }
    let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2, "max_children": 1,
                        "max_occupancy": 3});
    let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
    let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
    assert_eq!(post(app, &owner, &format!("{path}/rooms/bulk"), range).await.status, StatusCode::CREATED);
    let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                     "currency": "USD", "room_type_ids": [deluxe]});
    let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
    let prices: Vec<Value> = (0..2)
        .map(|offset| {
            json!({"room_type_id": deluxe, "date": (day + time::Duration::days(offset)).to_string(),
                             "occupancy": 2, "amount": 10_000})
        })
        .collect();
    let priced = app
        .send(Method::PUT, &format!("{path}/rate-plans/{bar}/prices"), Some(&owner), Some(json!({"prices": prices})))
        .await;
    assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
    let outlet = post(app, &owner, &format!("{path}/outlets"), json!({"code": "BAR", "name": "Bar"})).await;
    assert_eq!(outlet.status, StatusCode::CREATED, "{:?}", outlet.body);

    let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
    let guest = uuid(&post(app, &owner, &format!("{path}/guests"), guest).await.body["id"]);
    sqlx::query("update guest set tin = '102345678' where id = $1").bind(guest).execute(&superuser).await.unwrap();
    let booking = json!({"booker_guest_id": guest, "source": "front_desk",
        "rooms": [{"room_type_id": deluxe, "rate_plan_id": bar, "meal_plan": "RO", "check_in": day.to_string(),
                   "check_out": (day + time::Duration::days(2)).to_string(), "adults": 2}]});
    let booked = post(app, &owner, &format!("{path}/reservations"), booking).await;
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    let stay = uuid(&booked.body["rooms"][0]["id"]);
    let checked_in = app
        .send_with(
            Method::POST,
            &format!("{path}/reservation-rooms/{stay}/check-in"),
            Some(&owner),
            None,
            &[("x-goodfolk-csrf", "1"), ("if-match", "\"1\"")],
        )
        .await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    let posted = post(app, &owner, &format!("{path}/folio/post-charges"), json!({})).await;
    assert_eq!(posted.status, StatusCode::OK, "{:?}", posted.body);
    let bill = json!({"outlet": "BAR", "bill_no": "B-1", "currency": "LKR",
                      "items": [{"description": "Lime juice", "quantity": 2, "unit_net": 50_000}],
                      "target": {"reservation_room": stay}});
    let billed = post(app, &owner, &format!("{path}/pos-bills"), bill).await;
    assert_eq!(billed.status, StatusCode::CREATED, "{:?}", billed.body);

    let read = graphql(app, &owner, FOLIOS, json!({"p": id, "rr": stay})).await;
    let folio = uuid(&read["data"]["folios"][0]["id"]);
    for balance in read["data"]["folios"][0]["balances"].as_array().unwrap() {
        let payment = match balance["currency"].as_str().unwrap() {
            "USD" => json!({"method": "card_terminal", "details": {"approval": "A1", "last4": "4242", "brand": "Visa"},
                            "amount": balance["amount"], "currency": "USD"}),
            currency => json!({"method": "cash", "amount": balance["amount"], "currency": currency}),
        };
        let paid = post(app, &owner, &format!("{path}/folios/{folio}/payments"), payment).await;
        assert_eq!(paid.status, StatusCode::CREATED, "{:?}", paid.body);
    }
    let invoice = post(app, &owner, &format!("{path}/folios/{folio}/invoice"), json!({})).await;
    assert_eq!(invoice.status, StatusCode::CREATED, "{:?}", invoice.body);
    let note = json!({"reason": "Wrong purchaser"});
    let credited =
        post(app, &owner, &format!("{path}/invoices/{}/credit-note", invoice.body["id"].as_str().unwrap()), note).await;
    assert_eq!(credited.status, StatusCode::CREATED, "{:?}", credited.body);

    // The close of the day, as the night audit stores it, and the business date past it.
    let tenant: Uuid = sqlx::query_scalar("select tenant_id from property where id = $1")
        .bind(id)
        .fetch_one(&superuser)
        .await
        .unwrap();
    let mut tx = db::begin(&app.pool, Scope::tenant(TenantId(tenant))).await.unwrap();
    let stats = audit::compute(&mut tx, id, day).await.unwrap();
    audit::store(&mut tx, TenantId(tenant), id, &stats).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update property set business_date = business_date + 1 where id = $1")
        .bind(id)
        .execute(&superuser)
        .await
        .unwrap();

    let (year, month) = (day.year(), u8::from(day.month()));
    let quarter = (month - 1) / 3 + 1;
    let (fy_start_year, fiscal_quarter) = if month >= 4 { (year, (month - 4) / 3 + 1) } else { (year - 1, 4) };
    let vat_period = reports::VatPeriod::Quarter(year, quarter).code();
    let requests = [
        json!({"report": "summary", "params": {"from": day.to_string(), "to": day.to_string()}}),
        json!({"report": "daily_list", "params": {"date": day.to_string(), "kind": "in_house"}}),
        json!({"report": "tdl", "params": {"fy_start_year": fy_start_year, "quarter": fiscal_quarter}}),
        json!({"report": "sscl", "params": {"year": year, "quarter": quarter}}),
        json!({"report": "vat", "params": {"period": vat_period}}),
        json!({"report": "service_charge", "params": {"year": year, "month": month}}),
    ];
    let mut documents = Vec::new();
    for request in requests {
        let queued = post(app, &owner, &format!("{path}/reports/render"), request.clone()).await;
        assert_eq!(queued.status, StatusCode::ACCEPTED, "{request}: {:?}", queued.body);
        let report = uuid(&queued.body["id"]);
        let ready = settled(app, &owner, &format!("{path}/reports/{report}")).await;
        assert_eq!(ready["status"], "ready", "{request}: {ready}");
        let pdf = http().get(ready["url"].as_str().unwrap()).send().await.unwrap();
        assert!(pdf.bytes().await.unwrap().starts_with(b"%PDF-"), "{request}");
        let data: Value = sqlx::query_scalar("select data->'data' from report where id = $1")
            .bind(report)
            .fetch_one(&superuser)
            .await
            .unwrap();
        documents.push(data);
    }

    // Every row path of every template held data.
    let [summary, list, tdl, sscl, vat, service_charge] = &documents[..] else { unreachable!() };
    assert_eq!(summary["days"].as_array().unwrap().len(), 1, "{summary}");
    assert!(!summary["breakdowns"].as_array().unwrap().is_empty(), "{summary}");
    assert_eq!(list["rows"][0]["guest"], "Ada Silva", "{list}");
    assert_ne!(tdl["sales"][0]["value"], "0", "room sales: {tdl}");
    assert_ne!(tdl["sales"][1]["value"], "0", "food sales (an outlet is food until regrouped): {tdl}");
    assert_ne!(sscl["rows"][0]["values"][3], "Rs 0", "{sscl}");
    let tin_rows: Vec<&Value> =
        vat["schedule01"].as_array().unwrap().iter().filter(|row| row["tin"] == "102345678").collect();
    assert_eq!(tin_rows.len(), 1, "{vat}");
    assert_eq!(vat["schedule04"].as_array().unwrap().len(), 1, "{vat}");
    assert!(
        service_charge["rows"].as_array().unwrap().iter().any(|row| row["label"] == "Outlet BAR"),
        "{service_charge}"
    );
}

/// A report media-svc can never render is marked failed, with no link, and raises the property's `reports` alert.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn render_failure_raises_reports_alert(_: PgPoolOptions, opts: PgConnectOptions) {
    let stack = stack(opts.clone()).await;
    let app = &stack.app;
    let hotel = MediaHotel::new(app, "owner@example.com", "GAL").await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let tenant = TenantId(hotel.tenant);
    let mut tx = db::begin(&app.pool, Scope::tenant(tenant)).await.unwrap();
    // Data the template cannot read: it has none of the head's fields.
    let report = reports::queue_render(
        &mut tx,
        tenant,
        hotel.id,
        ReportKind::Tdl,
        date!(2026 - 07 - 01),
        date!(2026 - 09 - 30),
        ReportTemplate::TdlFormA,
        json!({"unexpected": true}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let failed = settled(app, &hotel.owner, &format!("{}/reports/{report}", hotel.path)).await;
    assert_eq!(failed, json!({"status": "failed"}));
    let error: Option<String> =
        sqlx::query_scalar("select error from report where id = $1").bind(report).fetch_one(&superuser).await.unwrap();
    assert!(error.is_some_and(|error| error.starts_with("laying out the report failed")));
    let alert: String = sqlx::query_scalar("select message from ops_alert where property_id = $1 and kind = 'reports'")
        .bind(hotel.id)
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(alert, "The TDL report PDF for 2026-07-01 to 2026-09-30 could not be made");
}

/// A command with the CSRF header and a fresh `Idempotency-Key`.
async fn render(app: &TestApp, cookie: &str, property: Uuid, body: Value) -> common::TestResponse {
    let key = Uuid::now_v7().to_string();
    let path = format!("/api/v1/properties/{property}/reports/render");
    app.send_with(Method::POST, &path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

/// The liability reports render for owners and accountants only, and so do their links; the everyday report for
/// managers too; a daily list for the front desk too. Parameters are checked as the reports check them.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn liability_render_needs_reports_liability(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let (superuser, owner, property) = hotel(&app, opts).await;
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let accountant = app.staff(&superuser, &owner, "accountant@example.com", "accountant").await;
    let desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
    let tdl = json!({"report": "tdl", "params": {"fy_start_year": 2026, "quarter": 2}});
    let summary = json!({"report": "summary", "params": {"from": "2026-10-01", "to": "2026-10-07"}});
    let list = json!({"report": "daily_list", "params": {"date": "2026-10-01", "kind": "in_house"}});

    for liability in [
        &tdl,
        &json!({"report": "sscl", "params": {"year": 2026, "quarter": 3}}),
        &json!({"report": "vat", "params": {"period": "2630"}}),
        &json!({"report": "service_charge", "params": {"year": 2026, "month": 8}}),
    ] {
        assert_eq!(render(&app, &manager, property, liability.clone()).await.status, StatusCode::FORBIDDEN);
    }
    let queued = render(&app, &accountant, property, tdl.clone()).await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{:?}", queued.body);
    let path = format!("/api/v1/properties/{property}/reports/{}", uuid(&queued.body["id"]));
    assert_eq!(app.send(Method::GET, &path, Some(&manager), None).await.status, StatusCode::FORBIDDEN);
    let read = app.send(Method::GET, &path, Some(&accountant), None).await;
    assert_eq!(read.body, json!({"status": "rendering"}), "no link before it is ready");

    assert_eq!(render(&app, &manager, property, summary.clone()).await.status, StatusCode::ACCEPTED);
    assert_eq!(render(&app, &desk, property, summary).await.status, StatusCode::FORBIDDEN);
    assert_eq!(render(&app, &desk, property, list.clone()).await.status, StatusCode::ACCEPTED);
    assert_eq!(render(&app, &desk, property, tdl).await.status, StatusCode::FORBIDDEN);

    let refused = |body: Value| {
        let (app, owner) = (&app, &owner);
        async move {
            let response = render(app, owner, property, body).await;
            assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
            response.body["detail"].as_str().unwrap_or_default().to_owned()
        }
    };
    let vat = json!({"report": "vat", "params": {"period": "2630"}});
    assert_eq!(refused(vat.clone()).await, "This property is not VAT registered");
    set_vat(&superuser, property, None).await;
    assert_eq!(refused(vat).await, "Set the property's TIN on Settings → Taxes and invoices first");
    refused(json!({"report": "tdl", "params": {"fy_start_year": 2026, "quarter": 5}})).await;
    refused(json!({"report": "service_charge", "params": {"year": 2026, "month": 13}})).await;
    refused(json!({"report": "summary", "params": {"from": "2026-10-07", "to": "2026-10-01"}})).await;
    refused(json!({"report": "payroll", "params": {}})).await;
    let other = Uuid::now_v7();
    let missing =
        app.send(Method::GET, &format!("/api/v1/properties/{property}/reports/{other}"), Some(&owner), None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

/// Queues `hotel`'s `kind` report of the week of 19 October 2026 (or October 2026) and delivers its rendered PDF to
/// `/internal/events`, as media-svc does; returns the report.
async fn rendered(app: &TestApp, hotel: &MediaHotel, kind: ReportKind) -> Uuid {
    let tenant = TenantId(hotel.tenant);
    let (from, to) = match kind {
        ReportKind::Monthly => (date!(2026 - 10 - 01), date!(2026 - 10 - 31)),
        _ => (date!(2026 - 10 - 19), date!(2026 - 10 - 25)),
    };
    let mut tx = db::begin(&app.pool, Scope::tenant(tenant)).await.unwrap();
    let report = reports::queue_render(&mut tx, tenant, hotel.id, kind, from, to, ReportTemplate::Summary, json!({}))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    deliver(app, hotel, report).await;
    report
}

/// Delivers `report`'s rendered PDF to `/internal/events` as a new message.
async fn deliver(app: &TestApp, hotel: &MediaHotel, report: Uuid) {
    let key = storage::report_pdf_key(hotel.tenant, report);
    let result = MediaResult::ReportRendered { tenant_id: hotel.tenant, report_id: report, key };
    let body = envelope(Uuid::now_v7(), &serde_json::to_value(result).unwrap()).to_string();
    assert_eq!(push(app, &body, Some(&format!("Bearer {SECRET}"))).await, StatusCode::NO_CONTENT);
}

/// The users `report` was mailed to: its `report_mail` rows and its `mail-send` jobs, each sorted.
async fn mailed(superuser: &PgPool, report: Uuid) -> (Vec<Uuid>, Vec<Uuid>) {
    let mut recorded: Vec<Uuid> = sqlx::query_scalar("select user_id from report_mail where report_id = $1")
        .bind(report)
        .fetch_all(superuser)
        .await
        .unwrap();
    recorded.sort();
    let payloads: Vec<Value> =
        sqlx::query_scalar("select payload from outbox where topic = 'mail-send'").fetch_all(superuser).await.unwrap();
    let mut jobs: Vec<Uuid> = payloads
        .into_iter()
        .filter_map(|payload| match serde_json::from_value(payload).unwrap() {
            MailJob::Report { report_id, user_id, .. } if report_id == report => Some(user_id),
            _ => None,
        })
        .collect();
    jobs.sort();
    (recorded, jobs)
}

async fn user_id(app: &TestApp, cookie: &str) -> Uuid {
    uuid(&app.send(Method::GET, "/api/v1/me", Some(cookie), None).await.body["user_id"])
}

/// Narrows `cookie`'s user's grants in `hotel`'s tenant to `property`.
async fn scope_to(app: &TestApp, superuser: &PgPool, hotel: &MediaHotel, cookie: &str, property: Uuid) {
    sqlx::query("update role_grant set property_id = $3 where user_id = $1 and tenant_id = $2")
        .bind(user_id(app, cookie).await)
        .bind(hotel.tenant)
        .bind(property)
        .execute(superuser)
        .await
        .unwrap();
}

/// A rendered weekly report is mailed to every owner and manager of the property (granted everywhere or there), and
/// to no one else; a monthly one too, and an on-demand or liability report to no one.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ready_weekly_mails_owners_and_managers_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = internal_app(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let hotel = MediaHotel::new(&app, "owner@example.com", "GAL").await;
    let other = post(
        &app,
        &hotel.owner,
        "/api/v1/properties",
        json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"}),
    )
    .await;
    let other = uuid(&other.body["id"]);
    let everywhere = app.staff(&superuser, &hotel.owner, "manager@example.com", "manager").await;
    let here = app.staff(&superuser, &hotel.owner, "here@example.com", "manager").await;
    scope_to(&app, &superuser, &hotel, &here, hotel.id).await;
    let elsewhere = app.staff(&superuser, &hotel.owner, "kandy@example.com", "manager").await;
    scope_to(&app, &superuser, &hotel, &elsewhere, other).await;
    app.staff(&superuser, &hotel.owner, "accountant@example.com", "accountant").await;
    app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let mut expected =
        vec![user_id(&app, &hotel.owner).await, user_id(&app, &everywhere).await, user_id(&app, &here).await];
    expected.sort();

    let weekly = rendered(&app, &hotel, ReportKind::Weekly).await;
    let monthly = rendered(&app, &hotel, ReportKind::Monthly).await;
    let tdl = rendered(&app, &hotel, ReportKind::Tdl).await;

    assert_eq!(mailed(&superuser, weekly).await, (expected.clone(), expected.clone()));
    assert_eq!(mailed(&superuser, monthly).await, (expected.clone(), expected));
    assert_eq!(mailed(&superuser, tdl).await, (vec![], vec![]));
    let job: Value = sqlx::query_scalar("select payload from outbox where topic = 'mail-send' limit 1")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(job["Report"]["tenant_id"], json!(hotel.tenant));
}

async fn set_email(app: &TestApp, path: &str, cookie: &str, enabled: bool) -> common::TestResponse {
    app.send_with(Method::PUT, path, Some(cookie), Some(json!({"enabled": enabled})), &[("x-goodfolk-csrf", "1")]).await
}

async fn settings(app: &TestApp, cookie: &str, property: Uuid) -> Value {
    let query = "query ($p: UUID!) { reportEmails(propertyId: $p) { kind enabled } }";
    graphql(app, cookie, query, json!({"p": property})).await
}

/// Each person's report emails are theirs to turn off, per property and kind, with `ReportsView`; one turned off
/// is not mailed.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn opted_out_user_not_mailed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = internal_app(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let hotel = MediaHotel::new(&app, "owner@example.com", "GAL").await;
    let manager = app.staff(&superuser, &hotel.owner, "manager@example.com", "manager").await;
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let path = format!("{}/report-emails/weekly", hotel.path);

    assert_eq!(set_email(&app, &path, &desk, false).await.status, StatusCode::FORBIDDEN);
    assert!(denied(&settings(&app, &desk, hotel.id).await));
    let off = set_email(&app, &path, &manager, false).await;
    assert_eq!(off.status, StatusCode::OK, "{:?}", off.body);
    assert_eq!(off.body, json!([{"kind": "weekly", "enabled": false}, {"kind": "monthly", "enabled": true}]));
    let read = settings(&app, &manager, hotel.id).await;
    assert_eq!(
        read["data"]["reportEmails"],
        json!([{"kind": "WEEKLY", "enabled": false}, {"kind": "MONTHLY", "enabled": true}])
    );
    let owners = settings(&app, &hotel.owner, hotel.id).await;
    assert_eq!(
        owners["data"]["reportEmails"],
        json!([{"kind": "WEEKLY", "enabled": true}, {"kind": "MONTHLY", "enabled": true}])
    );
    let unknown = format!("/api/v1/properties/{}/report-emails/weekly", Uuid::now_v7());
    let unknown = set_email(&app, &unknown, &hotel.owner, false).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    let owner = user_id(&app, &hotel.owner).await;
    let mut both = vec![owner, user_id(&app, &manager).await];
    both.sort();
    let weekly = rendered(&app, &hotel, ReportKind::Weekly).await;
    let monthly = rendered(&app, &hotel, ReportKind::Monthly).await;

    assert_eq!(mailed(&superuser, weekly).await, (vec![owner], vec![owner]));
    assert_eq!(mailed(&superuser, monthly).await, (both.clone(), both));
}

/// A render result delivered again, even after the report was rendered again, mails no one twice.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn redelivered_render_result_does_not_double_mail(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = internal_app(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let hotel = MediaHotel::new(&app, "owner@example.com", "GAL").await;
    let owner = user_id(&app, &hotel.owner).await;
    let weekly = rendered(&app, &hotel, ReportKind::Weekly).await;

    deliver(&app, &hotel, weekly).await;
    sqlx::query("update report set status = 'rendering' where id = $1").bind(weekly).execute(&superuser).await.unwrap();
    deliver(&app, &hotel, weekly).await;

    assert_eq!(mailed(&superuser, weekly).await, (vec![owner], vec![owner]));
    let status: String =
        sqlx::query_scalar("select status from report where id = $1").bind(weekly).fetch_one(&superuser).await.unwrap();
    assert_eq!(status, "ready");
}
