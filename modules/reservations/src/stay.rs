//! Checking a room in, undoing a same-day check-in, and checking it out -- releasing an early departure's
//! nights back onto the counters they were sold from.

use crate::{ReservationsError, audit, business_date, decode_error, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use serde::Serialize;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

/// The room-condition gate for check-in. `docs/design/api-conventions.md`'s Phase 3b design reserves this for
/// Phase 5, which adds a real room status (`clean`, `dirty`, `inspected`): once it exists,
/// `require_clean_room` will refuse checking a guest into a room that isn't `clean` or `inspected`. Until then
/// this is a no-op regardless of the flag -- the parameter exists now so the REST layer's config switch (e.g.
/// `CHECKIN_REQUIRES_CLEAN_ROOM`) needs no further signature change here when Phase 5 lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckInPolicy {
    pub require_clean_room: bool,
}

/// A room after [`check_in`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CheckedIn {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub checked_in_at: OffsetDateTime,
    pub checked_in_business_date: Date,
}

/// A room after [`undo_check_in`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct UndoneCheckIn {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
}

/// A room after [`check_out`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CheckedOut {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub checked_out_at: OffsetDateTime,
    /// Nights an early departure released back onto the counters, oldest first; empty on a late check-out.
    pub released_nights: Vec<Date>,
}

/// Checks room `id` of the property in, at `expected_version` (`VersionMismatch`).
///
/// The room must be `confirmed`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). A confirmed room may still not check in today: its check-in date must be the business date,
/// or this is `Conflict "check-in is only on the arrival date (<date>)"`. It must have an assigned room
/// (`Conflict "assign a room first"` otherwise), and that room must be active and not blocked today (each a
/// `Conflict`, worded as [`crate::assign_room`]'s own checks are). `policy` gates the still-unimplemented
/// room-condition check; see [`CheckInPolicy`].
///
/// Lock order: this row, then the assigned room -- see "Room assignment lock order" in
/// `docs/design/api-conventions.md`. No inventory lock: check-in changes no counter, the stay was already sold
/// at booking.
///
/// Sets `checked_in_at` to now and `checked_in_business_date` to the business date, bumps both versions,
/// audits `reservation_room.checked_in` and notifies the reservation list and this reservation's detail.
pub async fn check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    policy: CheckInPolicy,
) -> Result<CheckedIn, ReservationsError> {
    let row: Option<CheckInRow> = sqlx::query_as(
        "select reservation_id, room_id, status, lower(stay) as check_in, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if row.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&row.status).ok_or_else(|| decode_error("status", &row.status))?;
    domain::transition(current, Action::CheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    if !is_arrival_day(row.check_in, today) {
        return Err(ReservationsError::Conflict(format!("check-in is only on the arrival date ({})", row.check_in)));
    }
    let room = row.room_id.ok_or_else(|| ReservationsError::Conflict("assign a room first".into()))?;

    // Lock order: this row (already locked above), then the room, exactly as `assign_room` and `modify_room` do.
    let target: Option<(String, bool)> =
        sqlx::query_as("select number, active from room where id = $1 and property_id = $2 for update")
            .bind(room)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let (number, active) = target.ok_or(ReservationsError::NotFound("room"))?;
    if !active {
        return Err(ReservationsError::Conflict(format!("room {number} is inactive")));
    }
    let blocked: Option<(Date, Date)> = sqlx::query_as(
        "select lower(period), upper(period) from room_block
         where room_id = $1 and released_at is null and period @> $2
         order by lower(period)
         limit 1",
    )
    .bind(room)
    .bind(today)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((from, to)) = blocked {
        return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
    }

    // Room condition (clean/inspected) arrives in Phase 5; until then `require_clean_room` has nothing to
    // check against, so checking in never refuses on it.
    let CheckInPolicy { require_clean_room: _ } = policy;

    let (version, checked_in_at): (i32, OffsetDateTime) = sqlx::query_as(
        "update reservation_room
         set status = 'checked_in', checked_in_at = now(), checked_in_business_date = $2, version = version + 1
         where id = $1
         returning version, checked_in_at",
    )
    .bind(id)
    .bind(today)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(row.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({
        "reservation_id": row.reservation_id,
        "room_id": room,
        "checked_in_business_date": today,
    });
    audit(tx, tenant, actor, "reservation_room.checked_in", "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(row.reservation_id)]).await?;

    Ok(CheckedIn {
        id,
        reservation_id: row.reservation_id,
        status: RoomStatus::CheckedIn,
        version,
        checked_in_at,
        checked_in_business_date: today,
    })
}

/// Undoes the check-in of room `id` of the property, at `expected_version` (`VersionMismatch`).
///
/// The room must be `checked_in`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). It must also still be the day it was checked in: once the business date has moved on, this is
/// `Conflict "check-in can only be undone on the day it happened"`. Reverts to `confirmed` and clears
/// `checked_in_at` and `checked_in_business_date`; bumps both versions, audits
/// `reservation_room.check_in_undone` and notifies as [`check_in`] does.
pub async fn undo_check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<UndoneCheckIn, ReservationsError> {
    let row: Option<(Uuid, String, Option<Date>, i32)> = sqlx::query_as(
        "select reservation_id, status, checked_in_business_date, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (reservation_id, status, checked_in_business_date, version) =
        row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&status).ok_or_else(|| decode_error("status", &status))?;
    domain::transition(current, Action::UndoCheckIn).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    if !same_business_day(checked_in_business_date, today) {
        return Err(ReservationsError::Conflict("check-in can only be undone on the day it happened".into()));
    }

    let new_version: i32 = sqlx::query_scalar(
        "update reservation_room
         set status = 'confirmed', checked_in_at = null, checked_in_business_date = null, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({ "reservation_id": reservation_id });
    audit(tx, tenant, actor, "reservation_room.check_in_undone", "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(reservation_id)]).await?;

    Ok(UndoneCheckIn { id, reservation_id, status: RoomStatus::Confirmed, version: new_version })
}

/// Checks room `id` of the property out, at `expected_version` (`VersionMismatch`).
///
/// The room must be `checked_in`: [`domain::transition`] refuses every other status with its own message
/// (`Conflict`). A late check-out (the business date is on or after the booked check-out) leaves the stay
/// exactly as it is. An early departure (the business date is before the booked check-out) shortens it to
/// `[check_in, max(business date, check_in + 1))`, always keeping at least the arrival night:
/// [`rooms::lock_days`] locks the released range first, `sold` is released there (every released night is on or
/// after the business date, since a checked-in room's check-in cannot be later), and the released nights'
/// `reservation_night` rows are deleted. Shrinking `stay` is also what lets `rooms::assigned_stay` stop seeing
/// this room as held once its last night is over, so it can be blocked or deactivated (the Phase 3a carry-over
/// this resolves).
///
/// Bumps both versions, audits `reservation_room.checked_out` (recording the released nights, if any) and
/// notifies the reservation list, this reservation's detail, and the released range's inventory months.
pub async fn check_out(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<CheckedOut, ReservationsError> {
    let row: Option<CheckOutRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, lower(stay) as check_in, upper(stay) as check_out, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let stay = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if stay.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&stay.status).ok_or_else(|| decode_error("status", &stay.status))?;
    domain::transition(current, Action::CheckOut).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;

    let today = business_date(tx, property).await?;
    let mut new_check_out = stay.check_out;
    let mut released_nights = Vec::new();
    if today < stay.check_out {
        new_check_out = today.max(stay.check_in + Duration::days(1));
        if new_check_out < stay.check_out {
            rooms::lock_days(tx, property, &[stay.room_type_id], new_check_out, stay.check_out).await?;
            sqlx::query(
                "update inventory_day set sold = sold - 1
                 where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
            )
            .bind(property)
            .bind(stay.room_type_id)
            .bind(new_check_out)
            .bind(stay.check_out)
            .execute(&mut **tx)
            .await?;
            released_nights = dates_in(new_check_out, stay.check_out);
            sqlx::query("delete from reservation_night where reservation_room_id = $1 and date >= $2")
                .bind(id)
                .bind(new_check_out)
                .execute(&mut **tx)
                .await?;
        }
    }

    let (version, checked_out_at): (i32, OffsetDateTime) = sqlx::query_as(
        "update reservation_room
         set stay = daterange($2, $3), status = 'checked_out', checked_out_at = now(), version = version + 1
         where id = $1
         returning version, checked_out_at",
    )
    .bind(id)
    .bind(stay.check_in)
    .bind(new_check_out)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({ "reservation_id": stay.reservation_id, "released_nights": released_nights });
    audit(tx, tenant, actor, "reservation_room.checked_out", "reservation_room", id, data).await?;
    let mut keys = vec![reservations_key(property), reservation_key(stay.reservation_id)];
    if !released_nights.is_empty() {
        keys.extend(rooms::month_keys(property, new_check_out, stay.check_out));
    }
    notify(tx, tenant, property, keys).await?;

    Ok(CheckedOut {
        id,
        reservation_id: stay.reservation_id,
        status: RoomStatus::CheckedOut,
        version,
        checked_out_at,
        released_nights,
    })
}

/// Whether `check_in` is the property's business date -- the day a room may check in. The one rule
/// [`check_in`] adds on top of [`domain::transition`]; also used by [`can_check_in`] so the reservation
/// detail's `canCheckIn` flag never drifts from what the command itself enforces.
fn is_arrival_day(check_in: Date, business_date: Date) -> bool {
    check_in == business_date
}

/// Whether `checked_in_business_date` is still the property's business date -- the one rule [`undo_check_in`]
/// adds on top of [`domain::transition`]; also used by [`can_undo_check_in`] for the same reason
/// [`is_arrival_day`] is.
fn same_business_day(checked_in_business_date: Option<Date>, business_date: Date) -> bool {
    checked_in_business_date == Some(business_date)
}

/// Whether a room in `status`, arriving `check_in`, with a room already assigned (`room_assigned`) could be
/// checked in on `business_date` -- the same rule [`check_in`] itself checks. The assigned room's own
/// active/blocked state is left out: an assigned stay's room cannot become inactive or blocked (the room and
/// block commands refuse while a stay holds it, and `assign_room` refuses such a room, each under the room's
/// lock), so [`check_in`]'s re-check under that lock is a backstop, not a rule this flag could disagree with.
/// Permissions are not part of it either: the caller combines it with `FrontDeskCheckIn`. This is the one place
/// the rule lives; the reservation detail's `canCheckIn` field calls this rather than re-deriving it.
pub(crate) fn can_check_in(status: RoomStatus, check_in: Date, business_date: Date, room_assigned: bool) -> bool {
    domain::transition(status, Action::CheckIn).is_ok() && is_arrival_day(check_in, business_date) && room_assigned
}

/// Whether a room in `status`, checked in on `checked_in_business_date`, could have that check-in undone on
/// `business_date` -- the same rule [`undo_check_in`] itself checks. Used by the reservation detail's
/// `canUndoCheckIn` field.
pub(crate) fn can_undo_check_in(
    status: RoomStatus,
    checked_in_business_date: Option<Date>,
    business_date: Date,
) -> bool {
    domain::transition(status, Action::UndoCheckIn).is_ok()
        && same_business_day(checked_in_business_date, business_date)
}

/// Whether a room in `status` could be checked out -- the same rule [`check_out`] itself checks (a checked-in
/// room may always be checked out, early or late). Used by the reservation detail's `canCheckOut` field.
pub(crate) fn can_check_out(status: RoomStatus) -> bool {
    domain::transition(status, Action::CheckOut).is_ok()
}

/// The dates of `[from, to)`, in order.
fn dates_in(from: Date, to: Date) -> Vec<Date> {
    let mut days = Vec::new();
    let mut day = from;
    while day < to {
        days.push(day);
        day = day.next_day().expect("stays end inside the counter window");
    }
    days
}

/// The parts of a `reservation_room` that checking in reads.
#[derive(sqlx::FromRow)]
struct CheckInRow {
    reservation_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    version: i32,
}

/// The parts of a `reservation_room` that checking out reads.
#[derive(sqlx::FromRow)]
struct CheckOutRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    check_in: Date,
    check_out: Date,
    version: i32,
}
