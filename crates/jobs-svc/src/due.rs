//! When a property's business date is due to close, in its own local time.

use time::macros::time;
use time::{Date, Duration, PrimitiveDateTime, Time};

/// From this audit time on, the audit moment falls on the business date itself rather than the day after.
const NOON: Time = time!(12:00);

/// The audit moment of business date `d`: d at `audit_time` when it is 12:00 or later, else d + 1 at `audit_time`.
pub fn audit_moment(d: Date, audit_time: Time) -> PrimitiveDateTime {
    let day = if audit_time >= NOON { d } else { d.saturating_add(Duration::DAY) };
    PrimitiveDateTime::new(day, audit_time)
}

pub fn audit_due(local_now: PrimitiveDateTime, d: Date, audit_time: Time) -> bool {
    local_now >= audit_moment(d, audit_time)
}

#[cfg(test)]
mod tests {
    use super::audit_due;
    use time::macros::{date, datetime, time};

    const D: time::Date = date!(2026 - 10 - 20);

    #[test]
    fn an_audit_time_before_noon_falls_on_the_next_day() {
        assert!(audit_due(datetime!(2026-10-21 03:00), D, time!(03:00)));
        assert!(!audit_due(datetime!(2026-10-21 02:59), D, time!(03:00)));
        assert!(!audit_due(datetime!(2026-10-20 23:59), D, time!(03:00)));
        assert!(audit_due(datetime!(2026-10-21 00:00), D, time!(00:00)));
        assert!(!audit_due(datetime!(2026-10-20 23:59), D, time!(00:00)));
    }

    #[test]
    fn an_audit_time_from_noon_falls_on_the_day_itself() {
        assert!(audit_due(datetime!(2026-10-20 23:30), D, time!(23:30)));
        assert!(!audit_due(datetime!(2026-10-20 23:29), D, time!(23:30)));
        assert!(audit_due(datetime!(2026-10-20 12:00), D, time!(12:00)));
        assert!(!audit_due(datetime!(2026-10-20 11:59), D, time!(12:00)));
    }
}
