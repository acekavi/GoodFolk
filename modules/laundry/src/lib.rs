//! Linen items, where they are kept and the par each room type needs, and guests' laundry.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction and queue a change event.

mod linen;

pub use linen::{
    LinenItem, LinenLocation, LocationKind, ParRow, create_item, create_location, list_items, list_locations, list_par,
    set_par, update_item, update_location,
};

use db::{Event, TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum LaundryError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// The request clashes with other data.
    #[error("{0}")]
    Conflict(String),
    /// A business rule or a malformed value.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Folio(#[from] folio::FolioError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's linen items, locations, par and stock.
pub fn linen_key(property: Uuid) -> String {
    format!("linen:{property}")
}

/// Cache key for a property's laundry prices, batches and orders.
pub fn laundry_key(property: Uuid) -> String {
    format!("laundry:{property}")
}

/// Whether `err` violated the named constraint or unique index.
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

/// Queues one change event for `keys`.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}
