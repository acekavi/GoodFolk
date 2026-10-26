//! The everyday reports and daily lists over GraphQL: `reportSummary` needs `ReportsView`, and `dailyList` also
//! accepts `ReservationsView`. The liability reports (`tdlReport`, `ssclReport`, `vatReport`, `serviceChargeReport`)
//! and the VAT schedule downloads need `ReportsLiability`.

mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, uuid};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
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
