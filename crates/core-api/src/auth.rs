use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use db::{Scope, TenantId, UserId};
use identity::{Grant, Permission, SessionInfo, allows, load_grants, resolve_session};
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "gf_session";

pub fn session_cookie(token: String, expires: time::OffsetDateTime, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .expires(expires)
        .build()
}

pub fn removal_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, "")).path("/").build()
}

/// A signed-in user, from the session cookie.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub session: SessionInfo,
    pub token: String,
}

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<Authenticated>() {
            return Ok(found.clone());
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned()).ok_or_else(ApiError::unauthenticated)?;
        let session = resolve_session(&state.pool, &token).await?.ok_or_else(ApiError::unauthenticated)?;
        let found = Authenticated { session, token };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}

/// A signed-in user acting in their session's current tenant, with their grants loaded.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub user: UserId,
    pub tenant: TenantId,
    pub grants: Vec<Grant>,
}

impl TenantContext {
    pub fn require(&self, permission: Permission, property: Option<Uuid>) -> Result<(), ApiError> {
        if allows(&self.grants, permission, property) {
            Ok(())
        } else {
            Err(ApiError::forbidden("you do not have permission for this action"))
        }
    }

    /// `None` if the user may see every property, otherwise the properties they hold grants for.
    pub fn visible_properties(&self) -> Option<Vec<Uuid>> {
        if allows(&self.grants, Permission::PropertiesView, None) {
            None
        } else {
            Some(self.grants.iter().filter_map(|g| g.property_id).collect())
        }
    }
}

impl FromRequestParts<AppState> for TenantContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<TenantContext>() {
            return Ok(found.clone());
        }
        let auth = Authenticated::from_request_parts(parts, state).await?;
        let tenant = auth.session.tenant.ok_or_else(|| ApiError::forbidden("no tenant selected"))?;
        let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
        let grants = load_grants(&mut tx, auth.session.user).await?;
        tx.commit().await?;
        let found = TenantContext { user: auth.session.user, tenant, grants };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}
