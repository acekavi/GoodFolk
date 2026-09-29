//! Guests, what a property has free to sell them, and the reservations they book.
//!
//! Every function takes a transaction scoped to the caller's tenant. Writes record an audit entry in the same
//! transaction, and reservation writes queue change events. Guests belong to the tenant, not to a property, so
//! a chain shares guest history.

mod accounts;
mod assignment;
mod availability;
mod cancellation;
mod detail;
mod guests;
mod list;
mod reservations;

pub use accounts::{
    Account, AccountChanges, AccountContact, AccountKind, MAX_ACCOUNT_LIST, NewAccount, create_account, get_account,
    list_accounts, update_account,
};
pub use assignment::{AssignedRoom, FreeRoom, assign_room, free_rooms, unassign_room};
pub use availability::{AvailabilityRequest, MAX_AVAILABILITY_NIGHTS, RoomTypeAvailability, availability};
pub use cancellation::{CancellationTerms, CancelledRoom, cancel_room, cancellation_penalty};
pub use detail::{
    AccountRef, HistoryEntry, Night, RatePlanRef, ReservationDetail, RoomDetail, RoomRef, RoomTypeRef, get_reservation,
    reservation_history,
};
pub use guests::{
    Guest, GuestChanges, IdDocType, MAX_GUEST_SEARCH, NewGuest, create_guest, get_guest, search_guests, update_guest,
};
pub use list::{
    ListFilter, ListRequest, MAX_PAGE_SIZE, ReservationRoomPage, ReservationRoomRow, Sort, SortDirection, SortField,
    list_reservation_rooms,
};
pub use reservations::{
    CreatedReservation, CreatedRoom, MAX_ROOMS_PER_RESERVATION, NewReservation, NewReservationRoom, ReservationChanges,
    Source, Total, UpdatedReservation, create_reservation, update_reservation,
};

use db::{Event, TenantId, Tx, UserId};
use rates::RatesError;
use rooms::WINDOW_DAYS;
use time::{Date, Duration};
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

/// Cache key for a property's reservation list.
pub fn reservations_key(property: Uuid) -> String {
    format!("reservations:{property}")
}

/// Cache key for one reservation's detail.
pub fn reservation_key(reservation: Uuid) -> String {
    format!("reservation:{reservation}")
}

/// Whether `err` violated the named constraint.
fn violates(err: &sqlx::Error, constraint: &str) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
}

/// A column value the code has no variant for.
fn decode_error(column: &str, value: &str) -> sqlx::Error {
    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
}

/// The property's business date. `NotFound` if the property is not in this tenant.
async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, ReservationsError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("property"))
}

/// Refuses a stay outside the counter window, `[business date, business date + WINDOW_DAYS)`: it must arrive
/// on or after the business date and leave by the window's end.
fn check_window(business_date: Date, check_in: Date, check_out: Date) -> Result<(), ReservationsError> {
    let last = business_date + Duration::days(WINDOW_DAYS);
    if check_in < business_date || check_out > last {
        return Err(ReservationsError::Invalid(format!(
            "stays must arrive on or after {business_date} and leave by {last}"
        )));
    }
    Ok(())
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

/// Queues one change event for `keys`. Reservation writes stay inside the counter window, so their inventory
/// month keys (at most 25) keep the event well under the NOTIFY payload limit.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}
