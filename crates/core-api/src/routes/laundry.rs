//! The guest laundry price list and guests' laundry orders, charged to the stay on delivery. Reads go through GraphQL
//! (`laundryPrices`, `laundryOrders`, `laundryOrdersForStay`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, laundry_error, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use laundry::{Order, OrderStatus, Price, Service};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateLaundryPriceRequest {
    /// 1 to 20 of `A-Z`, `0-9` and `_`; with the service, unique in the property. Never changes.
    #[garde(pattern(r"^[A-Z0-9_]{1,20}$"))]
    pub item_code: String,
    /// `wash`, `press`, `dry_clean` or `express`. Never changes.
    #[garde(skip)]
    pub service: Service,
    /// 1 to 100 characters once trimmed.
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// Net of tax, in minor units of the property's base currency; more than 0.
    #[garde(range(min = 1))]
    pub price: i64,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateLaundryPriceRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    /// Applies to orders taken from now on; orders already taken keep the price they were taken at.
    #[garde(inner(range(min = 1)))]
    pub price: Option<i64>,
    /// An inactive price is left off new orders.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateLaundryPriceRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.price.is_none() && self.active.is_none()
    }
}

/// One line of an order: an active price of the property and how many items.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LaundryOrderLineRequest {
    #[garde(skip)]
    pub price_id: Uuid,
    /// 1 to 100.
    #[garde(range(min = 1, max = 100))]
    pub quantity: i32,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateLaundryOrderRequest {
    /// A stay checked in at the property.
    #[garde(skip)]
    pub reservation_room_id: Uuid,
    /// 1 to 50 lines; a price may repeat.
    #[garde(length(min = 1, max = 50), dive)]
    pub lines: Vec<LaundryOrderLineRequest>,
    /// Up to 500 characters.
    #[garde(skip)]
    pub note: Option<String>,
}

/// Replaces a `collected` order's lines; the note is kept when left out and cleared when sent as `null`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateLaundryOrderRequest {
    /// 1 to 50 lines; a price may repeat.
    #[garde(length(min = 1, max = 50), dive)]
    pub lines: Vec<LaundryOrderLineRequest>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(skip)]
    pub note: Option<Option<String>>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetLaundryOrderStatusRequest {
    /// The next step (`processing`, `ready`, then `delivered`), or `cancelled` from any step before `delivered`.
    #[garde(skip)]
    pub status: OrderStatus,
}

fn lines(lines: Vec<LaundryOrderLineRequest>) -> Vec<(Uuid, i32)> {
    lines.into_iter().map(|line| (line.price_id, line.quantity)).collect()
}

/// Adds a price to the guest laundry price list. An item code and service the property already prices is 409.
/// Audited. A retried request with the same `Idempotency-Key` is answered without adding another.
#[utoipa::path(post, operation_id = "create_laundry_price", path = "/api/v1/properties/{property}/laundry-prices", request_body = CreateLaundryPriceRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Price,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_price(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateLaundryPriceRequest>,
) -> Result<Versioned<Price>, ApiError> {
    ctx.require(Permission::LaundryPricesManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let price = laundry::create_price(
        &mut tx,
        ctx.tenant,
        ctx.user,
        property,
        &body.item_code,
        body.service,
        &body.name,
        body.price,
    )
    .await
    .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::created(price.version, price))
}

/// Renames a price, changes its amount or switches it on or off. Audited.
#[utoipa::path(patch, operation_id = "update_laundry_price", path = "/api/v1/properties/{property}/laundry-prices/{price}", request_body = UpdateLaundryPriceRequest,
    params(("property" = Uuid, Path), ("price" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Price,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_price(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, price)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateLaundryPriceRequest>,
) -> Result<Versioned<Price>, ApiError> {
    ctx.require(Permission::LaundryPricesManage, Some(property))?;
    validate_changes(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = laundry::update_price(
        &mut tx,
        ctx.tenant,
        ctx.user,
        property,
        price,
        version,
        body.name,
        body.price,
        body.active,
    )
    .await
    .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Takes a laundry order for a checked-in stay (else 422), priced from the price list, as `collected`. Nothing is
/// charged until delivery. Audited. A retried request with the same `Idempotency-Key` is answered without taking
/// another.
#[utoipa::path(post, operation_id = "create_laundry_order", path = "/api/v1/properties/{property}/laundry-orders", request_body = CreateLaundryOrderRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Order,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 422)))]
pub async fn create_order(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateLaundryOrderRequest>,
) -> Result<Versioned<Order>, ApiError> {
    ctx.require(Permission::LaundryOrders, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let order = laundry::create_order(
        &mut tx,
        ctx.tenant,
        ctx.user,
        property,
        body.reservation_room_id,
        &lines(body.lines),
        body.note,
    )
    .await
    .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::created(order.version, order))
}

/// Replaces the lines (and optionally the note) of an order still `collected` (else 409). Audited.
#[utoipa::path(patch, operation_id = "update_laundry_order", path = "/api/v1/properties/{property}/laundry-orders/{order}", request_body = UpdateLaundryOrderRequest,
    params(("property" = Uuid, Path), ("order" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Order,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_order(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, order)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateLaundryOrderRequest>,
) -> Result<Versioned<Order>, ApiError> {
    ctx.require(Permission::LaundryOrders, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated =
        laundry::update_lines(&mut tx, ctx.tenant, ctx.user, property, order, version, &lines(body.lines), body.note)
            .await
            .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Moves an order one step on, or cancels it before delivery; any other move is 409. Delivery charges each line to
/// the stay's bill (routed as its other charges, taxed by the folio), so the guest must still be checked in (409); a
/// charge the folio refuses refuses the delivery. Audited.
#[utoipa::path(post, operation_id = "set_laundry_order_status", path = "/api/v1/properties/{property}/laundry-orders/{order}/status", request_body = SetLaundryOrderStatusRequest,
    params(("property" = Uuid, Path), ("order" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Order,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn set_order_status(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, order)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SetLaundryOrderStatusRequest>,
) -> Result<Versioned<Order>, ApiError> {
    ctx.require(Permission::LaundryOrders, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let moved = laundry::set_status(&mut tx, ctx.tenant, ctx.user, property, order, version, body.status)
        .await
        .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(moved.version, moved))
}
