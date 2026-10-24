//! The guest laundry price list: what one item costs for each service, in the property's base currency.

use crate::linen::{clean_name, ensure_property, valid_code};
use crate::{LaundryError, audit, laundry_key, notify, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

db::text_enum!(
    /// What is done to a guest's item.
    Service { Wash = "wash", Press = "press", DryClean = "dry_clean", Express = "express" }
);

/// A price, net of tax, in minor units of the property's base currency.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct Price {
    pub id: Uuid,
    pub item_code: String,
    #[sqlx(try_from = "String")]
    pub service: Service,
    pub name: String,
    pub price: i64,
    pub active: bool,
    pub version: i32,
}

fn valid_price(price: i64) -> Result<i64, LaundryError> {
    if price > 0 { Ok(price) } else { Err(LaundryError::Invalid("the price must be more than 0".into())) }
}

/// Creates a price. `item_code` is 1 to 20 of `A-Z`, `0-9`, `_`; `(item_code, service)` is unique in the property
/// (`Conflict`); `price` is more than 0.
#[allow(clippy::too_many_arguments)]
pub async fn create_price(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    item_code: &str,
    service: Service,
    name: &str,
    price: i64,
) -> Result<Price, LaundryError> {
    if !valid_code(item_code) {
        return Err(LaundryError::Invalid("the item code must be 1 to 20 characters of A-Z, 0-9 and _".into()));
    }
    let name = clean_name(name)?;
    let price = valid_price(price)?;
    ensure_property(tx, property).await?;
    let created: Price = sqlx::query_as(
        "insert into laundry_price (id, tenant_id, property_id, item_code, service, name, price)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id, item_code, service, name, price, active, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(item_code)
    .bind(service.as_str())
    .bind(name)
    .bind(price)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if violates(&err, "laundry_price_property_id_item_code_service_key") {
            LaundryError::Conflict(format!("{item_code} already has a {} price", service.as_str()))
        } else {
            err.into()
        }
    })?;
    audit(
        tx,
        tenant,
        actor,
        "laundry_price.created",
        "laundry_price",
        created.id,
        json!({ "item_code": item_code, "service": service.as_str(), "name": name, "price": price }),
    )
    .await?;
    notify(tx, tenant, property, vec![laundry_key(property)]).await?;
    Ok(created)
}

/// Renames a price, changes its amount or switches it on or off; at least one change is needed (`Invalid`).
#[allow(clippy::too_many_arguments)]
pub async fn update_price(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    name: Option<String>,
    price: Option<i64>,
    active: Option<bool>,
) -> Result<Price, LaundryError> {
    let current: Price = sqlx::query_as(
        "select id, item_code, service, name, price, active, version from laundry_price
         where id = $1 and property_id = $2 for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(LaundryError::NotFound("price"))?;
    if current.version != expected_version {
        return Err(LaundryError::VersionMismatch("price"));
    }
    let name = name.as_deref().map(clean_name).transpose()?.map_or(current.name.clone(), str::to_owned);
    let price = price.map(valid_price).transpose()?.unwrap_or(current.price);
    let active = active.unwrap_or(current.active);
    if name == current.name && price == current.price && active == current.active {
        return Err(LaundryError::Invalid("Nothing to change".into()));
    }
    let updated: Price = sqlx::query_as(
        "update laundry_price set name = $2, price = $3, active = $4, version = version + 1 where id = $1
         returning id, item_code, service, name, price, active, version",
    )
    .bind(id)
    .bind(&name)
    .bind(price)
    .bind(active)
    .fetch_one(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "laundry_price.updated",
        "laundry_price",
        id,
        json!({
            "name": name, "price": price, "active": active,
            "previous": { "name": current.name, "price": current.price, "active": current.active },
        }),
    )
    .await?;
    notify(tx, tenant, property, vec![laundry_key(property)]).await?;
    Ok(updated)
}

/// The property's prices by item code, then service (wash, press, dry clean, express).
pub async fn list_prices(tx: &mut Tx, property: Uuid, include_inactive: bool) -> Result<Vec<Price>, LaundryError> {
    Ok(sqlx::query_as(
        "select id, item_code, service, name, price, active, version from laundry_price
         where property_id = $1 and (active or $2)
         order by item_code, array_position(array['wash', 'press', 'dry_clean', 'express'], service)",
    )
    .bind(property)
    .bind(include_inactive)
    .fetch_all(&mut **tx)
    .await?)
}
