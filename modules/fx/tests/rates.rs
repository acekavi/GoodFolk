mod common;

use async_trait::async_trait;
use common::Hotel;
use fx::{FxError, RateSource, exponent, import, rate_for, set_manual, to_base};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use time::Date;
use time::macros::date;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Answers with fixed rates, and records what it was asked for.
struct Fake {
    rates: Vec<(&'static str, Date, &'static str)>,
    fail: bool,
    calls: AtomicUsize,
}

#[async_trait]
impl RateSource for Fake {
    async fn tt_selling(&self, currencies: &[String], _on: Date) -> Result<Vec<(String, Date, Decimal)>, FxError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(FxError::Layout("the columns changed".into()));
        }
        Ok(self
            .rates
            .iter()
            .filter(|(c, _, _)| currencies.iter().any(|w| w == c))
            .map(|(c, d, r)| ((*c).to_owned(), *d, dec(r)))
            .collect())
    }
}

fn full(source_date: Date) -> Fake {
    Fake {
        rates: vec![
            ("USD", source_date, "335.6485"),
            ("EUR", source_date, "377.9703"),
            ("GBP", source_date, "447.0403"),
        ],
        fail: false,
        calls: AtomicUsize::new(0),
    }
}

#[test]
fn to_base_converts_minor_units_exactly_and_rounds_half_up() {
    // USD 100.00 at 335.6485 is LKR 33,564.85.
    assert_eq!(to_base(10_000, 2, 2, dec("335.6485")).unwrap(), 3_356_485);
    // JPY has no minor unit: 1,000 yen at 2.1344 is LKR 2,134.40.
    assert_eq!(to_base(1_000, 0, 2, dec("2.1344")).unwrap(), 213_440);
    // A base currency without a minor unit: USD 1.00 at 335.6485 is 336 whole units.
    assert_eq!(to_base(100, 2, 0, dec("335.6485")).unwrap(), 336);
    // Exactly half rounds up: 0.5 cents.
    assert_eq!(to_base(1, 2, 2, dec("0.5")).unwrap(), 1);
    assert_eq!(to_base(1, 2, 2, dec("0.49")).unwrap(), 0);
    // A refund mirrors the charge.
    assert_eq!(to_base(-1, 2, 2, dec("0.5")).unwrap(), -1);
    assert_eq!(to_base(-10_000, 2, 2, dec("335.6485")).unwrap(), -3_356_485);
    // Three-decimal currencies: KWD 1.000 at 1,090.5 is LKR 1,090.50.
    assert_eq!(to_base(1_000, 3, 2, dec("1090.5")).unwrap(), 109_050);
    assert!(to_base(i64::MAX, 2, 2, dec("999999999999")).is_err());
    assert!(to_base(i64::MIN, 2, 2, dec("999999999999")).is_err());
    assert!(to_base(1, 19, 2, dec("1")).is_err());
    assert!(to_base(1, 2, 19, dec("1")).is_err());
}

#[test]
fn exponents_follow_iso_4217() {
    assert_eq!(exponent("JPY"), 0);
    assert_eq!(exponent("KWD"), 3);
    assert_eq!(exponent("BHD"), 3);
    assert_eq!(exponent("OMR"), 3);
    assert_eq!(exponent("USD"), 2);
    assert_eq!(exponent("LKR"), 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_base_currency_is_always_one(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let mut tx = h.tx().await;
    let on = date!(2026 - 10 - 16);
    let rate = rate_for(&mut tx, h.property, "LKR", on).await.unwrap().unwrap();
    assert_eq!((rate.rate, rate.date, rate.source_date), (Decimal::ONE, on, on));
    assert!(rate_for(&mut tx, h.property, "USD", on).await.unwrap().is_none());
    assert!(matches!(rate_for(&mut tx, uuid::Uuid::now_v7(), "USD", on).await, Err(FxError::NotFound(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rate_falls_back_to_the_latest_earlier_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    for (day, rate) in [(date!(2026 - 10 - 10), "330"), (date!(2026 - 10 - 14), "335")] {
        let mut tx = h.tx().await;
        set_manual(&mut tx, h.tenant, h.user, h.property, day, "USD", dec(rate)).await.unwrap();
        tx.commit().await.unwrap();
    }
    let h = &h;
    let on = |d| async move { rate_for(&mut h.tx().await, h.property, "USD", d).await.unwrap() };
    assert_eq!(on(date!(2026 - 10 - 09)).await, None);
    let r = on(date!(2026 - 10 - 12)).await.unwrap();
    assert_eq!((r.rate, r.date), (dec("330"), date!(2026 - 10 - 10)));
    let r = on(date!(2026 - 10 - 20)).await.unwrap();
    assert_eq!((r.rate, r.date), (dec("335"), date!(2026 - 10 - 14)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_manual_override_is_audited_and_validated(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let day = date!(2026 - 10 - 16);
    let mut tx = h.tx().await;
    set_manual(&mut tx, h.tenant, h.user, h.property, day, "USD", dec("340.5")).await.unwrap();
    set_manual(&mut tx, h.tenant, h.user, h.property, day, "USD", dec("341.25")).await.unwrap();
    for (currency, rate) in [("USD", "0"), ("USD", "-1"), ("USD", "1.1234567"), ("LKR", "1"), ("JPY", "2.1")] {
        let r = set_manual(&mut tx, h.tenant, h.user, h.property, day, currency, dec(rate)).await;
        assert!(matches!(r, Err(FxError::Invalid(_))), "{currency} {rate}");
    }
    let audited: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'fx.overridden'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(audited, 2);
    let data: Vec<serde_json::Value> =
        sqlx::query_scalar("select data from audit_log where action = 'fx.overridden' order by id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(data[0]["previous"], serde_json::Value::Null);
    assert_eq!(data[1]["previous"], "340.5");
    tx.commit().await.unwrap();
    let stored = h.stored("USD").await;
    assert_eq!(stored, vec![(day, day, dec("341.25"), "manual".to_owned())]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_import_stores_rates_and_clears_the_alert(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let today = date!(2026 - 10 - 16);
    {
        let mut tx = h.tx().await;
        sqlx::query("insert into fx_alert (property_id, tenant_id, message) values ($1, $2, 'old')")
            .bind(h.property)
            .bind(h.tenant.0)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let report = import(&h.pool, &full(today), today).await;
    assert_eq!((report.properties, report.stored, report.alerted), (1, 3, 0));
    assert_eq!(h.alert().await, None);
    assert_eq!(h.stored("USD").await, vec![(today, today, dec("335.6485"), "cbsl".to_owned())]);
    let mut tx = h.tx().await;
    let rate = rate_for(&mut tx, h.property, "EUR", today).await.unwrap().unwrap();
    assert_eq!(rate.rate, dec("377.9703"));
    // Running it again changes nothing.
    let again = import(&h.pool, &full(today), today).await;
    assert_eq!(again.alerted, 0);
    assert_eq!(h.stored("USD").await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_weekend_keeps_the_banks_own_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let friday = date!(2026 - 10 - 16);
    let sunday = date!(2026 - 10 - 18);
    import(&h.pool, &full(friday), sunday).await;
    assert_eq!(h.stored("GBP").await, vec![(sunday, friday, dec("447.0403"), "cbsl".to_owned())]);
    let mut tx = h.tx().await;
    let rate = rate_for(&mut tx, h.property, "GBP", sunday).await.unwrap().unwrap();
    assert_eq!((rate.date, rate.source_date), (sunday, friday));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_failing_source_raises_the_alert_and_stores_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let today = date!(2026 - 10 - 16);
    let report = import(&h.pool, &Fake { rates: vec![], fail: true, calls: AtomicUsize::new(0) }, today).await;
    assert_eq!((report.stored, report.alerted), (0, 1));
    assert!(h.alert().await.unwrap().contains("layout"));
    assert!(h.stored("USD").await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_missing_currency_raises_the_alert_but_keeps_the_rest(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let today = date!(2026 - 10 - 16);
    let source = Fake {
        rates: vec![("USD", today, "335.6485"), ("EUR", today, "377.9703")],
        fail: false,
        calls: AtomicUsize::new(0),
    };
    let report = import(&h.pool, &source, today).await;
    assert_eq!((report.stored, report.alerted), (2, 1));
    assert!(h.alert().await.unwrap().contains("GBP"));
    // The next complete import clears it.
    import(&h.pool, &full(today), today).await;
    assert_eq!(h.alert().await, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_import_never_overwrites_a_manual_override(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let today = date!(2026 - 10 - 16);
    let mut tx = h.tx().await;
    set_manual(&mut tx, h.tenant, h.user, h.property, today, "USD", dec("340")).await.unwrap();
    tx.commit().await.unwrap();
    let report = import(&h.pool, &full(today), today).await;
    assert_eq!(report.stored, 2);
    assert_eq!(h.stored("USD").await, vec![(today, today, dec("340"), "manual".to_owned())]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_import_serves_every_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let first = Hotel::new(opts).await;
    let second = Hotel::in_pool(first.pool.clone()).await;
    let today = date!(2026 - 10 - 16);
    let report = import(&first.pool, &full(today), today).await;
    assert_eq!((report.properties, report.stored), (2, 6));
    assert_eq!(first.stored("USD").await.len(), 1);
    assert_eq!(second.stored("USD").await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_source_is_asked_once_for_all_properties(_: PgPoolOptions, opts: PgConnectOptions) {
    let first = Hotel::new(opts).await;
    let second = Hotel::in_pool(first.pool.clone()).await;
    let today = date!(2026 - 10 - 16);
    let source = full(today);
    import(&first.pool, &source, today).await;
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(second.stored("EUR").await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_rate_is_stored_but_alerts(_: PgPoolOptions, opts: PgConnectOptions) {
    let h = Hotel::new(opts).await;
    let today = date!(2026 - 10 - 16);
    let old = date!(2026 - 10 - 09);
    let source = Fake {
        rates: vec![("USD", old, "335.6485"), ("EUR", today, "377.9703"), ("GBP", today, "447.0403")],
        fail: false,
        calls: AtomicUsize::new(0),
    };
    let report = import(&h.pool, &source, today).await;
    assert_eq!((report.stored, report.alerted), (3, 1));
    assert_eq!(h.alert().await.unwrap(), "Central Bank rate for USD is from 2026-10-09");
    assert_eq!(h.stored("USD").await.len(), 1);
}
