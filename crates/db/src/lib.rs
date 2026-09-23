//! Database access shared by every module: pool setup, migrations, tenant-scoped
//! transactions and change notifications.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}
