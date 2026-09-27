//! Guests, and later the reservations they book.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction. Guests belong to the tenant, not to a property, so a chain shares guest history.

mod guests;

pub use guests::{
    Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
};

use db::{TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ReservationsError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// The request clashes with other data, such as a room already taken on those nights.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a malformed email or a stay outside the booking window.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
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
