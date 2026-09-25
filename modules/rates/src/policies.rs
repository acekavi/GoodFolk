use crate::{RatesError, audit, business_date, notify, rate_plans_key, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

text_enum!(
    /// `nights`: that many nights' room price; `percent`: basis points of the stay; `amount`: minor units in
    /// the plan's currency.
    PenaltyKind { Nights = "nights", Percent = "percent", Amount = "amount" }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Penalty {
    pub kind: PenaltyKind,
    pub value: i64,
}

/// Cancelling `days_before_arrival` days or fewer before arrival costs `penalty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CancellationRule {
    pub days_before_arrival: i32,
    pub penalty: Penalty,
}

/// What cancelling or not arriving costs. Reservations (Phase 3) apply it; rate plans name one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CancellationPolicy {
    pub id: Uuid,
    pub property_id: Uuid,
    pub name: String,
    /// Furthest from arrival first; the rule with the fewest days at or above the days left applies.
    #[sqlx(json)]
    pub rules: Vec<CancellationRule>,
    #[sqlx(json)]
    pub no_show: Penalty,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewCancellationPolicy {
    pub name: String,
    pub rules: Vec<CancellationRule>,
    pub no_show: Penalty,
}

/// `None` leaves a field unchanged; `rules` replaces every rule.
#[derive(Debug, Clone, Default)]
pub struct CancellationPolicyChanges {
    pub name: Option<String>,
    pub rules: Option<Vec<CancellationRule>>,
    pub no_show: Option<Penalty>,
}

const COLUMNS: &str = "id, property_id, name, rules, no_show, version";

fn check_penalty(penalty: Penalty) -> Result<(), RatesError> {
    let (range, message) = match penalty.kind {
        PenaltyKind::Nights => (1..=30, "a nights penalty is 1 to 30 nights"),
        PenaltyKind::Percent => (1..=10_000, "a percent penalty is 1 to 10000 basis points"),
        PenaltyKind::Amount => (1..=100_000_000_000, "an amount penalty is 1 to 100000000000 minor units"),
    };
    if range.contains(&penalty.value) { Ok(()) } else { Err(RatesError::Invalid(message.into())) }
}

/// The rules, checked and sorted furthest from arrival first.
fn checked_rules(mut rules: Vec<CancellationRule>, no_show: Penalty) -> Result<Vec<CancellationRule>, RatesError> {
    check_penalty(no_show)?;
    for rule in &rules {
        if !(0..=365).contains(&rule.days_before_arrival) {
            return Err(RatesError::Invalid("days before arrival are 0 to 365".into()));
        }
        check_penalty(rule.penalty)?;
    }
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.days_before_arrival));
    if rules.windows(2).any(|pair| pair[0].days_before_arrival == pair[1].days_before_arrival) {
        return Err(RatesError::Invalid("each rule needs its own number of days before arrival".into()));
    }
    Ok(rules)
}

fn name_error(err: sqlx::Error, name: &str) -> RatesError {
    if violates(&err, "cancellation_policy_property_id_name_key") {
        RatesError::Conflict(format!("a cancellation policy named {name} already exists"))
    } else {
        err.into()
    }
}

pub async fn create_cancellation_policy(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewCancellationPolicy,
) -> Result<CancellationPolicy, RatesError> {
    business_date(tx, property).await?;
    let rules = checked_rules(input.rules, input.no_show)?;
    let created: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
         values ($1, $2, $3, $4, $5, $6)
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.name)
    .bind(sqlx::types::Json(&rules))
    .bind(sqlx::types::Json(input.no_show))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, &input.name))?;
    audit(
        tx,
        tenant,
        actor,
        "cancellation_policy.created",
        "cancellation_policy",
        created.id,
        serde_json::json!({ "name": input.name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(created)
}

pub async fn update_cancellation_policy(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: CancellationPolicyChanges,
) -> Result<CancellationPolicy, RatesError> {
    let current: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from cancellation_policy where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("cancellation policy"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("cancellation policy"));
    }
    let no_show = changes.no_show.unwrap_or(current.no_show);
    let rules = checked_rules(changes.rules.unwrap_or(current.rules), no_show)?;
    let name = changes.name.unwrap_or(current.name);
    let updated: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update cancellation_policy set name = $2, rules = $3, no_show = $4, version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(&name)
    .bind(sqlx::types::Json(&rules))
    .bind(sqlx::types::Json(no_show))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, &name))?;
    audit(
        tx,
        tenant,
        actor,
        "cancellation_policy.updated",
        "cancellation_policy",
        id,
        serde_json::json!({ "name": name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(updated)
}

/// The property's cancellation policies, by name.
pub async fn list_cancellation_policies(tx: &mut Tx, property: Uuid) -> Result<Vec<CancellationPolicy>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from cancellation_policy where property_id = $1 order by name"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
