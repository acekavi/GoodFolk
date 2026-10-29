//! Per-IP limits of the public API, counted in Postgres (`ibe_rate_hit`) in fixed one-minute windows, so every
//! instance shares them.

use crate::error::ApiError;
use sqlx::PgPool;
use std::net::IpAddr;

/// Searches one IP may make a minute.
pub const SEARCHES_PER_MINUTE: u32 = 60;

/// Counts a request of `scope` from `ip` in the current minute, and refuses it (`429` with `Retry-After`, the seconds
/// to the minute's end) once it is over `per_minute`. Refused requests count too, so retrying early does not help.
pub async fn check(pool: &PgPool, scope: &str, ip: IpAddr, per_minute: u32) -> Result<(), ApiError> {
    let (hits, seconds_left): (i32, i32) = sqlx::query_as(
        "insert into ibe_rate_hit (scope, ip, window_start, hits)
         values ($1, $2::inet, date_trunc('minute', now()), 1)
         on conflict (scope, ip, window_start) do update set hits = ibe_rate_hit.hits + 1
         returning hits, ceil(extract(epoch from window_start + interval '1 minute' - now()))::int",
    )
    .bind(scope)
    .bind(ip.to_string())
    .fetch_one(pool)
    .await?;
    if u32::try_from(hits).unwrap_or(u32::MAX) <= per_minute {
        return Ok(());
    }
    // At least a second: a client told 0 would come straight back into the same window.
    let retry_after = u64::try_from(seconds_left.max(1)).unwrap_or(1);
    Err(ApiError::too_many_requests("Too many requests from this address; try again in a minute")
        .retry_after(retry_after))
}
