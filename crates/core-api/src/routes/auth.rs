use crate::auth::{Authenticated, removal_cookie, session_cookie};
use crate::error::{ApiError, validate};
use crate::extract::ApiJson;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use db::TenantId;
use garde::Validate;
use identity::{Profile, SignupError, SignupInput};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SignupRequest {
    #[garde(email)]
    pub email: String,
    #[garde(length(chars, min = 12, max = 128))]
    pub password: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub display_name: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub tenant_name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SwitchTenantRequest {
    pub tenant_id: Uuid,
}

#[utoipa::path(post, path = "/api/v1/auth/signup", request_body = SignupRequest,
    responses((status = 201, body = Profile), (status = 409), (status = 422)))]
pub async fn signup(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(body): ApiJson<SignupRequest>,
) -> Result<(StatusCode, CookieJar, Json<Profile>), ApiError> {
    validate(&body)?;
    let input = SignupInput {
        email: body.email,
        password: body.password,
        display_name: body.display_name,
        tenant_name: body.tenant_name,
    };
    let (user, tenant) = identity::signup(&state.pool, input).await.map_err(|err| match err {
        SignupError::EmailTaken => ApiError::conflict(err.to_string()),
        SignupError::Database(db_err) => db_err.into(),
    })?;
    let (token, expires) = identity::create_session(&state.pool, user, Some(tenant)).await?;
    let profile = identity::load_profile(&state.pool, user, Some(tenant)).await?;
    Ok((StatusCode::CREATED, jar.add(session_cookie(token, expires, state.production)), Json(profile)))
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
    responses((status = 200, body = Profile), (status = 401)))]
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(body): ApiJson<LoginRequest>,
) -> Result<(CookieJar, Json<Profile>), ApiError> {
    let user = identity::authenticate(&state.pool, &body.email, &body.password)
        .await?
        .ok_or_else(ApiError::invalid_credentials)?;
    let tenant = identity::default_tenant(&state.pool, user).await?;
    let (token, expires) = identity::create_session(&state.pool, user, tenant).await?;
    let profile = identity::load_profile(&state.pool, user, tenant).await?;
    Ok((jar.add(session_cookie(token, expires, state.production)), Json(profile)))
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 204), (status = 401)))]
pub async fn logout(
    State(state): State<AppState>,
    auth: Authenticated,
    jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    identity::delete_session(&state.pool, &auth.token).await?;
    Ok((StatusCode::NO_CONTENT, jar.remove(removal_cookie())))
}

#[utoipa::path(get, path = "/api/v1/me", responses((status = 200, body = Profile), (status = 401)))]
pub async fn me(State(state): State<AppState>, auth: Authenticated) -> Result<Json<Profile>, ApiError> {
    Ok(Json(identity::load_profile(&state.pool, auth.session.user, auth.session.tenant).await?))
}

#[utoipa::path(put, path = "/api/v1/session/tenant", request_body = SwitchTenantRequest,
    responses((status = 200, body = Profile), (status = 403)))]
pub async fn switch_tenant(
    State(state): State<AppState>,
    auth: Authenticated,
    ApiJson(body): ApiJson<SwitchTenantRequest>,
) -> Result<Json<Profile>, ApiError> {
    let tenant = TenantId(body.tenant_id);
    if !identity::switch_tenant(&state.pool, &auth.session, tenant).await? {
        return Err(ApiError::forbidden("you are not a member of this tenant"));
    }
    Ok(Json(identity::load_profile(&state.pool, auth.session.user, Some(tenant)).await?))
}
