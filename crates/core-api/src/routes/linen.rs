//! Linen items, where they are kept, the par each room type needs, the movements between locations and the batches
//! sent to a laundry. Reads go through GraphQL (`linenItems`, `linenLocations`, `linenPar`, `linenStock`,
//! `linenMovements`, `laundryBatches`).

use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, laundry_error, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use laundry::{Batch, LinenItem, LinenLocation, LocationKind, Movement, MovementKind, NewMovement, ParRow, Settle};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateLinenItemRequest {
    /// 1 to 20 of `A-Z`, `0-9` and `_`, unique in the property. Never changes.
    #[garde(pattern(r"^[A-Z0-9_]{1,20}$"))]
    pub code: String,
    /// 1 to 100 characters once trimmed.
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateLinenItemRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    /// An inactive item is left out of stock, par and new movements; its history stays.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateLinenItemRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.active.is_none()
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateLinenLocationRequest {
    /// 1 to 100 characters once trimmed.
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// `store`, `pantry`, `laundry` or `vendor`. Never changes.
    #[garde(skip)]
    pub kind: LocationKind,
    /// A pantry's section of the property; other kinds have none. Never changes.
    #[garde(skip)]
    pub section_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateLinenLocationRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    /// A location holding stock cannot be switched off.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateLinenLocationRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.active.is_none()
    }
}

/// How many of one item.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LinenCountRequest {
    #[garde(skip)]
    pub item_id: Uuid,
    #[garde(skip)]
    pub quantity: i32,
}

/// The whole par table of a room type: how many of each item one of its rooms needs. Items left out need none.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetParRequest {
    /// 0 to 50 items of the property, each once, each with a quantity of 1 to 100.
    #[garde(length(max = 50), dive)]
    pub items: Vec<LinenCountRequest>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RecordMovementRequest {
    #[garde(skip)]
    pub item_id: Uuid,
    /// `receive` (bought in, to a store or pantry), `transfer` (between stores and pantries) or `write_off` (lost,
    /// from any location, with a reason). `send` and `return` go with a laundry batch.
    #[garde(skip)]
    pub kind: MovementKind,
    /// 1 to 100000.
    #[garde(skip)]
    pub quantity: i32,
    #[garde(skip)]
    pub from_location_id: Option<Uuid>,
    #[garde(skip)]
    pub to_location_id: Option<Uuid>,
    /// Up to 500 characters; needed for a write-off.
    #[garde(skip)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SendBatchRequest {
    /// A store or pantry of the property.
    #[garde(skip)]
    pub from_location_id: Uuid,
    /// A laundry or vendor of the property.
    #[garde(skip)]
    pub to_location_id: Uuid,
    /// 1 to 200 items, each once, each more than 0.
    #[garde(length(min = 1, max = 200), dive)]
    pub items: Vec<LinenCountRequest>,
    /// Up to 500 characters.
    #[garde(skip)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReceiveBatchRequest {
    /// Where the counted items go: a store or pantry; the location the batch was sent from when left out.
    #[garde(skip)]
    pub back_to: Option<Uuid>,
    /// The count of each batch item that came back, 0 to what is outstanding; items left out count as 0.
    #[garde(length(max = 200), dive)]
    pub items: Vec<LinenCountRequest>,
}

/// What a received batch still has out: `{"how": "return", "items": [...]}` for items found and returned (to
/// `back_to`, else where the batch came from), or `{"how": "write_off", "reason": "...", "items": [...]}` for items
/// lost.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum SettleBatchRequest {
    Return {
        back_to: Option<Uuid>,
        /// Up to 200 items, each at most what is outstanding; at least one more than 0.
        items: Vec<LinenCountRequest>,
    },
    WriteOff {
        /// 1 to 500 characters once trimmed.
        reason: String,
        items: Vec<LinenCountRequest>,
    },
}

fn counts(items: Vec<LinenCountRequest>) -> Vec<(Uuid, i32)> {
    items.into_iter().map(|item| (item.item_id, item.quantity)).collect()
}

/// Adds a linen item to the property. A code the property already uses is 409. Audited. A retried request with the
/// same `Idempotency-Key` is answered without adding another.
#[utoipa::path(post, operation_id = "create_linen_item", path = "/api/v1/properties/{property}/linen-items", request_body = CreateLinenItemRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = LinenItem,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_item(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateLinenItemRequest>,
) -> Result<Versioned<LinenItem>, ApiError> {
    ctx.require(Permission::LinenManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let item = laundry::create_item(&mut tx, ctx.tenant, ctx.user, property, &body.code, &body.name)
        .await
        .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::created(item.version, item))
}

/// Renames a linen item or switches it on or off. Audited.
#[utoipa::path(patch, operation_id = "update_linen_item", path = "/api/v1/properties/{property}/linen-items/{item}", request_body = UpdateLinenItemRequest,
    params(("property" = Uuid, Path), ("item" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = LinenItem,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_item(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, item)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateLinenItemRequest>,
) -> Result<Versioned<LinenItem>, ApiError> {
    ctx.require(Permission::LinenManage, Some(property))?;
    validate_changes(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = laundry::update_item(&mut tx, ctx.tenant, ctx.user, property, item, version, body.name, body.active)
        .await
        .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Adds a place linen is kept. A pantry needs a section of the property, which has at most one active pantry, and
/// the property has at most one active store (409). Audited. A retried request with the same `Idempotency-Key` is
/// answered without adding another.
#[utoipa::path(post, operation_id = "create_linen_location", path = "/api/v1/properties/{property}/linen-locations", request_body = CreateLinenLocationRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = LinenLocation,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_location(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateLinenLocationRequest>,
) -> Result<Versioned<LinenLocation>, ApiError> {
    ctx.require(Permission::LinenManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let location =
        laundry::create_location(&mut tx, ctx.tenant, ctx.user, property, &body.name, body.kind, body.section_id)
            .await
            .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::created(location.version, location))
}

/// Renames a location or switches it on or off; one holding stock cannot be switched off (409). Audited.
#[utoipa::path(patch, operation_id = "update_linen_location", path = "/api/v1/properties/{property}/linen-locations/{location}", request_body = UpdateLinenLocationRequest,
    params(("property" = Uuid, Path), ("location" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = LinenLocation,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_location(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, location)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateLinenLocationRequest>,
) -> Result<Versioned<LinenLocation>, ApiError> {
    ctx.require(Permission::LinenManage, Some(property))?;
    validate_changes(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated =
        laundry::update_location(&mut tx, ctx.tenant, ctx.user, property, location, version, body.name, body.active)
            .await
            .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Replaces the room type's whole par table and returns it, by item. Audited.
#[utoipa::path(put, operation_id = "set_linen_par", path = "/api/v1/properties/{property}/linen-par/{room_type}", request_body = SetParRequest,
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path)),
    responses((status = 200, body = Vec<ParRow>), (status = 403), (status = 404), (status = 422)))]
pub async fn set_par(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SetParRequest>,
) -> Result<Json<Vec<ParRow>>, ApiError> {
    ctx.require(Permission::LinenManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let rows = laundry::set_par(&mut tx, ctx.tenant, ctx.user, property, room_type, &counts(body.items))
        .await
        .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// Records one movement and updates the stock; one that would take a location below zero is 409. A `receive` or
/// `transfer` needs `LinenMove`, a `write_off` `LinenManage`. Audited. A retried request with the same
/// `Idempotency-Key` is answered without recording another.
#[utoipa::path(post, operation_id = "record_linen_movement", path = "/api/v1/properties/{property}/linen-movements", request_body = RecordMovementRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Movement), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn record_movement(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<RecordMovementRequest>,
) -> Result<(StatusCode, Json<Movement>), ApiError> {
    let needs = if body.kind == MovementKind::WriteOff { Permission::LinenManage } else { Permission::LinenMove };
    ctx.require(needs, Some(property))?;
    let movement = NewMovement {
        item_id: body.item_id,
        kind: body.kind,
        quantity: body.quantity,
        from_location_id: body.from_location_id,
        to_location_id: body.to_location_id,
        reason: body.reason,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let recorded = laundry::record(&mut tx, ctx.tenant, ctx.user, property, movement).await.map_err(laundry_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(recorded)))
}

/// Sends linen from a store or pantry to a laundry or vendor as one batch; more than the store holds is 409. Audited.
/// A retried request with the same `Idempotency-Key` is answered without sending another.
#[utoipa::path(post, operation_id = "send_laundry_batch", path = "/api/v1/properties/{property}/laundry-batches", request_body = SendBatchRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Batch,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn send_batch(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SendBatchRequest>,
) -> Result<Versioned<Batch>, ApiError> {
    ctx.require(Permission::LinenMove, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let batch = laundry::send(
        &mut tx,
        ctx.tenant,
        ctx.user,
        property,
        body.from_location_id,
        body.to_location_id,
        &counts(body.items),
        body.note,
    )
    .await
    .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::created(batch.version, batch))
}

/// Counts a sent batch back in: the counted items return to `back_to` and the batch is `received`; what did not come
/// back stays outstanding at the laundry until settled. A batch already received is 409. Audited.
#[utoipa::path(post, operation_id = "receive_laundry_batch", path = "/api/v1/properties/{property}/laundry-batches/{batch}/receive", request_body = ReceiveBatchRequest,
    params(("property" = Uuid, Path), ("batch" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Batch,
        headers(("ETag" = String, description = "the version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn receive_batch(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, batch)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<ReceiveBatchRequest>,
) -> Result<Versioned<Batch>, ApiError> {
    ctx.require(Permission::LinenMove, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let received =
        laundry::receive(&mut tx, ctx.tenant, ctx.user, property, batch, version, body.back_to, &counts(body.items))
            .await
            .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(received.version, received))
}

/// Settles part of what a received batch still has out: a further return needs `LinenMove`, a write-off
/// `LinenManage`. A batch not yet received, or with nothing outstanding, is 409. Audited.
#[utoipa::path(post, operation_id = "settle_laundry_batch", path = "/api/v1/properties/{property}/laundry-batches/{batch}/settle", request_body = SettleBatchRequest,
    params(("property" = Uuid, Path), ("batch" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Batch,
        headers(("ETag" = String, description = "the version, e.g. \"3\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn settle_batch(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, batch)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SettleBatchRequest>,
) -> Result<Versioned<Batch>, ApiError> {
    let (needs, items) = match &body {
        SettleBatchRequest::Return { items, .. } => (Permission::LinenMove, items),
        SettleBatchRequest::WriteOff { items, .. } => (Permission::LinenManage, items),
    };
    ctx.require(needs, Some(property))?;
    if items.len() > 200 {
        return Err(ApiError::unprocessable("a settlement lists at most 200 items"));
    }
    let settle = match body {
        SettleBatchRequest::Return { back_to, items } => Settle::Return { back_to, counts: counts(items) },
        SettleBatchRequest::WriteOff { reason, items } => Settle::WriteOff { reason, counts: counts(items) },
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let settled = laundry::settle(&mut tx, ctx.tenant, ctx.user, property, batch, version, settle)
        .await
        .map_err(laundry_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(settled.version, settled))
}
