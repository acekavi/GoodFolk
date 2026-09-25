use crate::{MealPlan, RatesError, audit, business_date, notify, rate_plans_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

/// What a meal plan costs per person per night on top of the room price, in one currency, for the nights
/// from `from` until `to` (the first night it no longer applies; `None`: until further notice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct MealSupplement {
    pub id: Uuid,
    pub property_id: Uuid,
    #[sqlx(try_from = "String")]
    pub meal_plan: MealPlan,
    pub currency: String,
    pub adult_amount: i64,
    pub child_amount: i64,
    pub from: Date,
    pub to: Option<Date>,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewMealSupplement {
    pub meal_plan: MealPlan,
    pub currency: String,
    pub adult_amount: i64,
    pub child_amount: i64,
    pub from: Date,
    pub to: Option<Date>,
}

/// `None` leaves a field unchanged; `to: Some(None)` makes the supplement open-ended.
#[derive(Debug, Clone, Default)]
pub struct MealSupplementChanges {
    pub adult_amount: Option<i64>,
    pub child_amount: Option<i64>,
    pub from: Option<Date>,
    pub to: Option<Option<Date>>,
}

const COLUMNS: &str = "id, property_id, meal_plan, currency::text as currency, adult_amount, child_amount, \
                       lower(valid) as \"from\", upper(valid) as \"to\", version";

fn check_period(from: Date, to: Option<Date>) -> Result<(), RatesError> {
    if to.is_some_and(|to| to <= from) {
        Err(RatesError::Invalid("a supplement ends after it starts".into()))
    } else {
        Ok(())
    }
}

fn overlap_error(err: sqlx::Error, meal_plan: MealPlan, currency: &str) -> RatesError {
    if violates(&err, "meal_supplement_no_overlap") {
        RatesError::Conflict(format!(
            "a {} supplement in {currency} already covers some of these dates",
            meal_plan.as_str()
        ))
    } else {
        err.into()
    }
}

pub async fn create_meal_supplement(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewMealSupplement,
) -> Result<MealSupplement, RatesError> {
    business_date(tx, property).await?;
    if input.meal_plan == MealPlan::Ro {
        return Err(RatesError::Invalid("room only (RO) has no supplement".into()));
    }
    check_period(input.from, input.to)?;
    let created: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
         values ($1, $2, $3, $4, $5, $6, $7, daterange($8, $9))
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(input.meal_plan.as_str())
    .bind(&input.currency)
    .bind(input.adult_amount)
    .bind(input.child_amount)
    .bind(input.from)
    .bind(input.to)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| overlap_error(err, input.meal_plan, &input.currency))?;
    audit(
        tx,
        tenant,
        actor,
        "meal_supplement.created",
        "meal_supplement",
        created.id,
        serde_json::json!({ "meal_plan": input.meal_plan.as_str(), "currency": input.currency }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(created)
}

pub async fn update_meal_supplement(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: MealSupplementChanges,
) -> Result<MealSupplement, RatesError> {
    let current: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from meal_supplement where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("meal supplement"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("meal supplement"));
    }
    let (from, to) = (changes.from.unwrap_or(current.from), changes.to.unwrap_or(current.to));
    check_period(from, to)?;
    let updated: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update meal_supplement set adult_amount = coalesce($2, adult_amount), child_amount = coalesce($3, child_amount),
                valid = daterange($4, $5), version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(changes.adult_amount)
    .bind(changes.child_amount)
    .bind(from)
    .bind(to)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| overlap_error(err, current.meal_plan, &current.currency))?;
    audit(
        tx,
        tenant,
        actor,
        "meal_supplement.updated",
        "meal_supplement",
        id,
        serde_json::json!({ "adult_amount": updated.adult_amount, "child_amount": updated.child_amount }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(updated)
}

/// Every supplement of the property, by currency, meal plan and start.
pub async fn list_meal_supplements(tx: &mut Tx, property: Uuid) -> Result<Vec<MealSupplement>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from meal_supplement where property_id = $1
         order by currency, array_position(array['BB', 'HB', 'FB'], meal_plan), lower(valid)"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
