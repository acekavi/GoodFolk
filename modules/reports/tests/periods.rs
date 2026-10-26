//! The periods the close renders (`reports::due_periods`, `reports::render_due`): each weekly and monthly report, TDL
//! and SSCL quarter, VAT return period and service charge month whose last day is closed, queued once.

mod common;

use common::{Hotel, date};
use reports::ReportKind;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;

impl Hotel {
    async fn render_due(&self, closed_through: Date) -> usize {
        let mut tx = self.tx().await;
        let queued = reports::render_due(&mut tx, self.tenant, self.property, closed_through).await.unwrap();
        tx.commit().await.unwrap();
        queued
    }

    async fn due(&self, closed_through: Date) -> Vec<(ReportKind, Date, Date)> {
        reports::due_periods(&mut self.tx().await, self.property, closed_through).await.unwrap()
    }

    /// `(kind, period_start, period_end, status)` of each stored report, by kind and start.
    async fn reports(&self) -> Vec<(String, Date, Date, String)> {
        sqlx::query_as("select kind, period_start, period_end, status from report order by kind, period_start")
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn register_vat(&self, period: &str) {
        let mut tx = self.tx().await;
        sqlx::query(
            "update property set vat_registered = true, tin = '300767028', vat_return_period = $2 where id = $1",
        )
        .bind(self.property)
        .bind(period)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn row(kind: &str, from: Date, to: Date) -> (String, Date, Date, String) {
    (kind.to_owned(), from, to, "rendering".to_owned())
}

async fn render_jobs(superuser: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from outbox where topic = 'report-render'").fetch_one(superuser).await.unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sunday_close_queues_weekly(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2026, 10, 26)).await;
    hotel.close_days(date(2026, 10, 19), date(2026, 10, 25), 5, 50_000).await;

    assert_eq!(hotel.due(date(2026, 10, 24)).await, [], "Saturday ends nothing");
    assert_eq!(hotel.render_due(date(2026, 10, 25)).await, 1);

    assert_eq!(hotel.reports().await, [row("weekly", date(2026, 10, 19), date(2026, 10, 25))]);
    assert_eq!(render_jobs(&superuser).await, 1);
    let data: serde_json::Value =
        sqlx::query_scalar("select data from report").fetch_one(&mut *hotel.tx().await).await.unwrap();
    assert_eq!(data["template"], "summary");
    assert_eq!(data["data"]["title"], "Weekly report");
    assert_eq!(
        data["data"]["headline"][0],
        serde_json::json!({"label": "Occupancy", "value": "50.0%", "last_year": null})
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn month_end_queues_monthly_sscl_sc_and_vat_when_monthly(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_vat("monthly").await;
    hotel.set_business_date(date(2026, 10, 1)).await;
    hotel.close_days(date(2026, 9, 28), date(2026, 9, 30), 5, 50_000).await;

    assert_eq!(hotel.render_due(date(2026, 9, 30)).await, 5);

    let (july, september, end) = (date(2026, 7, 1), date(2026, 9, 1), date(2026, 9, 30));
    assert_eq!(
        hotel.reports().await,
        [
            row("monthly", september, end),
            row("service_charge", september, end),
            row("sscl", july, end),
            row("tdl", july, end),
            row("vat", september, end),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn quarter_end_march_queues_tdl_q4(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2027, 4, 1)).await;
    hotel.close_days(date(2027, 3, 29), date(2027, 3, 31), 5, 50_000).await;

    assert_eq!(hotel.render_due(date(2027, 3, 31)).await, 4, "not VAT registered: no VAT period");

    let (january, march, end) = (date(2027, 1, 1), date(2027, 3, 1), date(2027, 3, 31));
    assert_eq!(
        hotel.reports().await,
        [
            row("monthly", march, end),
            row("service_charge", march, end),
            row("sscl", january, end),
            row("tdl", january, end),
        ]
    );
    let tdl: serde_json::Value = sqlx::query_scalar("select data from report where kind = 'tdl'")
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(tdl["data"]["period"], "January – March (2027)");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn catch_up_over_week_and_month_queues_both_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts).await;
    hotel.register_vat("quarterly").await;
    hotel.set_business_date(date(2026, 11, 2)).await;
    // Closed through Sunday 1 November in one go: October and the week of 26 October both end.
    hotel.close_days(date(2026, 10, 26), date(2026, 11, 1), 5, 50_000).await;

    assert_eq!(hotel.render_due(date(2026, 11, 1)).await, 3);
    assert_eq!(hotel.render_due(date(2026, 11, 1)).await, 0, "a rerun queues nothing");

    assert_eq!(
        hotel.reports().await,
        [
            row("monthly", date(2026, 10, 1), date(2026, 10, 31)),
            row("service_charge", date(2026, 10, 1), date(2026, 10, 31)),
            row("weekly", date(2026, 10, 26), date(2026, 11, 1)),
        ]
    );
    assert_eq!(render_jobs(&superuser).await, 3);
}

/// A report media-svc could not render is queued again by the next close, with fresh data, and once none of the
/// property's reports has failed the `reports` alert clears.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_failed_report_is_queued_again(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2026, 10, 27)).await;
    hotel.close_days(date(2026, 10, 19), date(2026, 10, 25), 5, 50_000).await;
    hotel.render_due(date(2026, 10, 25)).await;
    let report: uuid::Uuid =
        sqlx::query_scalar("select id from report").fetch_one(&mut *hotel.tx().await).await.unwrap();
    let mut tx = hotel.tx().await;
    reports::report_failed(&mut tx, hotel.tenant, report, "laying out the report failed").await.unwrap();
    tx.commit().await.unwrap();
    hotel.close_day(date(2026, 10, 26), 10, 5, 50_000, vec![]).await;

    assert_eq!(hotel.due(date(2026, 10, 26)).await, [(ReportKind::Weekly, date(2026, 10, 19), date(2026, 10, 25))]);
    assert_eq!(hotel.render_due(date(2026, 10, 26)).await, 1);

    assert_eq!(hotel.reports().await, [row("weekly", date(2026, 10, 19), date(2026, 10, 25))]);
    let (id, error): (uuid::Uuid, Option<String>) =
        sqlx::query_as("select id, error from report").fetch_one(&mut *hotel.tx().await).await.unwrap();
    assert_eq!((id, error), (report, None), "the same report, rendering again");
    assert_eq!(render_jobs(&superuser).await, 2);
    assert!(audit::alerts::list(&mut hotel.tx().await, hotel.property).await.unwrap().is_empty());
}

/// The scan reaches back 13 months at most, and never before the property's first closed day.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_scan_reaches_back_13_months(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    hotel.set_business_date(date(2026, 10, 26)).await;
    hotel.close_day(date(2024, 6, 5), 10, 5, 50_000, vec![]).await;
    hotel.close_day(date(2026, 10, 25), 10, 5, 50_000, vec![]).await;

    let due = hotel.due(date(2026, 10, 25)).await;
    let weeks: Vec<(Date, Date)> =
        due.iter().filter(|(kind, ..)| *kind == ReportKind::Weekly).map(|(_, from, to)| (*from, *to)).collect();
    // 25 September 2025 is 13 months back: the first week ending since is 22–28 September.
    assert_eq!(weeks.first(), Some(&(date(2025, 9, 22), date(2025, 9, 28))));
    assert_eq!(weeks.last(), Some(&(date(2026, 10, 19), date(2026, 10, 25))));
    assert_eq!(weeks.len(), 57);
    let months = due.iter().filter(|(kind, ..)| *kind == ReportKind::Monthly).count();
    assert_eq!(months, 13, "September 2025 to September 2026");

    let fresh = Hotel::new(opts).await;
    fresh.set_business_date(date(2026, 10, 26)).await;
    fresh.close_day(date(2026, 10, 21), 10, 5, 50_000, vec![]).await;
    assert_eq!(fresh.due(date(2026, 10, 25)).await, [(ReportKind::Weekly, date(2026, 10, 19), date(2026, 10, 25))]);
}
