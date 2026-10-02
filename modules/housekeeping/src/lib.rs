//! Room condition, who cleans which room, and the issues found in rooms.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction and queue a change event.

mod assignments;
mod condition;

pub use assignments::{Housekeeper, copy_yesterday, housekeepers, set_assignments};

pub use condition::{
    Actor, Condition, MAX_BULK_ROOMS, RoomConditionRow, housekeeper_may, lock_and_set, set_condition, set_conditions,
};

use db::{Event, TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum HousekeepingError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// The request clashes with other data.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a condition change the actor may not make.
    #[error("{0}")]
    Invalid(String),
    /// The actor may not do this, such as a housekeeper changing a room that is not theirs today.
    #[error("{0}")]
    Forbidden(&'static str),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's housekeeping data: room conditions, assignments and issues.
pub fn hk_key(property: Uuid) -> String {
    format!("hk:{property}")
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

/// Queues one change event for `keys`.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}

/// The users among `users` who hold `housekeeping` or `hk_supervisor` for the property, or tenant-wide.
async fn role_holders(tx: &mut Tx, property: Uuid, users: &[Uuid]) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "select distinct user_id from role_grant
         where user_id = any($1) and role in ('housekeeping', 'hk_supervisor')
           and (property_id = $2 or property_id is null)",
    )
    .bind(users)
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
