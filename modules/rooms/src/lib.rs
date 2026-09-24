//! Room types, housekeeping sections, rooms, room blocks, and the inventory counters they keep up to date.
//!
//! Every function takes a transaction scoped to the caller's tenant and checks that ids belong to the
//! given property. Writes record an audit entry and queue change events in the same transaction.

mod inventory;
mod room_types;
mod rooms;
mod sections;

pub use inventory::{InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys};
pub use room_types::{
    Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
    update_room_type,
};
pub use rooms::{
    MAX_ROOMS_PER_RANGE, NewRoom, Room, RoomChanges, RoomRange, create_room, create_rooms, list_rooms, reorder_rooms,
    update_room,
};
pub use sections::{Section, create_section, list_sections, rename_section};

use db::{Event, TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RoomsError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// A uniqueness rule, such as a duplicate code or room number.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a capacity that does not add up or an unknown room type.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's room types.
pub fn room_types_key(property: Uuid) -> String {
    format!("room-types:{property}")
}

/// Cache key for a property's rooms, sections and block reasons.
pub fn rooms_key(property: Uuid) -> String {
    format!("rooms:{property}")
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

async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}

/// Checks that every one of the property's `table` rows is listed exactly once, then stores the listed order.
async fn reorder(tx: &mut Tx, table: &'static str, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    let (existing, listed): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select (select count(*) from {table} where property_id = $1),
                (select count(distinct t.id) from {table} t where t.property_id = $1 and t.id = any($2))"
    )))
    .bind(property)
    .bind(ids)
    .fetch_one(&mut **tx)
    .await?;
    if existing != listed || usize::try_from(listed).ok() != Some(ids.len()) {
        return Err(RoomsError::Invalid(format!(
            "list every {} of the property exactly once",
            table.replace('_', " ")
        )));
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "update {table} t set sort_order = o.position::integer
         from unnest($2::uuid[]) with ordinality as o (id, position)
         where t.id = o.id and t.property_id = $1"
    )))
    .bind(property)
    .bind(ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
