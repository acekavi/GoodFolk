//! Staff: invitations, members' grants and revocation (`StaffManage`, with the manager rules checked by `identity`),
//! and the public routes an invited person previews and accepts an invitation through. An invitation's token is
//! never returned or logged: it reaches the invited person only in the email's link.

use crate::auth::{SESSION_COOKIE, TenantContext, session_cookie};
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, staff_error};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use db::Scope;
use identity::{Acceptor, GrantSpec, Invitation, Member, Permission, Profile, Role};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateInvitationRequest {
    pub email: String,
    /// 1 to 200 characters: the new account's name.
    pub display_name: String,
    /// 1 to 50 grants; `owner` is never given by invitation.
    pub grants: Vec<GrantSpec>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetGrantsRequest {
    /// Replaces every grant the member holds: 1 to 50 of them.
    pub grants: Vec<GrantSpec>,
}

/// The token from an invitation's link.
// No `Debug`, so it cannot be logged.
#[derive(Deserialize, ToSchema)]
pub struct InvitationTokenRequest {
    pub token: String,
}

// No `Debug`: it carries the token and a password.
#[derive(Deserialize, ToSchema)]
pub struct AcceptInvitationRequest {
    pub token: String,
    /// Needed when no account has the invitation's email (12 to 128 characters, as at sign-up), and ignored when the
    /// request comes with a session.
    pub password: Option<String>,
}

/// One role an invitation gives.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewGrant {
    pub role: Role,
    /// `null` for all properties.
    pub property_name: Option<String>,
}

/// What the invitation page shows before accepting.
#[derive(Debug, Serialize, ToSchema)]
pub struct InvitationPreview {
    pub tenant_name: String,
    /// `null` once the inviter's account is deleted.
    pub inviter: Option<String>,
    pub email: String,
    pub grants: Vec<PreviewGrant>,
    /// An account has the email: the page asks the person to sign in as it, not for a password.
    pub existing_account: bool,
}

impl From<identity::Preview> for InvitationPreview {
    fn from(p: identity::Preview) -> Self {
        Self {
            tenant_name: p.tenant_name,
            inviter: p.inviter,
            email: p.email,
            grants: p.grants.into_iter().map(|(role, property_name)| PreviewGrant { role, property_name }).collect(),
            existing_account: p.existing_account,
        }
    }
}

/// A transaction in the caller's tenant, acting as them.
async fn begin(state: &AppState, ctx: &TenantContext) -> Result<db::Tx, ApiError> {
    Ok(db::begin(&state.pool, Scope { tenant: Some(ctx.tenant), user: Some(ctx.user) }).await?)
}

#[utoipa::path(post, operation_id = "create_invitation", path = "/api/v1/invitations", request_body = CreateInvitationRequest,
    params(("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Invitation), (status = 403), (status = 409), (status = 422)))]
pub async fn invite(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiJson(body): ApiJson<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<Invitation>), ApiError> {
    ctx.require_somewhere(Permission::StaffManage)?;
    let mut tx = begin(&state, &ctx).await?;
    let invitation = identity::invite(&mut tx, ctx.tenant, ctx.user, &body.email, &body.display_name, &body.grants)
        .await
        .map_err(staff_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(invitation)))
}

/// Sends a pending or expired invitation again with a new link, valid for 7 days from now. The old link stops working.
#[utoipa::path(post, operation_id = "resend_invitation", path = "/api/v1/invitations/{invitation}/resend",
    params(("invitation" = Uuid, Path)),
    responses((status = 200, body = Invitation), (status = 403), (status = 404), (status = 409)))]
pub async fn resend(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(invitation): ApiPath<Uuid>,
) -> Result<Json<Invitation>, ApiError> {
    ctx.require_somewhere(Permission::StaffManage)?;
    let mut tx = begin(&state, &ctx).await?;
    let resent = identity::resend(&mut tx, ctx.tenant, ctx.user, invitation).await.map_err(staff_error)?;
    tx.commit().await?;
    Ok(Json(resent))
}

/// Revokes a pending invitation: its link answers 410.
#[utoipa::path(post, operation_id = "revoke_invitation", path = "/api/v1/invitations/{invitation}/revoke",
    params(("invitation" = Uuid, Path)),
    responses((status = 200, body = Invitation), (status = 403), (status = 404), (status = 409)))]
pub async fn revoke(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(invitation): ApiPath<Uuid>,
) -> Result<Json<Invitation>, ApiError> {
    ctx.require_somewhere(Permission::StaffManage)?;
    let mut tx = begin(&state, &ctx).await?;
    let revoked = identity::revoke_invitation(&mut tx, ctx.tenant, ctx.user, invitation).await.map_err(staff_error)?;
    tx.commit().await?;
    Ok(Json(revoked))
}

/// Replaces a member's grants. They take effect on the member's next request.
#[utoipa::path(put, operation_id = "set_member_grants", path = "/api/v1/members/{user}/grants", request_body = SetGrantsRequest,
    params(("user" = Uuid, Path), ("If-Match" = String, Header, description = "the member's version, e.g. \"3\"")),
    responses((status = 200, body = Member,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn set_grants(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(user): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SetGrantsRequest>,
) -> Result<Versioned<Member>, ApiError> {
    ctx.require_somewhere(Permission::StaffManage)?;
    let mut tx = begin(&state, &ctx).await?;
    let member =
        identity::set_grants(&mut tx, ctx.tenant, ctx.user, user, version, &body.grants).await.map_err(staff_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(member.version, member))
}

/// Removes a member from the tenant and ends their sessions in it: their next request is 401.
#[utoipa::path(delete, operation_id = "revoke_member", path = "/api/v1/members/{user}",
    params(("user" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 409)))]
pub async fn revoke_member(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(user): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    ctx.require_somewhere(Permission::StaffManage)?;
    let mut tx = begin(&state, &ctx).await?;
    identity::revoke_member(&mut tx, ctx.tenant, ctx.user, user).await.map_err(staff_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The sign-in throttle's key for an invitation token: the token's SHA-256, under a prefix that no email (which has
/// an `@`) can collide with.
fn throttle_key(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().fold(String::from("invitation:"), |mut key, byte| {
        // Writing to a `String` cannot fail.
        let _ = write!(key, "{byte:02x}");
        key
    })
}

/// Reserves an attempt with `token` under the sign-in throttle, returning its key to clear on success.
async fn reserve_attempt(state: &AppState, token: &str) -> Result<String, ApiError> {
    let key = throttle_key(token);
    if !identity::reserve_login_attempt(&state.pool, &key).await? {
        return Err(ApiError::too_many_requests("too many attempts with this invitation; try again in 15 minutes"));
    }
    Ok(key)
}

/// Public. An unknown, used, expired or revoked token is 410 "This invitation is no longer valid". After 5 failed
/// attempts with one token within 15 minutes, further ones are 429 until the window passes.
#[utoipa::path(post, operation_id = "preview_invitation", path = "/api/v1/invitations/preview", request_body = InvitationTokenRequest,
    responses((status = 200, body = InvitationPreview), (status = 410), (status = 429)))]
pub async fn preview(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<InvitationTokenRequest>,
) -> Result<Json<InvitationPreview>, ApiError> {
    let key = reserve_attempt(&state, &body.token).await?;
    let preview = identity::preview(&state.pool, &body.token).await.map_err(staff_error)?;
    identity::clear_login_failures(&state.pool, &key).await?;
    Ok(Json(preview.into()))
}

/// Public. With a session, accepts as its user (whose email must be the invitation's) and switches the session to the
/// tenant. Without one, creates the account with `password` (409 "Sign in as <email> to accept" if the email already
/// has one) and signs it in. Answers 410 and 429 as preview does.
#[utoipa::path(post, operation_id = "accept_invitation", path = "/api/v1/invitations/accept", request_body = AcceptInvitationRequest,
    responses((status = 200, body = Profile), (status = 403), (status = 409), (status = 410), (status = 422), (status = 429)))]
pub async fn accept(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(body): ApiJson<AcceptInvitationRequest>,
) -> Result<(CookieJar, Json<Profile>), ApiError> {
    let key = reserve_attempt(&state, &body.token).await?;
    let session = match jar.get(SESSION_COOKIE) {
        Some(cookie) => identity::resolve_session(&state.pool, cookie.value()).await?,
        None => None,
    };
    let acceptor = match (&session, body.password) {
        (Some(session), _) => Acceptor::SignedIn(*session),
        (None, Some(password)) => Acceptor::NewUser { password },
        (None, None) => return Err(ApiError::unprocessable("set a password, or sign in, to accept")),
    };
    let (user, tenant) = identity::accept(&state.pool, &body.token, acceptor).await.map_err(staff_error)?;
    identity::clear_login_failures(&state.pool, &key).await?;
    // A signed-in session was switched to the tenant in the accept's own transaction.
    let jar = match session {
        Some(_) => jar,
        None => {
            let (token, expires) = identity::create_session(&state.pool, user, Some(tenant)).await?;
            jar.add(session_cookie(token, expires, state.production))
        }
    };
    Ok((jar, Json(identity::load_profile(&state.pool, user, Some(tenant)).await?)))
}

#[cfg(test)]
mod tests {
    use super::throttle_key;

    #[test]
    fn the_throttle_key_is_the_tokens_hash_and_never_an_email() {
        let key = throttle_key("a-token");

        assert!(key.starts_with("invitation:") && !key.contains('@') && !key.contains("a-token"), "{key}");
        assert_eq!(key.len(), "invitation:".len() + 64);
        assert_eq!(key, throttle_key("a-token"));
        assert_ne!(key, throttle_key("another-token"));
    }
}
