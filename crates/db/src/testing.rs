//! Test helpers. Enabled with the `testing` feature, for dev-dependencies only.

use crate::crypto::{GuestIdKey, GuestIdKeys};
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

/// A fixed guest ID key for tests (base64 of 32 bytes). Never use it outside tests.
pub const GUEST_ID_KEY_B64: &str = crate::crypto::TEST_KEY_B64;

/// The test guest ID key, with key id `k1`.
pub fn guest_id_key() -> GuestIdKey {
    GuestIdKey::from_base64("k1", GUEST_ID_KEY_B64).expect("valid test key")
}

/// A keyring holding only [`guest_id_key`], for tests that need a [`GuestIdKeys`] rather than a bare key.
pub fn guest_id_keys() -> GuestIdKeys {
    GuestIdKeys::new(guest_id_key(), Vec::new()).expect("a single key has no id to collide with")
}
