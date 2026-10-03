use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use db::{TenantId, UserId};
use identity::{Grant, Permission, SessionInfo, TenantAccess, allows, resolve_session, resolve_tenant_access};
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

    /// `None` if the user may see every property, otherwise the properties they hold a viewing grant for.
    pub fn visible_properties(&self) -> Option<Vec<Uuid>> {
        if allows(&self.grants, Permission::PropertiesView, None) {
            None
        } else {
            Some(
                self.grants
                    .iter()
                    .filter(|g| allows(&[**g], Permission::PropertiesView, g.property_id))
                    .filter_map(|g| g.property_id)
                    .collect(),
            )
        }
    }
}

impl FromRequestParts<AppState> for TenantContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(found) = parts.extensions.get::<TenantContext>() {
            return Ok(found.clone());
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned()).ok_or_else(ApiError::unauthenticated)?;
        // One round trip: the session, the user's membership in its tenant (they may have been removed since the
        // tenant was chosen) and their grants there.
        let (session, tenant, grants) = match resolve_tenant_access(&state.pool, &token).await? {
            TenantAccess::NoSession => return Err(ApiError::unauthenticated()),
            TenantAccess::NoTenant | TenantAccess::NotMember => return Err(ApiError::forbidden("no tenant selected")),
            TenantAccess::Member { session, tenant, grants } => (session, tenant, grants),
        };
        parts.extensions.insert(Authenticated { session, token });
        let found = TenantContext { user: session.user, tenant, grants };
        parts.extensions.insert(found.clone());
        Ok(found)
    }
}
