use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

pub type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

/// Who a transaction acts for. Row-level security policies read these values.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub tenant: Option<TenantId>,
    pub user: Option<UserId>,
}

impl Scope {
    pub fn tenant(tenant: TenantId) -> Self {
        Self { tenant: Some(tenant), user: None }
    }

    pub fn user(user: UserId) -> Self {
        Self { tenant: None, user: Some(user) }
    }
}

/// Begins a transaction whose row-level security context is `scope`.
///
/// The settings are transaction-local (`set_config(.., true)`), so they end with the
/// transaction and can never leak to the next user of a pooled connection.
pub async fn begin(pool: &PgPool, scope: Scope) -> Result<Tx, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("select set_config('app.tenant_id', $1, true), set_config('app.user_id', $2, true)")
        .bind(scope.tenant.map(|t| t.0.to_string()).unwrap_or_default())
        .bind(scope.user.map(|u| u.0.to_string()).unwrap_or_default())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
