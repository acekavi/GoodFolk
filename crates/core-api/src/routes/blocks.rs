use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::rooms_error;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{Block, BlockKind, BlockReason, BlockReasonChanges, NewBlock, NewBlockReason};
use serde::Deserialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateBlockReasonRequest {
    /// Capital letters, digits and `_`, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9_]{1,20}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub label: String,
    /// The kind the block dialog suggests for this reason.
    #[garde(skip)]
    pub default_kind: BlockKind,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateBlockReasonRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub label: Option<String>,
    #[garde(skip)]
    pub default_kind: Option<BlockKind>,
    /// `false` retires the reason: existing blocks keep it, new blocks cannot use it.
    #[garde(skip)]
    pub active: Option<bool>,
}

/// Blocks the room for `[from, to)`: `to` is the first day it is back in service.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateBlockRequest {
    /// `YYYY-MM-DD`, on or after the property's business date.
    #[garde(skip)]
    pub from: Date,
    /// `YYYY-MM-DD`, after `from`.
    #[garde(skip)]
    pub to: Date,
    #[garde(skip)]
    pub kind: BlockKind,
    #[garde(skip)]
    pub reason_id: Uuid,
    #[serde(default)]
    #[garde(length(chars, max = 500))]
    pub note: String,
}

/// Ends the block early: the room is back from `to`. Sending the business date releases it now; a date on
/// or before the block's start cancels it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ShortenBlockRequest {
    /// `YYYY-MM-DD`, on or after the business date and before the block's current end.
    #[garde(skip)]
    pub to: Date,
}

#[utoipa::path(post, operation_id = "create_block_reason", path = "/api/v1/properties/{property}/block-reasons", request_body = CreateBlockReasonRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = BlockReason), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_reason(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateBlockReasonRequest>,
) -> Result<Versioned<BlockReason>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewBlockReason { code: body.code, label: body.label, default_kind: body.default_kind };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rooms::create_block_reason(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_block_reason", path = "/api/v1/properties/{property}/block-reasons/{reason}",
    request_body = UpdateBlockReasonRequest,
    params(("property" = Uuid, Path), ("reason" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = BlockReason), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_reason(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, reason)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateBlockReasonRequest>,
) -> Result<Versioned<BlockReason>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = BlockReasonChanges { label: body.label, default_kind: body.default_kind, active: body.active };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_block_reason(&mut tx, ctx.tenant, ctx.user, property, reason, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// A 409 lists the blocks in the way as `conflicts` (each with `id`, `room_id`, `from`, `to`, `kind`).
#[utoipa::path(post, operation_id = "create_block", path = "/api/v1/properties/{property}/rooms/{room}/blocks", request_body = CreateBlockRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Block), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CreateBlockRequest>,
) -> Result<Versioned<Block>, ApiError> {
    ctx.require(Permission::InventoryBlock, Some(property))?;
    validate(&body)?;
    let input = NewBlock {
        room_id: room,
        from: body.from,
        to: body.to,
        kind: body.kind,
        reason_id: body.reason_id,
        note: body.note,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_block(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "shorten_block", path = "/api/v1/properties/{property}/blocks/{block}", request_body = ShortenBlockRequest,
    params(("property" = Uuid, Path), ("block" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Block), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn shorten(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, block)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<ShortenBlockRequest>,
) -> Result<Versioned<Block>, ApiError> {
    ctx.require(Permission::InventoryBlock, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::shorten_block(&mut tx, ctx.tenant, ctx.user, property, block, version, body.to)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
