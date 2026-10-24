//! Linen items, the places they are kept, and how many of each a room of a type should have (par).

use crate::{LaundryError, audit, linen_key, notify, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

/// The most items one room type's par may list.
const MAX_PAR_ROWS: usize = 50;

db::text_enum!(
    /// What a place linen is kept in is for.
    LocationKind { Store = "store", Pantry = "pantry", Laundry = "laundry", Vendor = "vendor" }
);

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct LinenItem {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct LinenLocation {
    pub id: Uuid,
    pub name: String,
    #[sqlx(try_from = "String")]
    pub kind: LocationKind,
    pub section_id: Option<Uuid>,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct ParRow {
    pub room_type_id: Uuid,
    pub item_id: Uuid,
    pub quantity: i32,
}

/// Whether `code` is 1 to 20 characters of `A-Z`, `0-9` and `_`.
pub(crate) fn valid_code(code: &str) -> bool {
    (1..=20).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A name once trimmed, 1 to 100 characters.
pub(crate) fn clean_name(name: &str) -> Result<&str, LaundryError> {
    let name = name.trim();
    if (1..=100).contains(&name.chars().count()) {
        Ok(name)
    } else {
        Err(LaundryError::Invalid("the name must be 1 to 100 characters".into()))
    }
}

/// Creates an item. `code` is 1 to 20 of `A-Z`, `0-9`, `_` and unique in the property (`Conflict`).
pub async fn create_item(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    code: &str,
    name: &str,
) -> Result<LinenItem, LaundryError> {
    if !valid_code(code) {
        return Err(LaundryError::Invalid("the code must be 1 to 20 characters of A-Z, 0-9 and _".into()));
    }
    let name = clean_name(name)?;
    ensure_property(tx, property).await?;
    let item: LinenItem = sqlx::query_as(
        "insert into linen_item (id, tenant_id, property_id, code, name) values ($1, $2, $3, $4, $5)
         returning id, code, name, active, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(code)
    .bind(name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if violates(&err, "linen_item_property_id_code_key") {
            LaundryError::Conflict(format!("an item with the code {code} already exists"))
        } else {
            err.into()
        }
    })?;
    audit(tx, tenant, actor, "linen_item.created", "linen_item", item.id, json!({ "code": code, "name": name }))
        .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(item)
}

/// Renames an item or switches it on or off; at least one change is needed (`Invalid`). An item with stock in any
/// location cannot be switched off (`Conflict`).
#[allow(clippy::too_many_arguments)]
pub async fn update_item(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    name: Option<String>,
    active: Option<bool>,
) -> Result<LinenItem, LaundryError> {
    let current: LinenItem = sqlx::query_as(
        "select id, code, name, active, version from linen_item where id = $1 and property_id = $2 for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(LaundryError::NotFound("item"))?;
    if current.version != expected_version {
        return Err(LaundryError::VersionMismatch("item"));
    }
    let name = name.as_deref().map(clean_name).transpose()?.map_or(current.name.clone(), str::to_owned);
    let active = active.unwrap_or(current.active);
    if name == current.name && active == current.active {
        return Err(LaundryError::Invalid("Nothing to change".into()));
    }
    if current.active && !active {
        let holds_stock: bool =
            sqlx::query_scalar("select exists (select 1 from linen_stock where item_id = $1 and quantity > 0)")
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
        if holds_stock {
            return Err(LaundryError::Conflict("Move or write off its stock first".into()));
        }
    }
    let item: LinenItem = sqlx::query_as("update linen_item set name = $2, active = $3, version = version + 1 where id = $1 returning id, code, name, active, version")
    .bind(id)
    .bind(&name)
    .bind(active)
    .fetch_one(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "linen_item.updated",
        "linen_item",
        id,
        json!({ "name": name, "active": active, "previous": { "name": current.name, "active": current.active } }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(item)
}

/// Creates a location. A pantry needs a section of the property and other kinds must have none; a property has
/// one active store and a section one active pantry (`Conflict`).
pub async fn create_location(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    name: &str,
    kind: LocationKind,
    section_id: Option<Uuid>,
) -> Result<LinenLocation, LaundryError> {
    let name = clean_name(name)?;
    match (kind, section_id) {
        (LocationKind::Pantry, None) => return Err(LaundryError::Invalid("a pantry needs a section".into())),
        (LocationKind::Pantry, Some(section)) => {
            let found: bool = sqlx::query_scalar(
                "select exists (select 1 from housekeeping_section where id = $1 and property_id = $2)",
            )
            .bind(section)
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
            if !found {
                return Err(LaundryError::NotFound("section"));
            }
        }
        (_, Some(_)) => return Err(LaundryError::Invalid("only a pantry has a section".into())),
        (_, None) => {}
    }
    ensure_property(tx, property).await?;
    let location: LinenLocation = sqlx::query_as(
        "insert into linen_location (id, tenant_id, property_id, name, kind, section_id)
         values ($1, $2, $3, $4, $5, $6) returning id, name, kind, section_id, active, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(name)
    .bind(kind.as_str())
    .bind(section_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| location_error(err, name))?;
    audit(
        tx,
        tenant,
        actor,
        "linen_location.created",
        "linen_location",
        location.id,
        json!({ "name": name, "kind": kind.as_str(), "section_id": section_id }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(location)
}

/// Renames a location or switches it on or off; at least one change is needed (`Invalid`). A location holding
/// stock cannot be switched off (`Conflict`).
#[allow(clippy::too_many_arguments)]
pub async fn update_location(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    name: Option<String>,
    active: Option<bool>,
) -> Result<LinenLocation, LaundryError> {
    let current: LinenLocation = sqlx::query_as("select id, name, kind, section_id, active, version from linen_location where id = $1 and property_id = $2 for update")
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(LaundryError::NotFound("location"))?;
    if current.version != expected_version {
        return Err(LaundryError::VersionMismatch("location"));
    }
    let name = name.as_deref().map(clean_name).transpose()?.map_or(current.name.clone(), str::to_owned);
    let active = active.unwrap_or(current.active);
    if name == current.name && active == current.active {
        return Err(LaundryError::Invalid("Nothing to change".into()));
    }
    if current.active && !active {
        let holds_stock: bool =
            sqlx::query_scalar("select exists (select 1 from linen_stock where location_id = $1 and quantity > 0)")
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
        if holds_stock {
            return Err(LaundryError::Conflict("Move or write off its stock first".into()));
        }
    }
    let location: LinenLocation = sqlx::query_as(
        "update linen_location set name = $2, active = $3, version = version + 1 where id = $1
         returning id, name, kind, section_id, active, version",
    )
    .bind(id)
    .bind(&name)
    .bind(active)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| location_error(err, &name))?;
    audit(
        tx,
        tenant,
        actor,
        "linen_location.updated",
        "linen_location",
        id,
        json!({ "name": name, "active": active, "previous": { "name": current.name, "active": current.active } }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(location)
}

/// Replaces the room type's whole par table (0-50 rows; quantity 1-100; items of the property).
pub async fn set_par(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_type: Uuid,
    rows: &[(Uuid, i32)],
) -> Result<Vec<ParRow>, LaundryError> {
    if rows.len() > MAX_PAR_ROWS {
        return Err(LaundryError::Invalid(format!("a par lists at most {MAX_PAR_ROWS} items")));
    }
    if rows.iter().any(|(_, quantity)| !(1..=100).contains(quantity)) {
        return Err(LaundryError::Invalid("a par quantity is 1 to 100".into()));
    }
    if rows.iter().map(|(item, _)| item).collect::<HashSet<_>>().len() != rows.len() {
        return Err(LaundryError::Invalid("an item appears once in a par".into()));
    }
    let found: bool = sqlx::query_scalar(
        "select exists (select 1 from room_type where id = $1 and property_id = $2 for no key update)",
    )
    .bind(room_type)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    if !found {
        return Err(LaundryError::NotFound("room type"));
    }
    let items: Vec<Uuid> = rows.iter().map(|(item, _)| *item).collect();
    let known: i64 = sqlx::query_scalar("select count(*) from linen_item where property_id = $1 and id = any($2)")
        .bind(property)
        .bind(&items)
        .fetch_one(&mut **tx)
        .await?;
    if known != rows.len() as i64 {
        return Err(LaundryError::NotFound("item"));
    }
    let previous: Vec<ParRow> = sqlx::query_as(
        "select room_type_id, item_id, quantity from linen_par where room_type_id = $1 order by item_id",
    )
    .bind(room_type)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("delete from linen_par where room_type_id = $1").bind(room_type).execute(&mut **tx).await?;
    let quantities: Vec<i32> = rows.iter().map(|(_, quantity)| *quantity).collect();
    sqlx::query(
        "insert into linen_par (tenant_id, property_id, room_type_id, item_id, quantity)
         select $1, $2, $3, item, quantity from unnest($4::uuid[], $5::int[]) as r(item, quantity)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(room_type)
    .bind(&items)
    .bind(&quantities)
    .execute(&mut **tx)
    .await?;
    let saved: Vec<ParRow> = sqlx::query_as(
        "select room_type_id, item_id, quantity from linen_par where room_type_id = $1 order by item_id",
    )
    .bind(room_type)
    .fetch_all(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "linen_par.updated",
        "room_type",
        room_type,
        json!({ "rows": rows_json(&saved), "previous": rows_json(&previous) }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(saved)
}

fn rows_json(rows: &[ParRow]) -> serde_json::Value {
    rows.iter().map(|row| json!({ "item_id": row.item_id, "quantity": row.quantity })).collect()
}

pub async fn list_items(tx: &mut Tx, property: Uuid) -> Result<Vec<LinenItem>, sqlx::Error> {
    sqlx::query_as("select id, code, name, active, version from linen_item where property_id = $1 order by code")
        .bind(property)
        .fetch_all(&mut **tx)
        .await
}

pub async fn list_locations(tx: &mut Tx, property: Uuid) -> Result<Vec<LinenLocation>, sqlx::Error> {
    sqlx::query_as(
        "select id, name, kind, section_id, active, version from linen_location where property_id = $1 order by name",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// Every par row of the property, by room type then item.
pub async fn list_par(tx: &mut Tx, property: Uuid) -> Result<Vec<ParRow>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, item_id, quantity from linen_par where property_id = $1 order by room_type_id, item_id",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

pub(crate) async fn ensure_property(tx: &mut Tx, property: Uuid) -> Result<(), LaundryError> {
    let found: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    if found { Ok(()) } else { Err(LaundryError::NotFound("property")) }
}

fn location_error(err: sqlx::Error, name: &str) -> LaundryError {
    if violates(&err, "linen_location_property_id_name_key") {
        LaundryError::Conflict(format!("a location named {name} already exists"))
    } else if violates(&err, "linen_location_one_store_idx") {
        LaundryError::Conflict("The property already has a store".into())
    } else if violates(&err, "linen_location_one_pantry_per_section_idx") {
        LaundryError::Conflict("The section already has a pantry".into())
    } else {
        err.into()
    }
}
