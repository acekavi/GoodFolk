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
    PgPoolOptions::new().max_connections(max_connections).acquire_timeout(Duration::from_secs(3)).connect(url).await
}
