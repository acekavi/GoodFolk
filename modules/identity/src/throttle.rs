use db::Scope;
use sqlx::PgPool;
use uuid::Uuid;

/// Sign-in attempts allowed per email within [`LOGIN_WINDOW_SECS`]; further attempts are refused until the
/// oldest of them is older than the window.
pub const MAX_LOGIN_FAILURES: i64 = 5;
pub const LOGIN_WINDOW_SECS: i32 = 15 * 60;

/// Reserves a sign-in attempt for `email`. Returns `None`, recording nothing, if [`MAX_LOGIN_FAILURES`]
/// attempts failed within the window. Otherwise the attempt counts as a failure until
/// [`clear_login_failures`] is called after a successful sign-in, or [`release_login_attempt`] when the password
/// could not be checked at all.
///
/// The attempt is recorded before the password is checked, under a per-email lock, so concurrent attempts
/// cannot exceed the limit.
pub async fn reserve_login_attempt(pool: &PgPool, email: &str) -> Result<Option<Uuid>, sqlx::Error> {
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
        return Ok(None);
    }
    // Older attempts no longer count, so they are pruned here.
    sqlx::query("delete from login_failure where email = $1::citext and at <= now() - make_interval(secs => $2)")
        .bind(email)
        .bind(LOGIN_WINDOW_SECS)
        .execute(&mut *tx)
        .await?;
    let attempt = Uuid::now_v7();
    sqlx::query("insert into login_failure (id, email) values ($1, $2::citext)")
        .bind(attempt)
        .bind(email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(attempt))
}

/// Takes back the attempt [`reserve_login_attempt`] recorded, when the password was never checked (the server was too
/// busy to hash it): that is not a failed sign-in, so it must not count toward the lockout.
pub async fn release_login_attempt(pool: &PgPool, attempt: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("delete from login_failure where id = $1").bind(attempt).execute(pool).await?;
    Ok(())
}

/// Forgets `email`'s failed attempts, after a successful sign-in.
pub async fn clear_login_failures(pool: &PgPool, email: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from login_failure where email = $1::citext").bind(email).execute(pool).await?;
    Ok(())
}
