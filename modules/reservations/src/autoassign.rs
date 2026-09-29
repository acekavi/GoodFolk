//! Picking a room for a newly booked stay: the tightest-fitting active room of its type that is free for
//! every night of the stay, so the front desk does not have to assign every booking by hand.
//!
//! **Lock order.** `create_reservation` locks `inventory_day` (`rooms::lock_days`), then `property_counter`
//! (the confirmation number), then, room by room, calls [`pick_room`] here, which locks `room` rows with
//! `SKIP LOCKED`: the order is `inventory_day` -> `property_counter` -> `room`. Because the `room` locks are
//! `SKIP LOCKED`, `pick_room` never waits on a `room` row someone else holds, so nothing that holds a `room`
//! lock can be waiting on `property_counter`, and no cycle can form with the `inventory_day` lock order or the
//! "Room assignment lock order" that `assign_room` and the `rooms` block/retype commands follow (both in
//! `docs/design/api-conventions.md`; Task 13 adds this order there too). Picking never refuses a booking, so
//! it cannot break the rule that the confirmation number is taken only after every refusal check.
//!
//! **Ranking.** The candidate query takes no locks. It ranks the type's active, unblocked, unbooked-over-the-
//! stay rooms by the tightest fit: the free nights between the previous stay or block on that room and this
//! arrival, plus the free nights between this departure and the next one, each capped at [`FIT_HORIZON_DAYS`]
//! (an open end counts as the cap). Ties go to the room's rail order: room type sort order, then the room's
//! own sort order, then its number.
//!
//! **Locking.** [`pick_room`] then walks the ranked candidates in order and locks each in turn with `select
//! ... for update skip locked`: a room another command already holds (a block, a retype, another assignment)
//! is skipped rather than waited for, so two bookings racing for rooms of one type never deadlock and never
//! wait on each other; each ends up on a different room, or unassigned. Once a room is locked, both `not
//! exists` checks from the ranking query are re-run for it alone, because read committed takes a fresh
//! snapshot per statement and so now sees a stay or block committed between the ranking query and the lock;
//! if either finds a row, the candidate is skipped. A skipped candidate's lock is not released early -- it
//! stays held until the caller's transaction commits or rolls back, which is harmless: nothing in this
//! transaction still needs that room once its checks have failed. The first candidate whose checks both pass
//! is returned locked. `reservation_room_no_double_booking` remains the final guard against double booking.

use db::Tx;
use time::Date;
use uuid::Uuid;

/// How many nights either way of a stay [`pick_room`] looks for the previous or next booking or block on a
/// candidate room, when ranking by tightest fit. An open end (nothing found within the horizon) counts as
/// this many free nights, same as a booking or block exactly this far away.
pub const FIT_HORIZON_DAYS: i32 = 60;

/// The candidate query: active rooms of `$2` in property `$1` with no stay or block over `[$3, $4)`, ranked
/// by tightest fit (nights free before `$3` plus nights free after `$4`, each capped at `$5` free nights) then
/// by rail order. Takes no locks; [`pick_room`] locks each candidate itself, in this order, one at a time.
const CANDIDATE_SQL: &str = "
with candidate as (
  select r.id, r.number, rt.sort_order as type_order, r.sort_order as room_order,
         coalesce((select $3 - max(upper(x.range)) from (
                      select a.stay as range from reservation_room a
                      where a.room_id = r.id and a.status not in ('cancelled', 'no_show')
                        and a.stay && daterange($3 - $5, $3)
                      union all
                      select b.period from room_block b
                      where b.room_id = r.id and b.released_at is null and b.period && daterange($3 - $5, $3)
                   ) x), $5) as gap_before,
         coalesce((select min(lower(x.range)) - $4 from (
                      select a.stay as range from reservation_room a
                      where a.room_id = r.id and a.status not in ('cancelled', 'no_show')
                        and a.stay && daterange($4, $4 + $5)
                      union all
                      select b.period from room_block b
                      where b.room_id = r.id and b.released_at is null and b.period && daterange($4, $4 + $5)
                   ) x), $5) as gap_after
  from room r join room_type rt on rt.id = r.room_type_id
  where r.property_id = $1 and r.room_type_id = $2 and r.active
    and not exists (select 1 from reservation_room a
                    where a.room_id = r.id and a.status not in ('cancelled', 'no_show')
                      and a.stay && daterange($3, $4))
    and not exists (select 1 from room_block b
                    where b.room_id = r.id and b.released_at is null and b.period && daterange($3, $4))
)
select id, number from candidate
order by greatest(gap_before, 0) + greatest(gap_after, 0), type_order, room_order, number";

/// The tightest-fitting active room of `room_type` in `property`, free for every night of `[check_in,
/// check_out)`, locked so it cannot be taken from under the caller before it commits; `None` if no room fits.
/// Never moves a stay already in a room: the caller only ever calls this for a stay whose `room_id` is still
/// null. See the module doc for the lock order and why candidates skipped along the way stay locked.
pub(crate) async fn pick_room(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    check_in: Date,
    check_out: Date,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let candidates: Vec<(Uuid, String)> = sqlx::query_as(CANDIDATE_SQL)
        .bind(property)
        .bind(room_type)
        .bind(check_in)
        .bind(check_out)
        .bind(FIT_HORIZON_DAYS)
        .fetch_all(&mut **tx)
        .await?;
    for (id, number) in candidates {
        let locked: Option<i32> = sqlx::query_scalar("select 1 from room where id = $1 for update skip locked")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
        if locked.is_none() {
            continue;
        }
        let stay_taken: bool = sqlx::query_scalar(
            "select exists (select 1 from reservation_room a
                            where a.room_id = $1 and a.status not in ('cancelled', 'no_show')
                              and a.stay && daterange($2, $3))",
        )
        .bind(id)
        .bind(check_in)
        .bind(check_out)
        .fetch_one(&mut **tx)
        .await?;
        if stay_taken {
            continue;
        }
        let blocked: bool = sqlx::query_scalar(
            "select exists (select 1 from room_block b
                            where b.room_id = $1 and b.released_at is null and b.period && daterange($2, $3))",
        )
        .bind(id)
        .bind(check_in)
        .bind(check_out)
        .fetch_one(&mut **tx)
        .await?;
        if blocked {
            continue;
        }
        return Ok(Some((id, number)));
    }
    Ok(None)
}
