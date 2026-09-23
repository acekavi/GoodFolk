use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::{TenantId, UserId};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Staff sessions last one working day plus margin.
pub const SESSION_TTL: Duration = Duration::hours(14);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: Uuid,
    pub user: UserId,
    pub tenant: Option<TenantId>,
}

/// Only the SHA-256 of a token is stored, so a database leak does not leak live sessions.
fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Creates a session and returns the opaque token for the cookie.
pub async fn create_session(
    pool: &PgPool,
    user: UserId,
    tenant: Option<TenantId>,
) -> Result<(String, OffsetDateTime), sqlx::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator available");
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let expires_at = OffsetDateTime::now_utc() + SESSION_TTL;
    sqlx::query(
        "insert into session (id, token_hash, user_id, active_tenant_id, expires_at) values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(token_hash(&token))
    .bind(user.0)
    .bind(tenant.map(|t| t.0))
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

/// Returns the live session for `token`, or `None` if it is unknown or expired.
pub async fn resolve_session(pool: &PgPool, token: &str) -> Result<Option<SessionInfo>, sqlx::Error> {
    let row: Option<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "select id, user_id, active_tenant_id from session where token_hash = $1 and expires_at > now()",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, user, tenant)| SessionInfo { id, user: UserId(user), tenant: tenant.map(TenantId) }))
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from session where token_hash = $1").bind(token_hash(token)).execute(pool).await?;
    Ok(())
}

/// Points the session at `tenant`. Returns `false` if the user is not a member of it.
pub async fn switch_tenant(pool: &PgPool, session: &SessionInfo, tenant: TenantId) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(pool, db::Scope::user(session.user)).await?;
    let member: bool =
        sqlx::query_scalar("select exists (select 1 from membership where tenant_id = $1 and user_id = $2)")
            .bind(tenant.0)
            .bind(session.user.0)
            .fetch_one(&mut *tx)
            .await?;
    if member {
        sqlx::query("update session set active_tenant_id = $1 where id = $2")
            .bind(tenant.0)
            .bind(session.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(member)
}
