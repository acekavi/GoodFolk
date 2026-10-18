//! Exchange rates to the property's base currency: lookup, manual overrides, and the daily import from the Central
//! Bank of Sri Lanka.
//!
//! Rates are `numeric(18,6)`: base currency per one unit of the currency. Converting an amount is exact decimal
//! arithmetic, never floating point.

mod cbsl;

pub use cbsl::{Cbsl, parse_cbsl};

use async_trait::async_trait;
use db::{Event, Scope, TenantId, Tx, UserId};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::PgPool;
use std::sync::Once;
use time::Date;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum FxError {
    /// The property does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// A business rule, such as an override for a currency the property does not accept.
    #[error("{0}")]
    Invalid(String),
    /// The rate source's page no longer looks the way the parser expects.
    #[error("the rate source's layout changed: {0}")]
    Layout(String),
    /// The rate source could not be reached or answered with an error.
    #[error("the rate source failed: {0}")]
    Source(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A rate in force on a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub rate: Decimal,
    /// The date the rate is stored under: the day it was imported or overridden for.
    pub date: Date,
    /// The date the source published it, which is earlier than `date` over a weekend or a holiday.
    pub source_date: Date,
}

/// Cache key for a property's exchange rates.
pub fn fx_key(property: Uuid) -> String {
    format!("fx:{property}")
}

pub(crate) fn install_tls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// ISO 4217 minor units: 0 for the currencies without a minor unit, 3 for the ones with thousandths, else 2.
pub fn exponent(currency: &str) -> u32 {
    match currency {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX" | "VND" | "VUV"
        | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

/// `amount` minor units of a currency (with `currency_exponent` decimals) in base minor units (with `base_exponent`
/// decimals) at `rate`, rounded half away from zero. An amount the result cannot hold, or an exponent above 18, is an
/// error: money is never clamped.
pub fn to_base(amount: i64, currency_exponent: u32, base_exponent: u32, rate: Decimal) -> Result<i64, FxError> {
    let too_large = || FxError::Invalid("amount too large to convert".into());
    if currency_exponent > 18 || base_exponent > 18 {
        return Err(too_large());
    }
    Decimal::new(amount, currency_exponent)
        .checked_mul(rate)
        .and_then(|major| major.checked_mul(Decimal::from(10u64.pow(base_exponent))))
        .and_then(|value| value.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero).to_i64())
        .ok_or_else(too_large)
}

/// The rate to convert `currency` to the property's base currency on `on`: 1 for the base currency itself, else the
/// latest stored rate dated on or before `on`. `None` when there is none; the caller refuses the posting (409).
pub async fn rate_for(tx: &mut Tx, property: Uuid, currency: &str, on: Date) -> Result<Option<Rate>, FxError> {
    let base: Option<String> = sqlx::query_scalar("select base_currency::text from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?;
    let base = base.ok_or(FxError::NotFound("property"))?;
    if base == currency {
        return Ok(Some(Rate { rate: Decimal::ONE, date: on, source_date: on }));
    }
    let row: Option<(Decimal, Date, Date)> = sqlx::query_as(
        "select rate_to_base, date, source_date from fx_rate
         where property_id = $1 and currency = $2 and date <= $3
         order by date desc limit 1",
    )
    .bind(property)
    .bind(currency)
    .bind(on)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(rate, date, source_date)| Rate { rate, date, source_date }))
}

/// One stored rate, as the exchange-rate settings list it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RateRow {
    pub date: Date,
    pub currency: String,
    /// Base currency per one unit of `currency`.
    pub rate: Decimal,
    /// `cbsl` or `manual`.
    pub source: String,
    pub source_date: Date,
    /// Who overrode it, for a manual rate.
    pub set_by: Option<Uuid>,
    pub set_at: time::OffsetDateTime,
}

/// The property's stored rates dated in `[from, to)`, by date then currency.
pub async fn list_rates(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Vec<RateRow>, sqlx::Error> {
    sqlx::query_as(
        "select date, currency::text as currency, rate_to_base as rate, source, source_date, set_by, set_at
         from fx_rate where property_id = $1 and date >= $2 and date < $3
         order by date, currency",
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}

/// Why the last import left the property without complete, current rates.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Alert {
    pub message: String,
    pub raised_at: time::OffsetDateTime,
}

/// The property's exchange-rate alert, until an import brings every rate it needs.
pub async fn alert(tx: &mut Tx, property: Uuid) -> Result<Option<Alert>, sqlx::Error> {
    sqlx::query_as("select message, raised_at from fx_alert where property_id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await
}

/// Overrides the rate of `currency` for `date`, for a staff member who knows better than the bank. Replaces an
/// imported or earlier manual rate for that day, and a later import does not undo it. Audited as `fx.overridden`.
/// Clears the property's `fx_alert` once every accepted currency other than the base has a rate dated today in the
/// property's time zone, as a complete import would.
pub async fn set_manual(
    tx: &mut Tx,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    date: Date,
    currency: &str,
    rate: Decimal,
) -> Result<(), FxError> {
    if rate <= Decimal::ZERO || rate.normalize().scale() > 6 || rate >= Decimal::from(10u64.pow(12)) {
        return Err(FxError::Invalid(
            "the rate must be positive, with at most 6 decimals and 12 digits before them".into(),
        ));
    }
    let row: Option<(String, Vec<String>)> =
        sqlx::query_as("select base_currency::text, accepted_currencies::text[] from property where id = $1")
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let (base, accepted) = row.ok_or(FxError::NotFound("property"))?;
    if currency == base {
        return Err(FxError::Invalid(format!("{currency} is the base currency; its rate is always 1")));
    }
    if !accepted.iter().any(|c| c == currency) {
        return Err(FxError::Invalid(format!("{currency} is not an accepted currency of this property")));
    }
    let previous: Option<Decimal> =
        sqlx::query_scalar("select rate_to_base from fx_rate where property_id = $1 and date = $2 and currency = $3")
            .bind(property)
            .bind(date)
            .bind(currency)
            .fetch_optional(&mut **tx)
            .await?;
    sqlx::query(
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date, set_by)
         values ($1, $2, $3, $4, $5, 'manual', $3, $6)
         on conflict (property_id, date, currency) do update
           set rate_to_base = excluded.rate_to_base, source = 'manual', source_date = excluded.source_date,
               set_by = excluded.set_by, set_at = now()",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(date)
    .bind(currency)
    .bind(rate)
    .bind(user.0)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'fx.overridden', 'fx_rate', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(user.0)
    .bind(property)
    .bind(serde_json::json!({
        "date": date.to_string(),
        "currency": currency,
        "rate": rate.to_string(),
        "previous": previous.map(|p| p.normalize().to_string()),
    }))
    .execute(&mut **tx)
    .await?;
    // The import's alert is about today's rates: once every accepted currency but the base has one dated today (at
    // the property), nothing is missing any more.
    sqlx::query(
        "delete from fx_alert a using property p
         where a.property_id = $1 and p.id = $1
           and not exists (
             select 1 from unnest(p.accepted_currencies) as c(currency)
             where c.currency <> p.base_currency
               and not exists (select 1 from fx_rate r
                               where r.property_id = $1 and r.currency = c.currency
                                 and r.date = (now() at time zone p.timezone)::date))",
    )
    .bind(property)
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![fx_key(property)] }).await?;
    Ok(())
}

/// Where selling rates come from.
#[async_trait]
pub trait RateSource: Send + Sync {
    /// For each of `currencies` that the source has, the newest selling rate dated on or before `on`, as
    /// `(currency, the date the source published it, rate)`. A currency the source lacks is left out.
    async fn tt_selling(&self, currencies: &[String], on: Date) -> Result<Vec<(String, Date, Decimal)>, FxError>;
}

/// What one import did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Properties whose base currency the source quotes against.
    pub properties: u32,
    /// Rates written (a manual override for the day is kept and not counted).
    pub stored: u32,
    /// Properties left with an alert: the source failed or lacked a currency.
    pub alerted: u32,
}

/// Fetches today's rates for every property and stores them under `today`, with the source's own date as
/// `source_date`. A property whose source fails or lacks one of its currencies gets an `fx_alert` row (and an error
/// in the log) that the next complete import clears. Only properties with base currency LKR are served: the source
/// quotes against the rupee. Each property is written in its own transaction scoped to its tenant, so a failure for
/// one leaves the rest alone.
pub async fn import(pool: &PgPool, source: &dyn RateSource, today: Date) -> ImportReport {
    let mut report = ImportReport::default();
    let targets: Vec<(Uuid, Uuid, String, Vec<String>)> = match sqlx::query_as(
        "select tenant_id, property_id, base_currency, accepted_currencies from app.fx_import_targets()",
    )
    .fetch_all(pool)
    .await
    {
        Ok(targets) => targets,
        Err(error) => {
            tracing::error!(%error, "fx import: listing properties failed");
            return report;
        }
    };
    let targets: Vec<(Uuid, Uuid, Vec<String>)> = targets
        .into_iter()
        .filter(|(_, _, base, _)| base == "LKR")
        .map(|(tenant, property, base, accepted)| {
            (tenant, property, accepted.into_iter().filter(|c| *c != base).collect())
        })
        .collect();
    // One request for every currency any property wants; each property then takes its own.
    let mut union: Vec<String> = targets.iter().flat_map(|(_, _, wanted)| wanted.iter().cloned()).collect();
    union.sort();
    union.dedup();
    let fetched = if union.is_empty() { Ok(Vec::new()) } else { source.tt_selling(&union, today).await };
    let fetched = fetched.map_err(|error| {
        tracing::error!(%error, "fx import: the rate source failed");
        alert_message(&error)
    });
    for (tenant, property, wanted) in targets {
        report.properties += 1;
        let all = match &fetched {
            Ok(all) => all,
            Err(message) => {
                report.alerted += 1;
                if let Err(error) = raise_alert(pool, TenantId(tenant), property, message).await {
                    tracing::error!(%error, %property, "fx import: raising the alert failed");
                }
                continue;
            }
        };
        match import_one(pool, all, today, TenantId(tenant), property, &wanted).await {
            Ok((stored, alerted)) => {
                report.stored += stored;
                report.alerted += u32::from(alerted);
            }
            Err(error) => {
                tracing::error!(%error, %property, "fx import: storing the rates failed");
                report.alerted += 1;
                if let Err(error) = raise_alert(pool, TenantId(tenant), property, &alert_message(&error)).await {
                    tracing::error!(%error, %property, "fx import: raising the alert failed");
                }
            }
        }
    }
    report
}

/// What staff see: the cause, except for database errors, whose detail stays in the log.
fn alert_message(error: &FxError) -> String {
    match error {
        FxError::Database(_) => "Exchange-rate import failed; see the logs".to_owned(),
        other => other.to_string(),
    }
}

/// A rate older than this many days at import time is stored but raises the alert: the bank has stopped publishing it.
const STALE_DAYS: i64 = 5;

/// Stores the rates of one property from the rates fetched for all properties and clears or raises its alert, returning how
/// many rates it stored and whether an alert was raised (a currency missing or stale). Rates found are stored even
/// when others are missing.
async fn import_one(
    pool: &PgPool,
    all: &[(String, Date, Decimal)],
    today: Date,
    tenant: TenantId,
    property: Uuid,
    wanted: &[String],
) -> Result<(u32, bool), FxError> {
    let found: Vec<&(String, Date, Decimal)> = all.iter().filter(|(c, _, _)| wanted.contains(c)).collect();
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    let mut stored = 0;
    for (currency, source_date, rate) in &found {
        let written = sqlx::query(
            "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
             values ($1, $2, $3, $4, $5, 'cbsl', $6)
             on conflict (property_id, date, currency) do update
               set rate_to_base = excluded.rate_to_base, source_date = excluded.source_date, set_at = now()
               where fx_rate.source = 'cbsl'",
        )
        .bind(tenant.0)
        .bind(property)
        .bind(today)
        .bind(currency)
        .bind(rate.round_dp(6))
        .bind(source_date)
        .execute(&mut *tx)
        .await?;
        stored += u32::try_from(written.rows_affected()).unwrap_or_default();
    }
    let mut problems: Vec<String> = Vec::new();
    let missing: Vec<&str> =
        wanted.iter().filter(|c| !found.iter().any(|(f, _, _)| f == *c)).map(String::as_str).collect();
    if !missing.is_empty() {
        problems.push(format!("The rate source has no rate for {}", missing.join(", ")));
    }
    for (currency, source_date, _) in &found {
        if (today - *source_date).whole_days() > STALE_DAYS {
            problems.push(format!("Central Bank rate for {currency} is from {source_date}"));
        }
    }
    if problems.is_empty() {
        sqlx::query("delete from fx_alert where property_id = $1").bind(property).execute(&mut *tx).await?;
    } else {
        tracing::error!(%property, ?problems, "fx import: the rates are incomplete or stale");
        upsert_alert(&mut tx, tenant, property, &problems.join("; ")).await?;
    }
    db::notify(&mut tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![fx_key(property)] })
        .await?;
    tx.commit().await?;
    Ok((stored, !problems.is_empty()))
}

async fn raise_alert(pool: &PgPool, tenant: TenantId, property: Uuid, message: &str) -> Result<(), sqlx::Error> {
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    upsert_alert(&mut tx, tenant, property, message).await?;
    db::notify(&mut tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![fx_key(property)] })
        .await?;
    tx.commit().await
}

async fn upsert_alert(tx: &mut Tx, tenant: TenantId, property: Uuid, message: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into fx_alert (property_id, tenant_id, message) values ($1, $2, $3)
         on conflict (property_id) do update set message = excluded.message, raised_at = now()",
    )
    .bind(property)
    .bind(tenant.0)
    .bind(message)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
