//! The everyday reports and daily lists over GraphQL: `reportSummary` needs `ReportsView`, and `dailyList` also
//! accepts `ReservationsView`.

mod common;

use axum::http::{Method, StatusCode};
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
