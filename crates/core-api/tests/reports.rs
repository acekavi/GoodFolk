//! The everyday reports and daily lists over GraphQL: `reportSummary` needs `ReportsView`, and `dailyList` also
//! accepts `ReservationsView`. The liability reports (`tdlReport`, `ssclReport`, `vatReport`, `serviceChargeReport`)
//! and the VAT schedule downloads need `ReportsLiability`. Every report renders as a PDF, on demand or queued by the
//! close, through the in-process media-svc.

mod common;

use axum::http::{Method, StatusCode, header};
use common::media::{Hotel as MediaHotel, http, post, stack};
use common::{TestApp, uuid};
use db::{Scope, TenantId};
use media_types::ReportTemplate;
use reports::ReportKind;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
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
