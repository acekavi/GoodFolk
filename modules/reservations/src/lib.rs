//! Guests, what a property has free to sell them, and later the reservations they book.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction. Guests belong to the tenant, not to a property, so a chain shares guest history.

mod availability;
mod guests;

pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
pub use guests::{
    Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
};

use db::{TenantId, Tx, UserId};
use rates::RatesError;
use time::Date;
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

impl From<RatesError> for ReservationsError {
    fn from(err: RatesError) -> Self {
        match err {
            RatesError::NotFound(what) => ReservationsError::NotFound(what),
            RatesError::VersionMismatch(what) => ReservationsError::VersionMismatch(what),
            RatesError::Conflict(message) => ReservationsError::Conflict(message),
            RatesError::Invalid(message) => ReservationsError::Invalid(message),
            RatesError::Database(err) => ReservationsError::Database(err),
        }
    }
}

/// The property's business date. `NotFound` if the property is not in this tenant.
async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("property"))
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
