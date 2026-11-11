use crate::auth::{Authenticated, removal_cookie, session_cookie};
use crate::error::{ApiError, validate};
use crate::extract::ApiJson;
use crate::public::{ClientIp, limits};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use db::TenantId;
use garde::Validate;
use identity::{AuthenticateError, Profile, SignupError, SignupInput};
use serde::Deserialize;
use std::net::IpAddr;
use utoipa::ToSchema;
use uuid::Uuid;

/// Sign-in attempts one IP may make a minute, whatever the emails. The per-email throttle stays besides.
pub const LOGINS_PER_MINUTE: u32 = 20;
/// Sign-ups one IP may make a minute.
pub const SIGNUPS_PER_MINUTE: u32 = 5;
/// Requests carrying an emailed token (an invitation's preview and accept, an email verification) one IP may make a
/// minute, together.
pub const TOKEN_REQUESTS_PER_MINUTE: u32 = 20;

/// Counts a request of `scope` from `ip` against `per_minute` (`429` with `Retry-After` once over it), in the public
/// API's fixed windows, unless `AUTH_IP_LIMITS=off`.
pub(crate) async fn limit_ip(state: &AppState, scope: &str, ip: IpAddr, per_minute: u32) -> Result<(), ApiError> {
    if !state.auth_ip_limits {
        return Ok(());
    }
    limits::check(&state.pool, scope, ip, per_minute).await
}

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

/// 5 sign-ups a minute per IP (`429` with `Retry-After`). `503` with `Retry-After` when the server is hashing as many
/// passwords as it allows at once.
#[utoipa::path(post, path = "/api/v1/auth/signup", request_body = SignupRequest,
    responses((status = 201, body = Profile), (status = 409), (status = 422),
        (status = 429, headers(("Retry-After" = String))), (status = 503, headers(("Retry-After" = String)))))]
pub async fn signup(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    ApiJson(body): ApiJson<SignupRequest>,
) -> Result<(StatusCode, CookieJar, Json<Profile>), ApiError> {
    limit_ip(&state, "signup", ip, SIGNUPS_PER_MINUTE).await?;
    validate(&body)?;
    let input = SignupInput {
        email: body.email,
        password: body.password,
        display_name: body.display_name,
        tenant_name: body.tenant_name,
    };
    let (user, tenant) = identity::signup(&state.pool, input).await.map_err(|err| match err {
        SignupError::EmailTaken => ApiError::conflict(err.to_string()),
        SignupError::Busy(busy) => busy.into(),
        SignupError::Database(db_err) => db_err.into(),
    })?;
    let (token, expires) = identity::create_session(&state.pool, user, Some(tenant)).await?;
    let profile = identity::load_profile(&state.pool, user, Some(tenant)).await?;
    Ok((StatusCode::CREATED, jar.add(session_cookie(token, expires, state.production)), Json(profile)))
}

/// 20 attempts a minute per IP (`429` with `Retry-After`). After 5 failed attempts for one email within 15 minutes,
/// further attempts for that email are answered with 429 until the window passes, whether or not the account exists.
/// A successful sign-in clears them. `503` with `Retry-After` when the server is hashing as many passwords as it allows
/// at once.
#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
    responses((status = 200, body = Profile), (status = 401), (status = 429, headers(("Retry-After" = String))),
        (status = 503, headers(("Retry-After" = String)))))]
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    ApiJson(body): ApiJson<LoginRequest>,
) -> Result<(CookieJar, Json<Profile>), ApiError> {
    limit_ip(&state, "login", ip, LOGINS_PER_MINUTE).await?;
    if !identity::reserve_login_attempt(&state.pool, &body.email).await? {
        return Err(ApiError::too_many_requests("too many failed sign-in attempts; try again in 15 minutes"));
    }
    let user = identity::authenticate(&state.pool, &body.email, &body.password)
        .await
        .map_err(|err| match err {
            AuthenticateError::Busy(busy) => ApiError::from(busy),
            AuthenticateError::Database(db_err) => db_err.into(),
        })?
        .ok_or_else(ApiError::invalid_credentials)?;
    identity::clear_login_failures(&state.pool, &body.email).await?;
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
