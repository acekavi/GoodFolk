//! Per-IP limits of the public API, counted in Postgres (`ibe_rate_hit`) in fixed one-minute windows, so every
//! instance shares them.

use crate::error::ApiError;
use sqlx::PgPool;
use std::net::{IpAddr, Ipv6Addr};

/// Searches one IP may make a minute.
pub const SEARCHES_PER_MINUTE: u32 = 60;
/// Bookings one IP may send a minute.
pub const BOOKINGS_PER_MINUTE: u32 = 5;
/// Manage page views and cancellations one IP may send a minute, together.
pub const MANAGES_PER_MINUTE: u32 = 5;

/// What a client is counted under: an IPv4 address on its own, an IPv6 one by its /64 (one subscriber's network, whose
/// addresses a client can rotate through freely), as `inet` text. An IPv4-mapped IPv6 address counts as the IPv4 one.
fn limit_key(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            format!("{}/64", Ipv6Addr::from(segments))
        }
    }
}

/// Counts a request of `scope` from `ip` (by [`limit_key`]) in the current minute, and refuses it (`429` with `Retry-After`, the seconds
/// to the minute's end) once it is over `per_minute`. Refused requests count too, so retrying early does not help.
pub async fn check(pool: &PgPool, scope: &str, ip: IpAddr, per_minute: u32) -> Result<(), ApiError> {
    let (hits, seconds_left): (i32, i32) = sqlx::query_as(
        "insert into ibe_rate_hit (scope, ip, window_start, hits)
         values ($1, $2::inet, date_trunc('minute', now()), 1)
         on conflict (scope, ip, window_start) do update set hits = ibe_rate_hit.hits + 1
         returning hits, ceil(extract(epoch from window_start + interval '1 minute' - now()))::int",
    )
    .bind(scope)
    .bind(limit_key(ip))
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

#[cfg(test)]
mod tests {
    use super::limit_key;

    #[test]
    fn ipv4_counts_by_address_and_ipv6_by_its_64() {
        let key = |ip: &str| limit_key(ip.parse().unwrap());

        assert_eq!(key("203.0.113.9"), "203.0.113.9");
        assert_eq!(key("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"), "2001:db8:1:2::/64");
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:ffff::7"));
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }
}
