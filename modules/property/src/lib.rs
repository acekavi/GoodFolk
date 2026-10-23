//! Properties (hotels) within a tenant.

use db::{Event, TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Property {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
    /// Local time guests may check in from, `HH:MM`.
    pub check_in_time: String,
    /// Local time guests check out by, `HH:MM`.
    pub check_out_time: String,
    /// Local time the night audit closes the business date by itself, `HH:MM`: on the date at 12:00 or later, on the
    /// next morning when earlier.
    pub audit_time: String,
    /// The property's current trading day. Set to its local today when created; moved only by night audit.
    pub business_date: Date,
    pub version: i32,
}

/// The `select` list that reads a [`Property`]. A macro, so queries can `concat!` it into a static string.
macro_rules! property_columns {
    () => {
        "id, code, name, timezone, base_currency, to_char(check_in_time, 'HH24:MI') as check_in_time, \
         to_char(check_out_time, 'HH24:MI') as check_out_time, to_char(audit_time, 'HH24:MI') as audit_time, \
         business_date, version"
    };
}

/// Settings changes; `None` leaves a field as it is. Times are `HH:MM`, validated by the caller.
#[derive(Debug, Clone, Default)]
pub struct PropertyChanges {
    pub name: Option<String>,
    pub check_in_time: Option<String>,
    pub check_out_time: Option<String>,
    pub audit_time: Option<String>,
}

pub struct NewProperty {
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PropertyError {
    #[error("a property with this code already exists")]
    CodeTaken,
    #[error("unknown time zone")]
    UnknownTimezone,
    #[error("property not found")]
    NotFound,
    #[error("the property was changed by someone else; reload and try again")]
    VersionMismatch,
    /// A business rule, such as accepted currencies that leave out the base currency.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key clients refetch when the property list changes.
pub const PROPERTIES_KEY: &str = "properties";

/// `tx` must be scoped to `tenant`.
pub async fn create_property(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    input: NewProperty,
) -> Result<Property, PropertyError> {
    let known_zone: bool = sqlx::query_scalar("select exists (select 1 from pg_timezone_names where name = $1)")
        .bind(&input.timezone)
        .fetch_one(&mut **tx)
        .await?;
    if !known_zone {
        return Err(PropertyError::UnknownTimezone);
    }
    let inserted = sqlx::query_as::<_, Property>(concat!(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, $3, $4, $5, $6, (now() at time zone $5)::date)
         returning ",
        property_columns!()
    ))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(&input.code)
    .bind(&input.name)
    .bind(&input.timezone)
    .bind(&input.base_currency)
    .fetch_one(&mut **tx)
    .await;
    let property = match inserted {
        Ok(property) => property,
        Err(err) => {
            let taken = err
                .as_database_error()
                .and_then(|db_err| db_err.constraint())
                .is_some_and(|constraint| constraint == "property_tenant_id_code_key");
            return Err(if taken { PropertyError::CodeTaken } else { err.into() });
        }
    };
    insert_preset_rules(tx, tenant, property.id).await?;
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.created', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(property.id)
    .bind(serde_json::json!({ "code": property.code, "name": property.name }))
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: None, keys: vec![PROPERTIES_KEY.into()] }).await?;
    Ok(property)
}

/// Gives a new property the Sri Lanka tax rules. `kind`, `presentation` and `condition` are stored as the serde forms
/// of the `tax` types, which is also how the folio module reads them back.
async fn insert_preset_rules(tx: &mut Tx, tenant: TenantId, property: Uuid) -> Result<(), sqlx::Error> {
    let to_text = |value: serde_json::Value| value.as_str().map(str::to_owned);
    for rule in tax::sri_lanka_preset(false) {
        sqlx::query(
            "insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence,
                                   presentation, condition, effective_from, effective_to)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(property)
        .bind(&rule.code)
        .bind(&rule.name)
        .bind(to_text(serde_json::json!(rule.kind)))
        .bind(i32::try_from(rule.rate_bp).expect("preset rates are at most 10000"))
        .bind(&rule.applies_to)
        .bind(&rule.base)
        .bind(rule.sequence)
        .bind(to_text(serde_json::json!(rule.presentation)))
        .bind(serde_json::json!(rule.condition))
        .bind(rule.effective_from)
        .bind(rule.effective_to)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Lists properties by code. `only` limits the result to those ids (for property-scoped staff).
pub async fn list_properties(tx: &mut Tx, only: Option<&[Uuid]>) -> Result<Vec<Property>, sqlx::Error> {
    sqlx::query_as(concat!(
        "select ",
        property_columns!(),
        " from property where $1::uuid[] is null or id = any($1) order by code"
    ))
    .bind(only)
    .fetch_all(&mut **tx)
    .await
}

/// Applies `changes` if the property is still at `expected_version`, and bumps the version.
/// `tx` must be scoped to `tenant`.
pub async fn update_property(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    id: Uuid,
    expected_version: i32,
    changes: PropertyChanges,
) -> Result<Property, PropertyError> {
    let updated: Option<Property> = sqlx::query_as(concat!(
        "update property set name = coalesce($3, name),
                check_in_time = coalesce($4::time, check_in_time),
                check_out_time = coalesce($5::time, check_out_time),
                audit_time = coalesce($6::time, audit_time),
                version = version + 1
         where id = $1 and version = $2
         returning ",
        property_columns!()
    ))
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(&changes.check_in_time)
    .bind(&changes.check_out_time)
    .bind(&changes.audit_time)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(property) = updated else {
        let exists: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        return Err(if exists { PropertyError::VersionMismatch } else { PropertyError::NotFound });
    };
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.updated', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(id)
    .bind(serde_json::json!({
        "name": changes.name,
        "check_in_time": changes.check_in_time,
        "check_out_time": changes.check_out_time,
        "audit_time": changes.audit_time,
    }))
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(id), keys: vec![PROPERTIES_KEY.into()] }).await?;
    Ok(property)
}

/// What a property's invoices and taxes depend on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TaxSettings {
    pub property_id: Uuid,
    /// VAT applies, and invoices are tax invoices, which need the TIN.
    pub vat_registered: bool,
    /// The taxpayer identification number printed on invoices.
    pub tin: Option<String>,
    /// The tourism development levy's small-turnover band (0.5% instead of 1%), used when the preset is applied.
    pub tdl_small_turnover: bool,
    /// The supplier's name on invoices; the property's name when `null`.
    pub legal_name: Option<String>,
    pub address: Option<String>,
    pub base_currency: String,
    /// The currencies charges and payments may be in, base currency included.
    pub accepted_currencies: Vec<String>,
    /// The property's version, shared with its other settings: send it as `If-Match`.
    pub version: i32,
}

const TAX_SETTINGS_COLUMNS: &str = "id as property_id, vat_registered, tin, tdl_small_turnover, legal_name, address, \
     base_currency::text as base_currency, accepted_currencies::text[] as accepted_currencies, version";

/// Tax settings changes; `None` leaves a field as it is, and `Some(None)` clears an optional one. Formats (TIN,
/// lengths, currency codes) are validated by the caller.
#[derive(Debug, Clone, Default)]
pub struct TaxSettingsChanges {
    pub vat_registered: Option<bool>,
    pub tin: Option<Option<String>>,
    pub tdl_small_turnover: Option<bool>,
    pub legal_name: Option<Option<String>>,
    pub address: Option<Option<String>>,
    pub accepted_currencies: Option<Vec<String>>,
}

/// The tax settings of `property`. `NotFound` if it is not in the tenant.
pub async fn tax_settings(tx: &mut Tx, property: Uuid) -> Result<TaxSettings, PropertyError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("select {TAX_SETTINGS_COLUMNS} from property where id = $1")))
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(PropertyError::NotFound)
}

/// Applies `changes` to the tax settings of `property` if it is still at `expected_version` (`NotFound`,
/// `VersionMismatch`), and bumps its version. Accepted currencies are deduplicated, keep their order and must include
/// the base currency (`Invalid`); a currency in which an open folio of the property still holds a balance can't be
/// removed (`Invalid`), or that money could never be paid or refunded. Audited as `property.tax_settings_updated` with
/// the values before and after.
pub async fn update_tax_settings(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    expected_version: i32,
    changes: TaxSettingsChanges,
) -> Result<TaxSettings, PropertyError> {
    let before: TaxSettings = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {TAX_SETTINGS_COLUMNS} from property where id = $1 for no key update"
    )))
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(PropertyError::NotFound)?;
    if before.version != expected_version {
        return Err(PropertyError::VersionMismatch);
    }
    let accepted = match changes.accepted_currencies {
        Some(currencies) => {
            let mut unique: Vec<String> = Vec::with_capacity(currencies.len());
            for currency in currencies {
                if !unique.contains(&currency) {
                    unique.push(currency);
                }
            }
            if !unique.contains(&before.base_currency) {
                return Err(PropertyError::Invalid(format!(
                    "the accepted currencies must include the base currency, {}",
                    before.base_currency
                )));
            }
            unique
        }
        None => before.accepted_currencies.clone(),
    };
    let removed: Vec<String> =
        before.accepted_currencies.iter().filter(|currency| !accepted.contains(currency)).cloned().collect();
    if !removed.is_empty() {
        let held: Vec<String> = sqlx::query_scalar(
            "select currency from (
               select l.currency::text as currency, l.folio_id, l.total as amount from folio_line l
               join folio f on f.id = l.folio_id
               where f.property_id = $1 and f.status = 'open' and l.currency::text = any($2)
               union all
               select p.currency::text, p.folio_id, -p.amount from payment p
               join folio f on f.id = p.folio_id
               where f.property_id = $1 and f.status = 'open' and p.currency::text = any($2)) as entries
             group by currency, folio_id
             having sum(amount) <> 0
             order by currency",
        )
        .bind(property)
        .bind(&removed)
        .fetch_all(&mut **tx)
        .await?;
        if let Some(currency) = held.first() {
            return Err(PropertyError::Invalid(format!(
                "an open folio still holds a {currency} balance; settle it before {currency} stops being accepted"
            )));
        }
    }
    let after = TaxSettings {
        vat_registered: changes.vat_registered.unwrap_or(before.vat_registered),
        tin: changes.tin.unwrap_or_else(|| before.tin.clone()),
        tdl_small_turnover: changes.tdl_small_turnover.unwrap_or(before.tdl_small_turnover),
        legal_name: changes.legal_name.unwrap_or_else(|| before.legal_name.clone()),
        address: changes.address.unwrap_or_else(|| before.address.clone()),
        accepted_currencies: accepted,
        version: before.version + 1,
        ..before.clone()
    };
    let updated: TaxSettings = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update property set vat_registered = $2, tin = $3, tdl_small_turnover = $4, legal_name = $5, address = $6,
                accepted_currencies = $7::text[]::char(3)[], version = version + 1
         where id = $1
         returning {TAX_SETTINGS_COLUMNS}"
    )))
    .bind(property)
    .bind(after.vat_registered)
    .bind(&after.tin)
    .bind(after.tdl_small_turnover)
    .bind(&after.legal_name)
    .bind(&after.address)
    .bind(&after.accepted_currencies)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "before": settings_audit(&before), "after": settings_audit(&updated) });
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, 'property.tax_settings_updated', 'property', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(property)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![PROPERTIES_KEY.into()] })
        .await?;
    Ok(updated)
}

fn settings_audit(settings: &TaxSettings) -> serde_json::Value {
    serde_json::json!({
        "vat_registered": settings.vat_registered,
        "tin": settings.tin,
        "tdl_small_turnover": settings.tdl_small_turnover,
        "legal_name": settings.legal_name,
        "address": settings.address,
        "accepted_currencies": settings.accepted_currencies,
    })
}
