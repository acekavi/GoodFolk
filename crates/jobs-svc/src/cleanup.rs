//! The daily cleanup: expired sessions, sign-in failures older than the throttle window, and idempotency keys older
//! than a week, across tenants through `app.purge_expired`.

use sqlx::PgPool;

/// How long an idempotency key is kept, in days.
const IDEMPOTENCY_DAYS: i32 = 7;

/// Purges the expired rows and logs how many went.
pub(crate) async fn run(pool: &PgPool) -> Result<(), String> {
    let purged: Result<(i64, i64, i64), sqlx::Error> =
        sqlx::query_as("select sessions, login_failures, idempotency_keys from app.purge_expired($1, $2)")
            .bind(identity::LOGIN_WINDOW_SECS)
            .bind(IDEMPOTENCY_DAYS)
            .fetch_one(pool)
            .await;
    let (sessions, login_failures, idempotency_keys) = purged.map_err(|error| error.to_string())?;
    tracing::info!(sessions, login_failures, idempotency_keys, "cleanup done");
    Ok(())
}
