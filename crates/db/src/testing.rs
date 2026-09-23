//! Test helpers. Enabled with the `testing` feature, for dev-dependencies only.

use sqlx::Executor;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

/// A pool acting as `goodfolk_app`, so row-level security applies
/// exactly as in production. (`#[sqlx::test]` connects as a superuser, which bypasses RLS.)
pub async fn app_pool(opts: PgConnectOptions, max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .after_connect(|conn, _| {
            Box::pin(async move {
                conn.execute("set role goodfolk_app").await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await
        .expect("connect test pool")
}
