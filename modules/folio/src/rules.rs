//! A property's tax rules as stored in `tax_rule`, and the settings commands that change them.
//!
//! A rule is never deleted while it has been in force: a change of rate from a date is the current rule ended that
//! day plus a new one from it. Every change is checked with [`tax::validate`] against the property's whole resulting
//! rule set (the set every posting is calculated with), and refused as `Invalid` if the set would not validate.
//! Writers queue on the property row (`for no key update`), so two concurrent changes can't each pass validation
//! alone and leave an invalid set together.

use crate::lines::{business_date, kind_str, presentation_str};
use crate::{FolioError, audit, valid_category};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

/// The longest rule name, in characters.
const MAX_RULE_NAME: usize = 100;

/// The most categories a rule applies to, and the most parts of its base.
const MAX_RULE_PARTS: usize = 20;

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: Uuid,
    code: String,
    name: String,
    kind: String,
    rate_bp: i32,
    applies_to: Vec<String>,
    base: Vec<String>,
    sequence: i32,
    presentation: String,
    condition: Json<serde_json::Value>,
    effective_from: Date,
    effective_to: Option<Date>,
    version: i32,
}

const RULE_COLUMNS: &str = "id, code, name, kind, rate_bp, applies_to, base, sequence, presentation, condition, \
     effective_from, effective_to, version";

/// The column values are the serde names of the `tax` enums, so a row converts through JSON: an unknown kind,
/// presentation or condition fails as a decode error instead of being guessed at.
fn into_rule(row: RuleRow) -> Result<tax::Rule, sqlx::Error> {
    let decode = |err: serde_json::Error| sqlx::Error::Decode(Box::new(err));
    Ok(tax::Rule {
        code: row.code,
        name: row.name,
        kind: serde_json::from_value(row.kind.into()).map_err(decode)?,
        rate_bp: u32::try_from(row.rate_bp).map_err(|err| sqlx::Error::Decode(Box::new(err)))?,
        applies_to: row.applies_to,
        base: row.base,
        sequence: row.sequence,
        presentation: serde_json::from_value(row.presentation.into()).map_err(decode)?,
        condition: serde_json::from_value(row.condition.0).map_err(decode)?,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
    })
}

/// When a rule applies: always, only for a VAT-registered property, only for stays on plans sold to residents (or
/// non-residents), or only for charges in one of `currencies`.
// The API's schema of `tax::Condition`, which stays free of API types; the same JSON deserializes as one (tested).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schema(as = TaxCondition)]
pub enum TaxConditionSchema {
    Always,
    VatRegistered,
    Residency { resident: bool },
    PaymentCurrency { currencies: Vec<String> },
}

/// A stored rule, with its id and version, as the settings screens show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct TaxRuleRow {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    /// `tax`, `levy` or `service_charge`.
    #[schema(value_type = String)]
    pub kind: tax::Kind,
    /// Basis points: 1000 is 10%.
    pub rate_bp: u32,
    /// `*`, a category (`room`, `meal_plan`, `laundry`, `misc`, `fee`), `pos:*` or one outlet (`pos:BAR`).
    pub applies_to: Vec<String>,
    /// `net` and the codes of rules calculated earlier.
    pub base: Vec<String>,
    pub sequence: i32,
    /// `on_bill` or `absorbed`.
    #[schema(value_type = String)]
    pub presentation: tax::Presentation,
    /// `{"type": "always"}`, `{"type": "vat_registered"}`, `{"type": "residency", "resident": true}` or
    /// `{"type": "payment_currency", "currencies": ["USD"]}`.
    #[schema(value_type = TaxConditionSchema)]
    pub condition: tax::Condition,
    pub effective_from: Date,
    /// The first day the rule no longer applies; `null` while open-ended.
    pub effective_to: Option<Date>,
    pub version: i32,
}

fn into_row(row: RuleRow) -> Result<TaxRuleRow, sqlx::Error> {
    let (id, version) = (row.id, row.version);
    let rule = into_rule(row)?;
    Ok(TaxRuleRow {
        id,
        code: rule.code,
        name: rule.name,
        kind: rule.kind,
        rate_bp: rule.rate_bp,
        applies_to: rule.applies_to,
        base: rule.base,
        sequence: rule.sequence,
        presentation: rule.presentation,
        condition: rule.condition,
        effective_from: rule.effective_from,
        effective_to: rule.effective_to,
        version,
    })
}

/// Every rule of `property`, in calculation order. `tx` must be scoped to the property's tenant.
pub async fn load_rules(tx: &mut Tx, property: Uuid) -> Result<Vec<tax::Rule>, sqlx::Error> {
    rule_rows(tx, property).await?.into_iter().map(into_rule).collect()
}

async fn rule_rows(tx: &mut Tx, property: Uuid) -> Result<Vec<RuleRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {RULE_COLUMNS} from tax_rule where property_id = $1 order by sequence, effective_from, code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// Every rule of `property`, ended or not, in calculation order then by start date.
pub async fn list_tax_rules(tx: &mut Tx, property: Uuid) -> Result<Vec<TaxRuleRow>, sqlx::Error> {
    rule_rows(tx, property).await?.into_iter().map(into_row).collect()
}

/// Makes every service charge of `rules` apply to no category, for a POS outlet that charges none. The rules stay in
/// the set, so a base naming one (`SC` in SSCL's and VAT's) still validates and counts it as zero, as the engine
/// counts any lower rule that does not apply to a line. Removing them instead would fail validation.
pub(crate) fn without_service_charge(rules: &mut [tax::Rule]) {
    for rule in rules.iter_mut().filter(|rule| rule.kind == tax::Kind::ServiceCharge) {
        rule.applies_to.clear();
    }
}

/// The fields of a rule to change; `None` keeps the current value. The code never changes: a rule under a new code
/// is a new rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaxRuleChanges {
    pub name: Option<String>,
    pub kind: Option<tax::Kind>,
    pub rate_bp: Option<u32>,
    pub applies_to: Option<Vec<String>>,
    pub base: Option<Vec<String>>,
    pub sequence: Option<i32>,
    pub presentation: Option<tax::Presentation>,
    pub condition: Option<tax::Condition>,
    pub effective_from: Option<Date>,
    /// `Some(None)` makes the rule open-ended.
    pub effective_to: Option<Option<Date>>,
}

/// Refuses a rule that is malformed on its own (`Invalid`); [`tax::validate`] then checks it within its set.
fn check_rule(rule: &tax::Rule) -> Result<(), FolioError> {
    let invalid = |message: String| Err(FolioError::Invalid(message));
    let code = rule.code.as_bytes();
    let code_ok = (1..=16).contains(&code.len())
        && code[0].is_ascii_uppercase()
        && code.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_');
    if !code_ok {
        return invalid("a rule code is 1 to 16 upper-case letters, digits and _, starting with a letter".into());
    }
    if !(1..=MAX_RULE_NAME).contains(&rule.name.trim().chars().count()) {
        return invalid(format!("a rule's name has 1 to {MAX_RULE_NAME} characters"));
    }
    if rule.rate_bp > 10_000 {
        return invalid("a rate is at most 10000 basis points (100%)".into());
    }
    if !(1..=MAX_RULE_PARTS).contains(&rule.applies_to.len()) || !(1..=MAX_RULE_PARTS).contains(&rule.base.len()) {
        return invalid(format!(
            "a rule applies to 1 to {MAX_RULE_PARTS} categories and has 1 to {MAX_RULE_PARTS} base parts"
        ));
    }
    if let Some(bad) =
        rule.applies_to.iter().find(|pattern| !matches!(pattern.as_str(), "*" | "pos:*") && !valid_category(pattern))
    {
        return invalid(format!(
            "{bad:?} is not a category: use *, room, meal_plan, laundry, misc, fee, pos:* or pos:<OUTLET>"
        ));
    }
    if let tax::Condition::PaymentCurrency { currencies } = &rule.condition
        && (currencies.is_empty()
            || currencies.iter().any(|c| c.len() != 3 || !c.bytes().all(|b| b.is_ascii_uppercase())))
    {
        return invalid("a payment-currency condition names one or more ISO 4217 codes".into());
    }
    if rule.effective_to.is_some_and(|to| to <= rule.effective_from) {
        return invalid("a rule ends after the day it starts".into());
    }
    Ok(())
}

/// `Invalid` unless the property's rules, as now written in `tx`, form a set [`tax::validate`] accepts.
async fn check_set(tx: &mut Tx, property: Uuid) -> Result<(), FolioError> {
    let rules = load_rules(tx, property).await?;
    tax::validate(&rules).map_err(|err| FolioError::Invalid(format!("the tax rules would not be valid: {err}")))
}

/// Holds the property row until the transaction ends, so the property's rule changes run one at a time. `NotFound`
/// if the property is not in the tenant.
async fn lock_property(tx: &mut Tx, property: Uuid) -> Result<(), FolioError> {
    sqlx::query_scalar::<_, Uuid>("select id from property where id = $1 for no key update")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(FolioError::NotFound("property"))?;
    Ok(())
}

async fn insert_rule(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    rule: &tax::Rule,
) -> Result<TaxRuleRow, FolioError> {
    let rate_bp = i32::try_from(rule.rate_bp).map_err(|_| FolioError::Invalid("the rate is too large".into()))?;
    let row: RuleRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence,
                               presentation, condition, effective_from, effective_to)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         returning {RULE_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&rule.code)
    .bind(rule.name.trim())
    .bind(kind_str(rule.kind))
    .bind(rate_bp)
    .bind(&rule.applies_to)
    .bind(&rule.base)
    .bind(rule.sequence)
    .bind(presentation_str(rule.presentation))
    .bind(serde_json::json!(rule.condition))
    .bind(rule.effective_from)
    .bind(rule.effective_to)
    .fetch_one(&mut **tx)
    .await?;
    Ok(into_row(row)?)
}

/// Adds `rule` to `property`'s rules (`NotFound` if the property is not in the tenant). The rule must be well formed
/// and the resulting set valid: its base may name only `net` and rules calculated earlier, and a code may not be in
/// force twice on any day (each `Invalid`). Audited as `tax_rule.created`. The caller checks `TaxSettingsManage`.
pub async fn create_tax_rule(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    rule: tax::Rule,
) -> Result<TaxRuleRow, FolioError> {
    check_rule(&rule)?;
    lock_property(tx, property).await?;
    let created = insert_rule(tx, tenant, property, &rule).await?;
    check_set(tx, property).await?;
    audit(tx, tenant, user, "tax_rule.created", "tax_rule", created.id, serde_json::json!(rule)).await?;
    Ok(created)
}

/// The rule `id` of `property`, locked. `NotFound` unless it is in the property; `VersionMismatch` unless it is at
/// `expected_version`.
async fn locked_rule(tx: &mut Tx, property: Uuid, id: Uuid, expected_version: i32) -> Result<RuleRow, FolioError> {
    let row: RuleRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {RULE_COLUMNS} from tax_rule where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("tax rule"))?;
    if row.version != expected_version {
        return Err(FolioError::VersionMismatch("tax rule"));
    }
    Ok(row)
}

/// Changes the rule `id` of `property` at `expected_version` (`NotFound`, `VersionMismatch`), checked as
/// [`create_tax_rule`] checks a new rule. Lines already posted keep the taxes they were posted with. A rule in force
/// (starting on or before the business date) may change only its name and, to a day after the business date or to
/// open-ended, its end (`Conflict`, "… End it and add a new rule"). Audited as
/// `tax_rule.updated` with the rule before and after.
#[allow(clippy::too_many_arguments)]
pub async fn update_tax_rule(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: TaxRuleChanges,
) -> Result<TaxRuleRow, FolioError> {
    lock_property(tx, property).await?;
    let before = into_rule(locked_rule(tx, property, id, expected_version).await?)?;
    let after = tax::Rule {
        code: before.code.clone(),
        name: changes.name.unwrap_or_else(|| before.name.clone()),
        kind: changes.kind.unwrap_or(before.kind),
        rate_bp: changes.rate_bp.unwrap_or(before.rate_bp),
        applies_to: changes.applies_to.unwrap_or_else(|| before.applies_to.clone()),
        base: changes.base.unwrap_or_else(|| before.base.clone()),
        sequence: changes.sequence.unwrap_or(before.sequence),
        presentation: changes.presentation.unwrap_or(before.presentation),
        condition: changes.condition.unwrap_or_else(|| before.condition.clone()),
        effective_from: changes.effective_from.unwrap_or(before.effective_from),
        effective_to: changes.effective_to.unwrap_or(before.effective_to),
    };
    check_rule(&after)?;
    let today = business_date(tx, property).await?;
    if before.effective_from <= today && !in_force_change(&before, &after, today) {
        return Err(FolioError::Conflict(
            "The rule is in force: only its name and a later end can change. End it and add a new rule".into(),
        ));
    }
    let rate_bp = i32::try_from(after.rate_bp).map_err(|_| FolioError::Invalid("the rate is too large".into()))?;
    let row: RuleRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update tax_rule set name = $2, kind = $3, rate_bp = $4, applies_to = $5, base = $6, sequence = $7,
                presentation = $8, condition = $9, effective_from = $10, effective_to = $11, version = version + 1
         where id = $1
         returning {RULE_COLUMNS}"
    )))
    .bind(id)
    .bind(after.name.trim())
    .bind(kind_str(after.kind))
    .bind(rate_bp)
    .bind(&after.applies_to)
    .bind(&after.base)
    .bind(after.sequence)
    .bind(presentation_str(after.presentation))
    .bind(serde_json::json!(after.condition))
    .bind(after.effective_from)
    .bind(after.effective_to)
    .fetch_one(&mut **tx)
    .await?;
    check_set(tx, property).await?;
    let data = serde_json::json!({ "before": before, "after": after });
    audit(tx, tenant, user, "tax_rule.updated", "tax_rule", id, data).await?;
    Ok(into_row(row)?)
}

/// Whether `after` changes the rule `before`, in force by `today`, only as lines already posted allow: its name, and
/// its end when both the old and the new end are after `today` (or open-ended), so the days up to `today` keep the
/// rule exactly as their lines were taxed with it.
fn in_force_change(before: &tax::Rule, after: &tax::Rule, today: Date) -> bool {
    let later = |end: Option<Date>| end.is_none_or(|end| end > today);
    let same_terms =
        tax::Rule { name: before.name.clone(), effective_to: before.effective_to, ..after.clone() } == *before;
    same_terms
        && (after.effective_to == before.effective_to || (later(before.effective_to) && later(after.effective_to)))
}

/// Ends the rule `id` of `property` at `expected_version` (`NotFound`, `VersionMismatch`) on the property's business
/// date: it applies to no line dated that day or later. A rule is never deleted: one that has not started by the
/// business date can't be ended (`Conflict`; change its dates instead), nor can one already ended by then
/// (`Conflict`). Audited as `tax_rule.ended`.
pub async fn end_tax_rule(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<TaxRuleRow, FolioError> {
    lock_property(tx, property).await?;
    let current = locked_rule(tx, property, id, expected_version).await?;
    let today = business_date(tx, property).await?;
    if current.effective_from >= today {
        return Err(FolioError::Conflict(format!(
            "The rule starts on {}; change its dates instead of ending it",
            current.effective_from
        )));
    }
    if current.effective_to.is_some_and(|to| to <= today) {
        return Err(FolioError::Conflict("The rule has already ended".into()));
    }
    let row: RuleRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update tax_rule set effective_to = $2, version = version + 1 where id = $1 returning {RULE_COLUMNS}"
    )))
    .bind(id)
    .bind(today)
    .fetch_one(&mut **tx)
    .await?;
    check_set(tx, property).await?;
    let data =
        serde_json::json!({ "code": row.code, "effective_to": today, "previous_effective_to": current.effective_to });
    audit(tx, tenant, user, "tax_rule.ended", "tax_rule", id, data).await?;
    Ok(into_row(row)?)
}

/// What [`apply_preset`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct PresetApplied {
    /// The day the preset takes effect: the day after the property's business date, so no line already posted for
    /// the business date is taxed differently from the rest of that day's.
    pub effective_from: Date,
    /// Rules in force on that day, now ended on it.
    pub ended: u32,
    /// Rules that would have started after the business date, removed: they had never been in force, and once the
    /// preset takes over they would apply to no day at all.
    pub removed: u32,
    /// The property's rules now, as [`list_tax_rules`] returns them.
    pub rules: Vec<TaxRuleRow>,
}

/// Replaces `property`'s rules with the Sri Lanka preset (`tax::sri_lanka_preset`, with the property's
/// `tdl_small_turnover`) from the day after its business date: every rule in force on that day ends on it, and every
/// rule starting after the business date (never yet in force) is removed. Lines already posted keep their taxes.
/// `NotFound` if the property is not in the tenant. Audited as `tax_rule.preset_applied` on the property, with each
/// ended rule's previous `effective_to` and every removed rule in full. The caller checks `TaxSettingsManage`.
pub async fn apply_preset(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
) -> Result<PresetApplied, FolioError> {
    lock_property(tx, property).await?;
    let (today, small_turnover): (Date, bool) =
        sqlx::query_as("select business_date, tdl_small_turnover from property where id = $1 for share")
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
    let from =
        today.next_day().ok_or_else(|| FolioError::Invalid("the business date is the last day there is".into()))?;
    let ended: Vec<(Uuid, String, Option<Date>)> = sqlx::query_as(
        "update tax_rule r set effective_to = $2, version = r.version + 1
         from tax_rule before
         where before.id = r.id and r.property_id = $1 and r.effective_from < $2
           and (r.effective_to is null or r.effective_to > $2)
         returning r.id, r.code, before.effective_to",
    )
    .bind(property)
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    let removed: Vec<RuleRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "delete from tax_rule where property_id = $1 and effective_from >= $2 returning {RULE_COLUMNS}"
    )))
    .bind(property)
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    let removed: Vec<TaxRuleRow> = removed.into_iter().map(into_row).collect::<Result<_, _>>()?;
    for mut rule in tax::sri_lanka_preset(small_turnover) {
        rule.effective_from = from;
        insert_rule(tx, tenant, property, &rule).await?;
    }
    check_set(tx, property).await?;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let data = serde_json::json!({
        "preset": "sri_lanka",
        "effective_from": from,
        "tdl_small_turnover": small_turnover,
        "ended": ended
            .iter()
            .map(|(id, code, previous)| serde_json::json!({ "id": id, "code": code, "previous_effective_to": previous }))
            .collect::<Vec<_>>(),
        "removed": removed,
    });
    audit(tx, tenant, user, "tax_rule.preset_applied", "property", property, data).await?;
    Ok(PresetApplied {
        effective_from: from,
        ended: count(ended.len()),
        removed: count(removed.len()),
        rules: list_tax_rules(tx, property).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::TaxConditionSchema;

    /// The documented shape is the one `tax::Condition` reads and writes, variant for variant.
    #[test]
    fn the_condition_schema_is_the_tax_crates_json() {
        let pairs = [
            (TaxConditionSchema::Always, tax::Condition::Always),
            (TaxConditionSchema::VatRegistered, tax::Condition::VatRegistered),
            (TaxConditionSchema::Residency { resident: true }, tax::Condition::Residency { resident: true }),
            (
                TaxConditionSchema::PaymentCurrency { currencies: vec!["USD".into()] },
                tax::Condition::PaymentCurrency { currencies: vec!["USD".into()] },
            ),
        ];
        for (schema, condition) in pairs {
            let json = serde_json::to_value(&schema).unwrap();
            assert_eq!(json, serde_json::to_value(&condition).unwrap());
            assert_eq!(serde_json::from_value::<tax::Condition>(json).unwrap(), condition);
        }
    }
}
