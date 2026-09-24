use db::Scope;
use sqlx::PgPool;
use uuid::Uuid;

/// Sign-in attempts allowed per email within [`LOGIN_WINDOW_SECS`]; further attempts are refused until the
/// oldest of them is older than the window.
pub const MAX_LOGIN_FAILURES: i64 = 5;
pub const LOGIN_WINDOW_SECS: f64 = 15.0 * 60.0;

/// Reserves a sign-in attempt for `email`. Returns `false`, recording nothing, if [`MAX_LOGIN_FAILURES`]
/// attempts failed within the window. Otherwise the attempt counts as a failure until
/// [`clear_login_failures`] is called after a successful sign-in.
///
/// The attempt is recorded before the password is checked, under a per-email lock, so concurrent attempts
/// cannot exceed the limit.
pub async fn reserve_login_attempt(pool: &PgPool, email: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(pool, Scope::default()).await?;
    sqlx::query("select pg_advisory_xact_lock(hashtextextended(lower($1::text), 0))")
        .bind(email)
        .execute(&mut *tx)
        .await?;
    let recent: i64 = sqlx::query_scalar(
        "select count(*) from login_failure where email = $1::citext and at > now() - make_interval(secs => $2)",
    )
    .bind(email)
    .bind(LOGIN_WINDOW_SECS)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= MAX_LOGIN_FAILURES {
        tx.commit().await?;
        return Ok(false);
    }
    // Older attempts no longer count, so they are pruned here.
    sqlx::query("delete from login_failure where email = $1::citext and at <= now() - make_interval(secs => $2)")
        .bind(email)
        .bind(LOGIN_WINDOW_SECS)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into login_failure (id, email) values ($1, $2::citext)")
        .bind(Uuid::now_v7())
        .bind(email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Forgets `email`'s failed attempts, after a successful sign-in.
pub async fn clear_login_failures(pool: &PgPool, email: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from login_failure where email = $1::citext").bind(email).execute(pool).await?;
    Ok(())
}
