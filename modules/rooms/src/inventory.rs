//! `inventory_day` counters: one row per room type per day, from the business date for [`WINDOW_DAYS`].
//!
//! `physical` counts a type's active rooms; `out_of_order` counts its active rooms under an active
//! out-of-order block that day; `sold` counts its booked rooms that night (reservations). Writers adjust the
//! counters in their own transaction; [`find_drift`] recomputes them from rooms, blocks and reservations, for
//! tests and the nightly check (Phase 7).

use crate::RoomsError;
use db::Tx;
use serde::Serialize;
use time::{Date, Duration};
use uuid::Uuid;

/// Counter rows exist for `[business date, business date + WINDOW_DAYS)`.
pub const WINDOW_DAYS: i64 = 730;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct InventoryDay {
    pub room_type_id: Uuid,
    pub date: Date,
    pub physical: i32,
    pub sold: i32,
    pub out_of_order: i32,
}

impl InventoryDay {
    /// Physical rooms of this type free that day (`physical - sold - out_of_order`). This is the plain
    /// physical figure and never includes the room type's overbooking allowance; the `reservations` crate
    /// applies the allowance itself wherever a night's sellability decides whether a booking succeeds
    /// (`physical - sold - out_of_order + overbooking`).
    pub fn available(&self) -> i32 {
        self.physical - self.sold - self.out_of_order
    }
}

/// A counter row that disagrees with a recomputation from rooms, blocks and reservations. `None` means the row
/// is missing (expected) or should not exist (actual).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InventoryDrift {
    pub room_type_id: Uuid,
    pub date: Date,
    pub expected_physical: Option<i32>,
    pub actual_physical: Option<i32>,
    pub expected_sold: Option<i32>,
    pub actual_sold: Option<i32>,
    pub expected_out_of_order: Option<i32>,
    pub actual_out_of_order: Option<i32>,
}

/// The property's business date. `NotFound` if the property is not in this tenant.
pub(crate) async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, RoomsError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RoomsError::NotFound("property"))
}

/// Creates missing counter rows up to the end of the window, computed from rooms and blocks. Cheap when
/// nothing is missing. Until the nightly job exists (Phase 7), writers call it before changing counters.
pub async fn extend_window(tx: &mut Tx, property: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into inventory_day (tenant_id, property_id, room_type_id, date, physical, out_of_order)
         select rt.tenant_id, rt.property_id, rt.id, day.date,
                (select count(*) from room r where r.room_type_id = rt.id and r.active),
                (select count(*) from room_block b join room r on r.id = b.room_id
                 where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
                   and b.released_at is null and b.period @> day.date)
         from room_type rt
         join property p on p.id = rt.property_id
         cross join lateral (select max(i.date) as last from inventory_day i
                             where i.property_id = rt.property_id and i.room_type_id = rt.id) existing
         cross join lateral (select p.business_date + offset_days as date
                             from generate_series(greatest(0, existing.last - p.business_date + 1), $2 - 1) offset_days) day
         where rt.property_id = $1
         order by rt.id, day.date
         on conflict do nothing",
    )
    .bind(property)
    .bind(i32::try_from(WINDOW_DAYS).expect("window fits in i32"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Adds the deltas to `room_type`'s counters on each day in `[from, to)`. `from` must not be before the
/// business date: past days are history and never change.
pub(crate) async fn adjust(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    from: Date,
    to: Date,
    physical: i32,
    out_of_order: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update inventory_day set physical = physical + $5, out_of_order = out_of_order + $6
         where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
    )
    .bind(property)
    .bind(room_type)
    .bind(from)
    .bind(to)
    .bind(physical)
    .bind(out_of_order)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Locks `room_types`' counter rows on each day in `[from, to)`, in one statement. For counter writers only
/// (this crate and reservations): readers never lock counters.
///
/// Lock order: every command that changes counters calls this once, after its room and block row locks and
/// before its first counter update, covering every row it will change. Rows are locked in ascending
/// (room_type_id, date) order, so two such commands never wait on each other in a cycle.
pub async fn lock_days(
    tx: &mut Tx,
    property: Uuid,
    room_types: &[Uuid],
    from: Date,
    to: Date,
) -> Result<(), sqlx::Error> {
    let mut room_types = room_types.to_vec();
    room_types.sort_unstable();
    room_types.dedup();
    sqlx::query(
        "select 1 from inventory_day
         where property_id = $1 and room_type_id = any($2) and date >= $3 and date < $4
         order by room_type_id, date
         for update",
    )
    .bind(property)
    .bind(&room_types)
    .bind(from)
    .bind(to)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Adds (`sign = 1`) or removes (`sign = -1`) one room's share of its type's counters over the window from
/// `today`: one physical room, plus one out-of-order room on each day of its active out-of-order blocks.
/// Inactive rooms have no share. One statement changes both counters, so `out_of_order <= physical`
/// holds on every row after it.
pub(crate) async fn contribute(
    tx: &mut Tx,
    property: Uuid,
    today: Date,
    room: Uuid,
    room_type: Uuid,
    active: bool,
    sign: i32,
) -> Result<(), sqlx::Error> {
    if !active {
        return Ok(());
    }
    sqlx::query(
        "update inventory_day i
         set physical = i.physical + $5,
             out_of_order = i.out_of_order + $5 * (exists (
                 select 1 from room_block b
                 where b.room_id = $3 and b.released_at is null and b.kind = 'out_of_order' and b.period @> i.date
             ))::integer
         where i.property_id = $1 and i.room_type_id = $4 and i.date >= $2 and i.date < $6",
    )
    .bind(property)
    .bind(today)
    .bind(room)
    .bind(room_type)
    .bind(sign)
    .bind(today + Duration::days(WINDOW_DAYS))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Counter rows from the business date on that differ from a recomputation from rooms, blocks and
/// reservations. `sold` is expected to count the type's booked rooms whose stay covers the night and that are
/// neither cancelled nor no-shows.
pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrift>, sqlx::Error> {
    sqlx::query_as(
        "with expected as (
             select rt.id as room_type_id, day.date,
                    (select count(*) from room r where r.room_type_id = rt.id and r.active)::integer as physical,
                    (select count(*) from room_block b join room r on r.id = b.room_id
                     where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
                       and b.released_at is null and b.period @> day.date)::integer as out_of_order,
                    (select count(*) from reservation_room rr
                     where rr.property_id = rt.property_id and rr.room_type_id = rt.id
                       and rr.status not in ('cancelled', 'no_show') and rr.stay @> day.date)::integer as sold
             from room_type rt
             join property p on p.id = rt.property_id
             cross join lateral (select p.business_date + offset_days as date
                                 from generate_series(0, $2 - 1) offset_days) day
             where rt.property_id = $1
         ),
         actual as (
             select i.room_type_id, i.date, i.physical, i.sold, i.out_of_order
             from inventory_day i join property p on p.id = i.property_id
             where i.property_id = $1 and i.date >= p.business_date
         )
         select coalesce(e.room_type_id, a.room_type_id) as room_type_id, coalesce(e.date, a.date) as date,
                e.physical as expected_physical, a.physical as actual_physical,
                e.sold as expected_sold, a.sold as actual_sold,
                e.out_of_order as expected_out_of_order, a.out_of_order as actual_out_of_order
         from expected e full join actual a on a.room_type_id = e.room_type_id and a.date = e.date
         where e.physical is distinct from a.physical or e.sold is distinct from a.sold
            or e.out_of_order is distinct from a.out_of_order
         order by 2, 1",
    )
    .bind(property)
    .bind(i32::try_from(WINDOW_DAYS).expect("window fits in i32"))
    .fetch_all(&mut **tx)
    .await
}

/// Counters for every room type on each day in `[from, to)`, by date then room type.
pub async fn list_inventory(
    tx: &mut Tx,
    property: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<InventoryDay>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, physical, sold, out_of_order from inventory_day
         where property_id = $1 and date >= $2 and date < $3
         order by date, room_type_id",
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}

/// Every month (`yyyy-mm`) that `[from, to)` touches, in order. Cache keys for date ranges are built from it.
pub fn months(from: Date, to: Date) -> Vec<String> {
    let mut months = Vec::new();
    let mut month = from.replace_day(1).expect("every month has a first day");
    while from < to && month < to {
        months.push(format!("{:04}-{:02}", month.year(), u8::from(month.month())));
        month += Duration::days(i64::from(month.month().length(month.year())));
    }
    months
}

/// Cache keys `inventory:<property>:<yyyy-mm>` for every month that `[from, to)` touches.
pub fn month_keys(property: Uuid, from: Date, to: Date) -> Vec<String> {
    months(from, to).into_iter().map(|month| format!("inventory:{property}:{month}")).collect()
}

/// Month keys for the days of `[from, to)` inside the counter window. Only those days have counters to
/// change, and clamping keeps every event within the NOTIFY payload limit (under 8000 bytes).
pub(crate) fn clamped_month_keys(property: Uuid, business_date: Date, from: Date, to: Date) -> Vec<String> {
    month_keys(property, from.max(business_date), to.min(business_date + Duration::days(WINDOW_DAYS)))
}

/// Month keys for the whole counter window, for changes that touch every day from the business date on.
pub(crate) fn window_keys(property: Uuid, business_date: Date) -> Vec<String> {
    month_keys(property, business_date, business_date + Duration::days(WINDOW_DAYS))
}

#[cfg(test)]
mod tests {
    use super::{clamped_month_keys, month_keys, window_keys};
    use time::macros::date;
    use uuid::Uuid;

    #[test]
    fn month_keys_cover_each_touched_month_once() {
        let p = Uuid::nil();

        assert_eq!(
            month_keys(p, date!(2026 - 01 - 30), date!(2026 - 03 - 01)),
            vec![format!("inventory:{p}:2026-01"), format!("inventory:{p}:2026-02")]
        );
        assert_eq!(month_keys(p, date!(2026 - 12 - 31), date!(2027 - 01 - 02)).len(), 2);
        assert_eq!(month_keys(p, date!(2026 - 05 - 10), date!(2026 - 05 - 10)), Vec::<String>::new());
    }

    #[test]
    fn keys_for_a_change_stay_within_the_counter_window() {
        let p = Uuid::nil();
        let business_date = date!(2026 - 05 - 10);

        let keys = clamped_month_keys(p, business_date, date!(2020 - 01 - 01), date!(2040 - 01 - 01));

        assert_eq!(keys, window_keys(p, business_date));
        assert_eq!(keys.len(), 25, "730 days from 10 May 2026 touch May 2026 to May 2028");
        assert_eq!(keys[0], format!("inventory:{p}:2026-05"));
        assert_eq!(keys[24], format!("inventory:{p}:2028-05"));
    }
}
