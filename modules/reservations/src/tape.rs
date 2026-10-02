//! The tape chart's two reads: the stays and blocks of up to ten rooms over a window, and the stays that still
//! need a room. Both are range scans on indexes made for them, so the chart's cost follows what it shows.

use crate::ReservationsError;
use db::Tx;
use domain::RoomStatus;
use sqlx::Row;
use sqlx::postgres::PgRow;
use std::collections::HashSet;
use std::sync::LazyLock;
use time::{Date, Duration};
use uuid::Uuid;

/// Most rooms one `tape_window` shows.
pub const MAX_TAPE_ROOMS: usize = 10;
/// Longest window, in days, either read serves.
pub const MAX_TAPE_DAYS: i64 = 42;
/// Stays and blocks of at most this many nights are found by arrival or start, longer ones through their own
/// partial index (`reservation_room_long_stay_idx`, `room_block_long_idx`; see
/// `migrations/0011_tape_stays_by_arrival.sql` and `0012_tape_blocks_by_start.sql`). Changing it needs a new
/// migration for those indexes.
const SHORT_STAY_NIGHTS: i32 = 31;

/// Everything `tape_window` reads, in one statement. Rows are tagged by `kind` and share one column set, with
/// nulls where a column doesn't apply:
/// - `check`: one row, with `known` the number of `$1` rooms that belong to property `$5`;
/// - `stay`: the stays of those rooms overlapping `[$2, $3)` (`reason` null);
/// - `block`: their unreleased blocks overlapping it, with `reason` the block reason's label.
///
/// `&&` is not leakproof, so under row-level security it can't be an index condition: each stay and block
/// branch bounds its scan with leakproof comparisons (arrival or start, and length: `$4` is the earliest arrival or
/// start of a short one that can still overlap) and `&&` only trims the result.
const TAPE_WINDOW_TEMPLATE: &str = "
    select * from (
      select 'check'::text as kind, null::uuid as id, null::uuid as reservation_id, null::uuid as room_id,
             null::uuid as room_type_id, null::date as start, null::date as \"end\", null::text as status,
             null::text as guest_name, null::text as account_name, null::int as version, null::text as reason,
             (select count(*) from room where id = any($1) and property_id = $5) as known
      union all
      select 'stay', a.id, a.reservation_id, a.room_id, a.room_type_id, lower(a.stay), upper(a.stay), a.status,
             (select g.last_name || case when g.first_name = '' then '' else ', ' || left(g.first_name, 1) || '.' end
              from guest g where g.id = a.primary_guest_id),
             (select c.name from reservation r join account c on c.id = r.account_id where r.id = a.reservation_id),
             a.version, null, null
      from reservation_room a
      where a.room_id = any($1) and a.status not in ('cancelled', 'no_show') and a.stay && daterange($2, $3)
        and ((a.nights <= {short} and a.arrival >= $4 and a.arrival < $3) or a.nights > {short})
      union all
      select 'block', b.id, null, b.room_id, null, lower(b.period), upper(b.period), null, null, null, null,
             (select br.label from block_reason br where br.property_id = b.property_id and br.id = b.reason_id),
             null
      from room_block b
      where b.room_id = any($1) and b.released_at is null and b.period && daterange($2, $3)
        and ((b.days <= {short} and b.starts >= $4 and b.starts < $3) or b.days > {short})
    ) rows
    order by room_id, start";

/// [`TAPE_WINDOW_TEMPLATE`] with [`SHORT_STAY_NIGHTS`] written into the text. The length limit must be a literal,
/// not a bind: a partial index (`where nights > 31`) is only used when the planner can prove the query's condition
/// implies its predicate, which `nights > $4` doesn't in a generic plan. The number must match the predicates of
/// `reservation_room_long_stay_idx` (0011) and `room_block_long_idx` (0012).
static TAPE_WINDOW_SQL: LazyLock<String> =
    LazyLock::new(|| TAPE_WINDOW_TEMPLATE.replace("{short}", &SHORT_STAY_NIGHTS.to_string()));

/// A stay in one of the window's rooms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeStay {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_id: Uuid,
    pub room_type_id: Uuid,
    pub start: Date,
    pub end: Date,
    pub status: RoomStatus,
    /// The primary guest as "Silva, A.": last name and first initial, the last name alone for a single name.
    pub guest_name: String,
    pub account_name: Option<String>,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TapeBlock {
    pub id: Uuid,
    pub room_id: Uuid,
    pub start: Date,
    pub end: Date,
    /// The block reason's label.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeWindow {
    pub stays: Vec<TapeStay>,
    pub blocks: Vec<TapeBlock>,
}

db::text_enum!(
    /// Why a stay has no room: `overbooked` when a night is sold beyond the physical rooms of its type,
    /// `no_single_room` when there are rooms enough but no one room is free for every night.
    NeedsRoomReason { Overbooked = "overbooked", NoSingleRoom = "no_single_room" }
);

/// A confirmed stay with no room yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnassignedStay {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_type_id: Uuid,
    pub start: Date,
    pub end: Date,
    pub guest_name: String,
    pub reason: NeedsRoomReason,
    pub version: i32,
}

/// `[from, to)` must span 1 to [`MAX_TAPE_DAYS`] days.
fn check_window(from: Date, to: Date) -> Result<(), ReservationsError> {
    if to <= from || to - from > Duration::days(MAX_TAPE_DAYS) {
        return Err(ReservationsError::Invalid(format!("the window must be 1 to {MAX_TAPE_DAYS} days")));
    }
    Ok(())
}

fn stay_from_row(row: &PgRow) -> Result<TapeStay, sqlx::Error> {
    let status: String = row.try_get("status")?;
    Ok(TapeStay {
        id: row.try_get("id")?,
        reservation_id: row.try_get("reservation_id")?,
        room_id: row.try_get("room_id")?,
        room_type_id: row.try_get("room_type_id")?,
        start: row.try_get("start")?,
        end: row.try_get("end")?,
        status: RoomStatus::parse(&status).ok_or_else(|| crate::decode_error("status", &status))?,
        guest_name: row.try_get("guest_name")?,
        account_name: row.try_get("account_name")?,
        version: row.try_get("version")?,
    })
}

/// The stays (not cancelled or no-show) and the unreleased blocks of `rooms` that overlap `[from, to)`, by
/// room then start. The window is at most [`MAX_TAPE_DAYS`] days and `rooms` holds 1 to [`MAX_TAPE_ROOMS`]
/// rooms of the property, active or not; anything else is `Invalid`.
pub async fn tape_window(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[Uuid],
    from: Date,
    to: Date,
) -> Result<TapeWindow, ReservationsError> {
    check_window(from, to)?;
    if rooms.is_empty() || rooms.len() > MAX_TAPE_ROOMS {
        return Err(ReservationsError::Invalid(format!("name 1 to {MAX_TAPE_ROOMS} rooms")));
    }
    let distinct = rooms.iter().collect::<HashSet<_>>().len();
    let rows = sqlx::query(sqlx::AssertSqlSafe(TAPE_WINDOW_SQL.as_str()))
        .bind(rooms)
        .bind(from)
        .bind(to)
        .bind(from - Duration::days(i64::from(SHORT_STAY_NIGHTS)))
        .bind(property)
        .fetch_all(&mut **tx)
        .await?;
    let mut known = 0;
    let mut stays = Vec::new();
    let mut blocks = Vec::new();
    for row in &rows {
        match row.try_get::<String, _>("kind")?.as_str() {
            "check" => known = row.try_get("known")?,
            "stay" => stays.push(stay_from_row(row)?),
            _ => blocks.push(TapeBlock {
                id: row.try_get("id")?,
                room_id: row.try_get("room_id")?,
                start: row.try_get("start")?,
                end: row.try_get("end")?,
                reason: row.try_get("reason")?,
            }),
        }
    }
    if known != distinct as i64 {
        return Err(ReservationsError::Invalid("every room must belong to the property".into()));
    }
    Ok(TapeWindow { stays, blocks })
}

/// The confirmed stays of the property with no room that overlap `[from, to)` (at most [`MAX_TAPE_DAYS`]
/// days), by arrival. A stay is `overbooked` when any of its nights is sold beyond the type's physical rooms.
/// Nights before the business date have no counter, so a stay under way is never `overbooked` on their account.
pub async fn unassigned_stays(
    tx: &mut Tx,
    property: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<UnassignedStay>, ReservationsError> {
    check_window(from, to)?;
    let rows = sqlx::query(
        "select rr.id, rr.reservation_id, rr.room_type_id, lower(rr.stay) as start, upper(rr.stay) as \"end\",
                g.last_name || case when g.first_name = '' then '' else ', ' || left(g.first_name, 1) || '.' end
                  as guest_name,
                exists (
                  select 1 from inventory_day i
                  where i.property_id = $1 and i.room_type_id = rr.room_type_id
                    and i.date >= lower(rr.stay) and i.date < upper(rr.stay)
                    and i.physical - i.sold - i.out_of_order < 0) as overbooked,
                rr.version
         from reservation_room rr
         join guest g on g.id = rr.primary_guest_id
         where rr.property_id = $1 and rr.room_id is null and rr.status = 'confirmed'
           and rr.stay && daterange($2, $3)
         order by lower(rr.stay), rr.id",
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let stays = rows
        .iter()
        .map(|row| {
            let overbooked: bool = row.try_get("overbooked")?;
            Ok(UnassignedStay {
                id: row.try_get("id")?,
                reservation_id: row.try_get("reservation_id")?,
                room_type_id: row.try_get("room_type_id")?,
                start: row.try_get("start")?,
                end: row.try_get("end")?,
                guest_name: row.try_get("guest_name")?,
                reason: if overbooked { NeedsRoomReason::Overbooked } else { NeedsRoomReason::NoSingleRoom },
                version: row.try_get("version")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(stays)
}
