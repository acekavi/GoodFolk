//! `inventory_day` counters: one row per room type per day, from the business date for [`WINDOW_DAYS`].
//!
//! `physical` counts a type's active rooms; `out_of_order` counts its active rooms under an active
//! out-of-order block that day. Writers adjust the counters in their own transaction; [`find_drift`]
//! recomputes them from rooms and blocks, for tests and the nightly check (Phase 7).

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
    /// Rooms of this type that can still be sold that day.
    pub fn available(&self) -> i32 {
        self.physical - self.sold - self.out_of_order
    }
}

/// A counter row that disagrees with a recomputation from rooms and blocks. `None` means the row is
/// missing (expected) or should not exist (actual).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InventoryDrift {
    pub room_type_id: Uuid,
    pub date: Date,
    pub expected_physical: Option<i32>,
    pub actual_physical: Option<i32>,
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
         on conflict do nothing",
    )
    .bind(property)
    .bind(i32::try_from(WINDOW_DAYS).expect("window fits in i32"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Counter rows from the business date on that differ from a recomputation from rooms and blocks.
/// `sold` is expected to be 0 until reservations exist (Phase 3).
pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrift>, sqlx::Error> {
    sqlx::query_as(
        "with expected as (
             select rt.id as room_type_id, day.date,
                    (select count(*) from room r where r.room_type_id = rt.id and r.active)::integer as physical,
                    (select count(*) from room_block b join room r on r.id = b.room_id
                     where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
                       and b.released_at is null and b.period @> day.date)::integer as out_of_order
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
                e.out_of_order as expected_out_of_order, a.out_of_order as actual_out_of_order
         from expected e full join actual a on a.room_type_id = e.room_type_id and a.date = e.date
         where e.physical is distinct from a.physical or e.out_of_order is distinct from a.out_of_order
            or a.sold is distinct from 0
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

/// Cache keys `inventory:<property>:<yyyy-mm>` for every month that `[from, to)` touches.
pub fn month_keys(property: Uuid, from: Date, to: Date) -> Vec<String> {
    let mut keys = Vec::new();
    let mut month = from.replace_day(1).expect("every month has a first day");
    while from < to && month < to {
        keys.push(format!("inventory:{property}:{:04}-{:02}", month.year(), u8::from(month.month())));
        month += Duration::days(i64::from(month.month().length(month.year())));
    }
    keys
}

/// Month keys for the whole counter window, for changes that touch every day from the business date on.
pub(crate) fn window_keys(property: Uuid, business_date: Date) -> Vec<String> {
    month_keys(property, business_date, business_date + Duration::days(WINDOW_DAYS))
}

#[cfg(test)]
mod tests {
    use super::month_keys;
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
}
