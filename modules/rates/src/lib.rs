//! Rate plans with their resolved daily prices and restrictions, meal supplements and cancellation policies.
//!
//! Every function takes a transaction scoped to the caller's tenant and checks that ids belong to the given
//! property. Writes record an audit entry and queue change events in the same transaction. Writes to a
//! property's plans, prices and restrictions first take [`lock_rates`]: a change to one plan cascades to the
//! plans derived from it, and one lock per property is simpler than a lock order across all their rows.

mod meals;
mod plans;
mod policies;
mod prices;
mod quote;
mod restrictions;

pub use meals::{
    MealSupplement, MealSupplementChanges, NewMealSupplement, create_meal_supplement, list_meal_supplements,
    update_meal_supplement,
};
pub use plans::{
    ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, Residency, Segment, create_rate_plan,
    list_rate_plans, update_rate_plan,
};
pub use policies::{
    CancellationPolicy, CancellationPolicyChanges, CancellationRule, NewCancellationPolicy, Penalty, PenaltyKind,
    create_cancellation_policy, list_cancellation_policies, update_cancellation_policy,
};
pub use prices::{
    BulkChange, BulkPreview, Price, PriceChange, PriceChangeCell, PriceChangeMode, bulk_change, list_prices,
    preview_bulk_change, set_prices,
};
pub use quote::{
    MAX_STAY_NIGHTS, Quote, QuoteData, QuoteNight, QuoteRequest, QuoteRoomType, Violation, ViolationKind, load_quote,
    quote,
};
pub use restrictions::{Restriction, RestrictionChange, list_restrictions, set_restrictions};

use db::{Event, TenantId, Tx, UserId};
use time::Date;
use uuid::Uuid;

/// Largest amount of money, in minor units, anywhere in rates (the tables check it too).
pub const MAX_AMOUNT: i64 = 100_000_000_000;

#[derive(Debug, thiserror::Error)]
pub enum RatesError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// A uniqueness rule, such as a duplicate code, or a change that other data depends on.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a derivation in another currency or a date outside the rate window.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's rate plans, meal supplements and cancellation policies.
pub fn rate_plans_key(property: Uuid) -> String {
    format!("rate-plans:{property}")
}

/// Cache keys `rates:<property>:<plan>:<yyyy-mm>` for every month of `[from, to)`.
pub fn rates_keys(property: Uuid, plan: Uuid, from: Date, to: Date) -> Vec<String> {
    rooms::months(from, to).into_iter().map(|month| format!("rates:{property}:{plan}:{month}")).collect()
}

/// The property's business date. `NotFound` if the property is not in this tenant.
async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, RatesError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RatesError::NotFound("property"))
}

/// Serializes writes to one property's rate plans, prices and restrictions until the transaction ends.
async fn lock_rates(tx: &mut Tx, property: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('rates:' || $1::text, 0))")
        .bind(property)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Whether `err` violated the named constraint.
fn violates(err: &sqlx::Error, constraint: &str) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
}

async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    action: &str,
    entity: &str,
    id: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(action)
    .bind(entity)
    .bind(id)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Bytes of keys per event, well under Postgres' 8000-byte NOTIFY payload with the ids around them.
const EVENT_KEY_BYTES: usize = 6000;

/// `keys` split into batches of at most [`EVENT_KEY_BYTES`] (counting quotes and commas), in order.
fn event_batches(keys: Vec<String>) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for key in keys {
        let key_size = key.len() + 3;
        match batches.last_mut() {
            Some(batch) if size + key_size <= EVENT_KEY_BYTES => batch.push(key),
            _ => {
                size = 0;
                batches.push(vec![key]);
            }
        }
        size += key_size;
    }
    batches
}

/// Queues change events for `keys`. A change to a plan with many derived plans touches many plan-months, so
/// the keys are sent in as many events as it takes to keep each one under the NOTIFY payload limit.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    for keys in event_batches(keys) {
        db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EVENT_KEY_BYTES, event_batches, rates_keys};
    use time::macros::date;
    use uuid::Uuid;

    #[test]
    fn rate_keys_name_the_plan_and_each_month() {
        let (p, plan) = (Uuid::nil(), Uuid::max());

        assert_eq!(
            rates_keys(p, plan, date!(2026 - 07 - 31), date!(2026 - 08 - 02)),
            [format!("rates:{p}:{plan}:2026-07"), format!("rates:{p}:{plan}:2026-08")]
        );
    }

    #[test]
    fn many_keys_are_split_into_events_that_fit_a_notify_payload() {
        let keys: Vec<String> =
            (0..200).map(|n| format!("rates:{}:{}:2026-{n:03}", Uuid::nil(), Uuid::max())).collect();

        let batches = event_batches(keys.clone());

        assert!(batches.len() > 1);
        assert!(batches.iter().all(|batch| batch.iter().map(|key| key.len() + 3).sum::<usize>() <= EVENT_KEY_BYTES));
        assert_eq!(batches.concat(), keys);
        assert_eq!(event_batches(vec![]), Vec::<Vec<String>>::new());
    }
}
