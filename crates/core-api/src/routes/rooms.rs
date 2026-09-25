use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{NewRoom, Room, RoomChanges, RoomRange, RoomsError, Section};
use serde::{Deserialize, Deserializer};
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the rooms module's errors to problem details.
pub(crate) fn rooms_error(err: RoomsError) -> ApiError {
    match err {
        RoomsError::NotFound(_) => ApiError::not_found(err.to_string()),
        RoomsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        RoomsError::Conflict(message) => ApiError::conflict(message),
        RoomsError::Invalid(message) => ApiError::unprocessable(message),
        RoomsError::Overlap(_) => ApiError::conflict(err.to_string()),
        RoomsError::Database(db_err) => db_err.into(),
    }
}

/// Tells a field sent as `null` (`Some(None)`: clear it) from one left out (`None`: keep it).
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReorderRequest {
    /// Every id of the collection, in the new display order.
    #[garde(length(min = 1, max = 2000))]
    pub ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    /// Letters, digits and `-`, up to 10 characters, unique within the property.
    #[garde(pattern(r"^[A-Za-z0-9-]{1,10}$"))]
    pub number: String,
    #[garde(inner(length(chars, min = 1, max = 20)))]
    pub floor: Option<String>,
    #[garde(skip)]
    pub section_id: Option<Uuid>,
}

/// Rooms `{prefix}{first}` to `{prefix}{last}`: `{"first": 101, "last": 120}` adds 101 to 120.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomRangeRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    #[serde(default)]
    #[garde(pattern(r"^[A-Za-z0-9-]{0,5}$"))]
    pub prefix: String,
    #[garde(range(max = 99999))]
    pub first: u32,
    #[garde(range(max = 99999))]
    pub last: u32,
    #[garde(inner(length(chars, min = 1, max = 20)))]
    pub floor: Option<String>,
    #[garde(skip)]
    pub section_id: Option<Uuid>,
}

/// Fields left out stay as they are; `floor` and `section_id` sent as `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRoomRequest {
    #[garde(skip)]
    pub room_type_id: Option<Uuid>,
    #[garde(inner(pattern(r"^[A-Za-z0-9-]{1,10}$")))]
    pub number: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 1, max = 20))))]
    pub floor: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>, nullable)]
    #[garde(skip)]
    pub section_id: Option<Option<Uuid>>,
    #[garde(skip)]
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SectionRequest {
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
}

#[utoipa::path(post, operation_id = "create_room", path = "/api/v1/properties/{property}/rooms", request_body = CreateRoomRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Room), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomRequest>,
) -> Result<Versioned<Room>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewRoom {
        room_type_id: body.room_type_id,
        number: body.number,
        floor: body.floor,
        section_id: body.section_id,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_room(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(post, operation_id = "create_rooms", path = "/api/v1/properties/{property}/rooms/bulk", request_body = CreateRoomRangeRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Vec<Room>), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_range(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomRangeRequest>,
) -> Result<(StatusCode, Json<Vec<Room>>), ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let range = RoomRange {
        room_type_id: body.room_type_id,
        prefix: body.prefix,
        first: body.first,
        last: body.last,
        floor: body.floor,
        section_id: body.section_id,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_rooms(&mut tx, ctx.tenant, ctx.user, property, range).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(patch, operation_id = "update_room", path = "/api/v1/properties/{property}/rooms/{room}", request_body = UpdateRoomRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Room), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRoomRequest>,
) -> Result<Versioned<Room>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = RoomChanges {
        room_type_id: body.room_type_id,
        number: body.number,
        floor: body.floor,
        section_id: body.section_id,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_room(&mut tx, ctx.tenant, ctx.user, property, room, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(put, operation_id = "reorder_rooms", path = "/api/v1/properties/{property}/rooms/order", request_body = ReorderRequest,
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
    rooms::reorder_rooms(&mut tx, ctx.tenant, property, &body.ids).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, operation_id = "create_section", path = "/api/v1/properties/{property}/sections", request_body = SectionRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Section), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_section(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SectionRequest>,
) -> Result<Versioned<Section>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rooms::create_section(&mut tx, ctx.tenant, ctx.user, property, &body.name).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "rename_section", path = "/api/v1/properties/{property}/sections/{section}", request_body = SectionRequest,
    params(("property" = Uuid, Path), ("section" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Section), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn rename_section(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, section)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SectionRequest>,
) -> Result<Versioned<Section>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let renamed = rooms::rename_section(&mut tx, ctx.tenant, ctx.user, property, section, version, &body.name)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(renamed.version, renamed))
}
