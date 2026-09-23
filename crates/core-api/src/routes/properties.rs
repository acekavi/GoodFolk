use crate::auth::TenantContext;
use crate::error::{ApiError, validate};
use crate::extract::ApiJson;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use property::{NewProperty, Property, PropertyError};
use serde::Deserialize;
use utoipa::ToSchema;

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

#[utoipa::path(post, path = "/api/v1/properties", request_body = CreatePropertyRequest,
    params(("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiJson(body): ApiJson<CreatePropertyRequest>,
) -> Result<(StatusCode, Json<Property>), ApiError> {
    ctx.require(Permission::PropertiesCreate, None)?;
    validate(&body)?;
    let input =
        NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(|err| match err {
        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
        PropertyError::Database(db_err) => db_err.into(),
    })?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(created)))
}
