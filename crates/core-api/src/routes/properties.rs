use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use property::{NewProperty, Property, PropertyChanges, PropertyError};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePropertyRequest {
    /// 2 to 10 capital letters or digits, unique within the tenant.
    #[garde(pattern(r"^[A-Z0-9]{2,10}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub name: String,
    /// IANA time zone, e.g. `Asia/Colombo`.
    #[garde(length(min = 1, max = 64))]
    pub timezone: String,
    /// ISO 4217 code, e.g. `LKR`.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub base_currency: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdatePropertyRequest {
    #[garde(inner(length(chars, min = 1, max = 200)))]
    pub name: Option<String>,
    /// `HH:MM` (24-hour), local time.
    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
    pub check_in_time: Option<String>,
    /// `HH:MM` (24-hour), local time.
    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
    pub check_out_time: Option<String>,
}

fn property_error(err: PropertyError) -> ApiError {
    match err {
        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
        PropertyError::NotFound => ApiError::not_found(err.to_string()),
        PropertyError::VersionMismatch => ApiError::precondition_failed(err.to_string()),
        PropertyError::Database(db_err) => db_err.into(),
    }
}

#[utoipa::path(post, operation_id = "create_property", path = "/api/v1/properties", request_body = CreatePropertyRequest,
    params(("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiJson(body): ApiJson<CreatePropertyRequest>,
) -> Result<Versioned<Property>, ApiError> {
    ctx.require(Permission::PropertiesCreate, None)?;
    validate(&body)?;
    let input =
        NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(property_error)?;
    rooms::seed_block_reasons(&mut tx, ctx.tenant, created.id).await?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

/// Changes a property's settings. The business date is not editable: night audit moves it.
#[utoipa::path(patch, operation_id = "update_property", path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
    params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"")),
    responses((status = 200, body = Property), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdatePropertyRequest>,
) -> Result<Versioned<Property>, ApiError> {
    ctx.require(Permission::PropertiesManage, Some(property))?;
    validate(&body)?;
    let changes =
        PropertyChanges { name: body.name, check_in_time: body.check_in_time, check_out_time: body.check_out_time };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = property::update_property(&mut tx, ctx.tenant, ctx.user, property, version, changes)
        .await
        .map_err(property_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
