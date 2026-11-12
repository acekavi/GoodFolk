//! Database access shared by every module: pool setup, migrations, tenant-scoped
//! transactions, change notifications, encryption of guest ID numbers, and [`text_enum!`] for `text`
//! columns with a fixed set of values.

pub mod crypto;
mod events;
mod guard;
mod scope;
#[cfg(feature = "testing")]
pub mod testing;
mod text_enum;

pub use events::{CHANNEL, Event, notify};
pub use guard::{RlsBypassed, assert_rls_applies};
pub use scope::{Scope, TenantId, Tx, UserId, begin};

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    pool_options().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}

/// How long a pooled connection may sit idle before it is pinged on its way out of the pool.
const PING_AFTER_IDLE: Duration = Duration::from_secs(5);

/// The pool options every service uses. A connection idle for more than [`PING_AFTER_IDLE`] is pinged before it is
/// handed out, so one the server closed (Neon suspends an idle compute, a proxy drops an idle connection) is replaced
/// rather than failing the request; one that has just been used is not, which saves a round trip on every
/// transaction of a busy service.
pub fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new().test_before_acquire(false).before_acquire(|conn, meta| {
        Box::pin(async move {
            if meta.idle_for > PING_AFTER_IDLE {
                sqlx::Connection::ping(conn).await?;
            }
            Ok(true)
        })
    })
}

/// The enabled booking site that `code` (a property code, any case) or `host` (a custom host) names, as its tenant and
/// property. `None` when the site is disabled or nothing matches. Runs before any tenant is known, through
/// `app.ibe_resolve`.
pub async fn ibe_resolve(
    pool: &PgPool,
    code: Option<&str>,
    host: Option<&str>,
) -> Result<Option<(TenantId, uuid::Uuid)>, sqlx::Error> {
    let row: Option<(uuid::Uuid, uuid::Uuid)> =
        sqlx::query_as("select tenant_id, property_id from app.ibe_resolve($1, $2)")
            .bind(code)
            .bind(host)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(tenant, property)| (TenantId(tenant), property)))
}
