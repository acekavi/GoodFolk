use crate::inventory::{business_date, extend_window, window_keys};
use crate::{RoomsError, audit, notify, reorder, room_types_key, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Beds of one kind in a room type, e.g. `{ "kind": "king", "count": 1 }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Bed {
    pub kind: String,
    pub count: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RoomType {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub name: String,
    pub base_occupancy: i32,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    /// How many more rooms of this type may be sold than are physically available (0-20). A night is
    /// sellable when `physical - sold - out_of_order + overbooking > 0`; `reservations` applies the rule
    /// wherever a night is sold. [`InventoryDay::available`](crate::InventoryDay::available) stays the plain
    /// physical figure and does not include this allowance.
    pub overbooking: i32,
    #[sqlx(json)]
    pub bed_config: Vec<Bed>,
    pub amenities: Vec<String>,
    pub sort_order: i32,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRoomType {
    pub code: String,
    pub name: String,
    pub base_occupancy: i32,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    pub overbooking: i32,
    pub bed_config: Vec<Bed>,
    pub amenities: Vec<String>,
}

/// `None` leaves a field unchanged. The code never changes: channels and reports refer to it.
#[derive(Debug, Clone, Default)]
pub struct RoomTypeChanges {
    pub name: Option<String>,
    pub base_occupancy: Option<i32>,
    pub max_adults: Option<i32>,
    pub max_children: Option<i32>,
    pub max_occupancy: Option<i32>,
    pub overbooking: Option<i32>,
    pub bed_config: Option<Vec<Bed>>,
    pub amenities: Option<Vec<String>>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy, \
                       overbooking, bed_config, amenities, sort_order, active, version";

/// The capacity rules the database also enforces, checked first for a readable message.
fn check_capacity(base: i32, adults: i32, children: i32, max: i32) -> Result<(), RoomsError> {
    if base > max {
        Err(RoomsError::Invalid("base occupancy cannot exceed maximum occupancy".into()))
    } else if adults > max {
        Err(RoomsError::Invalid("maximum adults cannot exceed maximum occupancy".into()))
    } else if max > adults + children {
        Err(RoomsError::Invalid("maximum occupancy cannot exceed maximum adults plus maximum children".into()))
    } else {
        Ok(())
    }
}

/// The bound the database also enforces (`room_type_overbooking_check`), checked first for a readable message.
fn check_overbooking(value: i32) -> Result<(), RoomsError> {
    if (0..=20).contains(&value) {
        Ok(())
    } else {
        Err(RoomsError::Invalid("overbooking allowance must be between 0 and 20".into()))
    }
}

pub async fn create_room_type(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRoomType,
) -> Result<RoomType, RoomsError> {
    let today = business_date(tx, property).await?;
    check_capacity(input.base_occupancy, input.max_adults, input.max_children, input.max_occupancy)?;
    check_overbooking(input.overbooking)?;
    let inserted = sqlx::query_as::<_, RoomType>(sqlx::AssertSqlSafe(format!(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                max_occupancy, overbooking, bed_config, amenities, sort_order)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                 (select coalesce(max(sort_order) + 1, 0) from room_type where property_id = $3))
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.base_occupancy)
    .bind(input.max_adults)
    .bind(input.max_children)
    .bind(input.max_occupancy)
    .bind(input.overbooking)
    .bind(sqlx::types::Json(&input.bed_config))
    .bind(&input.amenities)
    .fetch_one(&mut **tx)
    .await;
    let room_type = match inserted {
        Ok(room_type) => room_type,
        Err(err) if violates(&err, "room_type_property_id_code_key") => {
            return Err(RoomsError::Conflict(format!("a room type with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    };
    extend_window(tx, property).await?;
    audit(tx, tenant, actor, "room_type.created", "room_type", room_type.id, serde_json::json!({ "code": input.code }))
        .await?;
    let mut keys = vec![room_types_key(property)];
    keys.extend(window_keys(property, today));
    notify(tx, tenant, property, keys).await?;
    Ok(room_type)
}

pub async fn update_room_type(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomTypeChanges,
) -> Result<RoomType, RoomsError> {
    let current: RoomType = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room_type where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RoomsError::NotFound("room type"))?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("room type"));
    }
    check_capacity(
        changes.base_occupancy.unwrap_or(current.base_occupancy),
        changes.max_adults.unwrap_or(current.max_adults),
        changes.max_children.unwrap_or(current.max_children),
        changes.max_occupancy.unwrap_or(current.max_occupancy),
    )?;
    if let Some(overbooking) = changes.overbooking {
        check_overbooking(overbooking)?;
    }
    if changes.active == Some(false) && current.active {
        let rooms: i64 = sqlx::query_scalar("select count(*) from room where room_type_id = $1 and active")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        if rooms > 0 {
            return Err(RoomsError::Conflict(format!(
                "{rooms} active rooms have this type; move or deactivate them first"
            )));
        }
    }
    let updated: RoomType = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update room_type set name = coalesce($3, name), base_occupancy = coalesce($4, base_occupancy),
                max_adults = coalesce($5, max_adults), max_children = coalesce($6, max_children),
                max_occupancy = coalesce($7, max_occupancy), overbooking = coalesce($8, overbooking),
                bed_config = coalesce($9, bed_config),
                amenities = coalesce($10, amenities), active = coalesce($11, active), version = version + 1
         where id = $1 and version = $2
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(changes.base_occupancy)
    .bind(changes.max_adults)
    .bind(changes.max_children)
    .bind(changes.max_occupancy)
    .bind(changes.overbooking)
    .bind(changes.bed_config.as_ref().map(sqlx::types::Json))
    .bind(&changes.amenities)
    .bind(changes.active)
    .fetch_one(&mut **tx)
    .await?;
    audit(tx, tenant, actor, "room_type.updated", "room_type", id, serde_json::json!({ "active": changes.active }))
        .await?;
    notify(tx, tenant, property, vec![room_types_key(property)]).await?;
    Ok(updated)
}

/// Sets the display order. `ids` must list every room type of the property exactly once.
pub async fn reorder_room_types(tx: &mut Tx, tenant: TenantId, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    business_date(tx, property).await?;
    reorder(tx, "room_type", property, ids).await?;
    notify(tx, tenant, property, vec![room_types_key(property)]).await?;
    Ok(())
}

/// Every room type of the property, active or not, in display order.
pub async fn list_room_types(tx: &mut Tx, property: Uuid) -> Result<Vec<RoomType>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room_type where property_id = $1 order by sort_order, code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
