use crate::rbac::{Grant, Role, load_grants};
use crate::{hash_password, verify_password};
use db::{Scope, TenantId, UserId};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::LazyLock;
use uuid::Uuid;

pub struct SignupInput {
    pub email: String,
    pub password: String,
    pub display_name: String,
    pub tenant_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SignupError {
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Creates a user, their tenant, a membership and an owner grant in one transaction.
pub async fn signup(pool: &PgPool, input: SignupInput) -> Result<(UserId, TenantId), SignupError> {
    let user = UserId(Uuid::now_v7());
    let tenant = TenantId(Uuid::now_v7());
    let password = input.password;
    let hash =
        tokio::task::spawn_blocking(move || hash_password(&password)).await.expect("hashing task does not panic");

    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user: Some(user) }).await?;
    let inserted = sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, $3, $4)")
        .bind(user.0)
        .bind(&input.email)
        .bind(&hash)
        .bind(&input.display_name)
        .execute(&mut *tx)
        .await;
    if let Err(err) = inserted {
        let taken = err
            .as_database_error()
            .and_then(|db_err| db_err.constraint())
            .is_some_and(|constraint| constraint == "app_user_email_key");
        return Err(if taken { SignupError::EmailTaken } else { err.into() });
    }
    sqlx::query("insert into tenant (id, name) values ($1, $2)")
        .bind(tenant.0)
        .bind(&input.tenant_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(tenant.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, null, $4)")
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .bind(Role::Owner.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id) values ($1, $2, $3, 'tenant.created', 'tenant', $2)")
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((user, tenant))
}

/// Verified against when an email is unknown, so response time does not reveal which emails exist.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| hash_password("dummy password for timing"));

/// Returns the user if the email and password match.
pub async fn authenticate(pool: &PgPool, email: &str, password: &str) -> Result<Option<UserId>, sqlx::Error> {
    let row: Option<(Uuid, String)> = sqlx::query_as("select id, password_hash from app_user where email = $1::citext")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    let password = password.to_owned();
    let (user, stored_hash) = match row {
        Some((id, hash)) => (Some(UserId(id)), Some(hash)),
        None => (None, None),
    };
    // DUMMY_HASH is computed on first use, so it is only touched on the blocking pool.
    let valid = tokio::task::spawn_blocking(move || {
        verify_password(&password, stored_hash.as_deref().unwrap_or(DUMMY_HASH.as_str()))
    })
    .await
    .expect("verification task does not panic");
    Ok(user.filter(|_| valid))
}

/// Returns a tenant the user belongs to, for new sessions. Tenants are listed oldest first.
pub async fn default_tenant(pool: &PgPool, user: UserId) -> Result<Option<TenantId>, sqlx::Error> {
    let mut tx = db::begin(pool, Scope::user(user)).await?;
    let tenant: Option<Uuid> =
        sqlx::query_scalar("select tenant_id from membership where user_id = $1 order by created_at limit 1")
            .bind(user.0)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(tenant.map(TenantId))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TenantSummary {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Profile {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub tenants: Vec<TenantSummary>,
    pub current_tenant: Option<Uuid>,
    pub grants: Vec<Grant>,
}

pub async fn load_profile(pool: &PgPool, user: UserId, current: Option<TenantId>) -> Result<Profile, sqlx::Error> {
    let mut tx = db::begin(pool, Scope { tenant: current, user: Some(user) }).await?;
    let (email, display_name): (String, String) =
        sqlx::query_as("select email::text, display_name from app_user where id = $1")
            .bind(user.0)
            .fetch_one(&mut *tx)
            .await?;
    let tenants: Vec<(Uuid, String)> = sqlx::query_as(
        "select t.id, t.name from tenant t join membership m on m.tenant_id = t.id
         where m.user_id = $1 order by t.name",
    )
    .bind(user.0)
    .fetch_all(&mut *tx)
    .await?;
    let grants = if current.is_some() { load_grants(&mut tx, user).await? } else { Vec::new() };
    tx.commit().await?;
    Ok(Profile {
        user_id: user.0,
        email,
        display_name,
        tenants: tenants.into_iter().map(|(id, name)| TenantSummary { id, name }).collect(),
        current_tenant: current.map(|t| t.0),
        grants,
    })
}
