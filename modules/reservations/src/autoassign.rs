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
//! own sort order, then its number the way the rail compares it, digit runs by value (`99` before `101`; a
//! number that is only digits comes before one that is not, shorter before longer, then as text).
//!
//! **Locking.** [`pick_room`] then walks the ranked candidates in order and locks each in turn with `select
//! ... for update skip locked`: a room another command already holds (a block, a retype, another assignment)
//! is skipped rather than waited for, so two bookings racing for rooms of one type never deadlock and never
//! wait on each other; each ends up on a different room, or unassigned. Once a room is locked (still active and of the type), both `not
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
/// by rail order (natural number order). Takes no locks; [`pick_room`] locks each candidate itself, in this order, one at a time.
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
order by greatest(gap_before, 0) + greatest(gap_after, 0), type_order, room_order,
         (number !~ '^[0-9]+$'), case when number ~ '^[0-9]+$' then length(number) end, number";

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
    lock_first_free(tx, candidates, room_type, check_in, check_out).await
}

/// Walks `candidates` (ranked best first) and returns the first that can be locked and is still a free, active
/// room of `room_type` for `[check_in, check_out)`. Split from [`pick_room`] so a test can hand it a ranking that
/// went stale, which the window between the ranking statement and the lock otherwise hides.
async fn lock_first_free(
    tx: &mut Tx,
    candidates: Vec<(Uuid, String)>,
    room_type: Uuid,
    check_in: Date,
    check_out: Date,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    for (id, number) in candidates {
        // `active` and the type are re-checked here too: a room retyped or deactivated between the ranking and
        // this lock (by `update_room`, which holds the room lock) is skipped like a locked one.
        let locked: Option<i32> = sqlx::query_scalar(
            "select 1 from room where id = $1 and active and room_type_id = $2 for update skip locked",
        )
        .bind(id)
        .bind(room_type)
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

#[cfg(test)]
mod tests {
    use super::*;
    use db::testing::app_pool;
    use db::{Scope, TenantId, UserId, begin};
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    /// A property with two DLX rooms (101, 102) and the ids a test needs, all committed.
    struct Fixture {
        pool: sqlx::PgPool,
        tenant: TenantId,
        user: UserId,
        property: Uuid,
        room_type: Uuid,
        /// A second room type (STD), to retype a room to.
        other_type: Uuid,
        rooms: Vec<rooms::Room>,
        from: Date,
        to: Date,
    }

    async fn fixture(opts: PgConnectOptions) -> Fixture {
        let pool = app_pool(opts, 2).await;
        let (tenant, user) = (TenantId(Uuid::now_v7()), UserId(Uuid::now_v7()));
        let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
        sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
        sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&mut *tx)
            .await
            .unwrap();
        let hotel = property::NewProperty {
            code: "GAL".into(),
            name: "Galle".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        let deluxe = rooms::NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        let room_type = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe.clone()).await.unwrap();
        let standard = rooms::NewRoomType {
            code: "STD".into(),
            name: "Standard".into(),
            base_occupancy: 1,
            max_children: 0,
            max_occupancy: 2,
            ..deluxe.clone()
        };
        let other_type = rooms::create_room_type(&mut tx, tenant, user, property.id, standard).await.unwrap().id;
        let mut made = Vec::new();
        for number in ["101", "102"] {
            let room =
                rooms::NewRoom { room_type_id: room_type.id, number: number.into(), floor: None, section_id: None };
            made.push(rooms::create_room(&mut tx, tenant, user, property.id, room).await.unwrap());
        }
        tx.commit().await.unwrap();
        let from = property.business_date + time::Duration::days(1);
        Fixture {
            pool,
            tenant,
            user,
            property: property.id,
            room_type: room_type.id,
            other_type,
            rooms: made,
            from,
            to: from + time::Duration::days(2),
        }
    }

    /// Ranks the candidates, applies `changes` to room 101 in a committed transaction (the ranking is now stale),
    /// then locks from that ranking: 101 must be skipped, 102 returned.
    async fn stale_ranking_skips_101(f: Fixture, changes: rooms::RoomChanges) {
        let mut tx = begin(&f.pool, Scope::tenant(f.tenant)).await.unwrap();
        let ranked: Vec<(Uuid, String)> = sqlx::query_as(CANDIDATE_SQL)
            .bind(f.property)
            .bind(f.room_type)
            .bind(f.from)
            .bind(f.to)
            .bind(FIT_HORIZON_DAYS)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(ranked.iter().map(|(_, number)| number.as_str()).collect::<Vec<_>>(), ["101", "102"]);

        let mut other = begin(&f.pool, Scope::tenant(f.tenant)).await.unwrap();
        let r101 = &f.rooms[0];
        rooms::update_room(&mut other, f.tenant, f.user, f.property, r101.id, r101.version, changes).await.unwrap();
        other.commit().await.unwrap();

        let picked = lock_first_free(&mut tx, ranked, f.room_type, f.from, f.to).await.unwrap();

        assert_eq!(picked.map(|(_, number)| number), Some("102".to_owned()));
    }

    #[sqlx::test(migrator = "db::MIGRATOR")]
    async fn a_room_deactivated_after_the_ranking_is_never_returned(_: PgPoolOptions, opts: PgConnectOptions) {
        let off = rooms::RoomChanges { active: Some(false), ..Default::default() };
        stale_ranking_skips_101(fixture(opts).await, off).await;
    }

    #[sqlx::test(migrator = "db::MIGRATOR")]
    async fn a_room_retyped_after_the_ranking_is_never_returned(_: PgPoolOptions, opts: PgConnectOptions) {
        let f = fixture(opts).await;
        let retype = rooms::RoomChanges { room_type_id: Some(f.other_type), ..Default::default() };
        stale_ranking_skips_101(f, retype).await;
    }
}
