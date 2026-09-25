use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::{ReorderRequest, rooms_error};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{Bed, NewRoomType, RoomType, RoomTypeChanges};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct BedRequest {
    /// e.g. `king`, `queen`, `twin`, `sofa bed`.
    #[garde(length(chars, min = 1, max = 40))]
    pub kind: String,
    #[garde(range(min = 1, max = 10))]
    pub count: i32,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomTypeRequest {
    /// 1 to 10 capital letters or digits, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9]{1,10}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    #[garde(range(min = 1, max = 50))]
    pub base_occupancy: i32,
    #[garde(range(min = 1, max = 50))]
    pub max_adults: i32,
    #[garde(range(min = 0, max = 50))]
    pub max_children: i32,
    #[garde(range(min = 1, max = 50))]
    pub max_occupancy: i32,
    #[serde(default)]
    #[garde(length(max = 10), dive)]
    pub bed_config: Vec<BedRequest>,
    #[serde(default)]
    #[garde(length(max = 50), inner(length(chars, min = 1, max = 60)))]
    pub amenities: Vec<String>,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRoomTypeRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub base_occupancy: Option<i32>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub max_adults: Option<i32>,
    #[garde(inner(range(min = 0, max = 50)))]
    pub max_children: Option<i32>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub max_occupancy: Option<i32>,
    #[garde(length(max = 10), dive)]
    pub bed_config: Option<Vec<BedRequest>>,
    #[garde(inner(length(max = 50), inner(length(chars, min = 1, max = 60))))]
    pub amenities: Option<Vec<String>>,
    /// `false` retires the type; it must have no active rooms.
    #[garde(skip)]
    pub active: Option<bool>,
}

fn beds(requests: Vec<BedRequest>) -> Vec<Bed> {
    requests.into_iter().map(|bed| Bed { kind: bed.kind, count: bed.count }).collect()
}

#[utoipa::path(post, operation_id = "create_room_type", path = "/api/v1/properties/{property}/room-types", request_body = CreateRoomTypeRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomTypeRequest>,
) -> Result<Versioned<RoomType>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewRoomType {
        code: body.code,
        name: body.name,
        base_occupancy: body.base_occupancy,
        max_adults: body.max_adults,
        max_children: body.max_children,
        max_occupancy: body.max_occupancy,
        bed_config: beds(body.bed_config),
        amenities: body.amenities,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_room_type(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_room_type", path = "/api/v1/properties/{property}/room-types/{room_type}", request_body = UpdateRoomTypeRequest,
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRoomTypeRequest>,
) -> Result<Versioned<RoomType>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = RoomTypeChanges {
        name: body.name,
        base_occupancy: body.base_occupancy,
        max_adults: body.max_adults,
        max_children: body.max_children,
        max_occupancy: body.max_occupancy,
        bed_config: body.bed_config.map(beds),
        amenities: body.amenities,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_room_type(&mut tx, ctx.tenant, ctx.user, property, room_type, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(put, operation_id = "reorder_room_types", path = "/api/v1/properties/{property}/room-types/order", request_body = ReorderRequest,
    params(("property" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn reorder(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ReorderRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rooms::reorder_room_types(&mut tx, ctx.tenant, property, &body.ids).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
