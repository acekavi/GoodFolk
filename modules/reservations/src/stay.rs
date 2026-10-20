//! Checking a room in, undoing a same-day check-in, and checking it out -- releasing an early departure's
//! nights back onto the counters they were sold from.

use crate::{
    ReservationsError, audit, bump_reservation, business_date, decode_error, notify, reservation_key, reservations_key,
};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use housekeeping::{Condition, hk_key};
use serde::Serialize;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

/// A manager's reason for checking a guest into a room that is not `clean` or `inspected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckInOverride {
    /// Not empty after trimming, at most [`MAX_OVERRIDE_REASON`] characters.
    pub reason: String,
}

/// The longest override reason.
pub const MAX_OVERRIDE_REASON: usize = 500;

/// The reason on the reversal of a night an early check-out releases.
const NIGHT_RELEASED: &str = "Night released at early check-out";

/// The reason on the reversal of the night posted for a stay whose check-in is undone.
const CHECK_IN_UNDONE: &str = "Check-in undone";

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
/// `Conflict`, worded as [`crate::assign_room`]'s own checks are). The room's condition must be `clean` or
/// `inspected`, or this is `Conflict "room 101 is Dirty"`, unless `override_` names a reason (`Invalid` when it
/// is empty after trimming or longer than [`MAX_OVERRIDE_REASON`] characters); the caller decides who may
/// override.
///
/// Lock order: this row, then the assigned room, then its condition (shared) -- see "Room assignment lock
/// order" in `docs/design/api-conventions.md` -- then the reservation's deposit folio and this stay's guest folio.
/// No inventory lock: check-in changes no counter, the stay was already sold at booking.
///
/// The reservation's first check-in moves its deposit folio's balance to this stay's guest folio
/// ([`folio::move_deposit`]); that needs a rate for each currency moved on the business date (`Conflict`).
///
/// Sets `checked_in_at` to now and `checked_in_business_date` to the business date, remembers the room's
/// condition in `condition_before_check_in` for [`undo_check_in`], bumps both versions, audits
/// `reservation_room.checked_in` (with the override reason and condition when overridden) and notifies the reservation list, this reservation's detail, and the
/// stay's tape months.
pub async fn check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    override_: Option<CheckInOverride>,
) -> Result<CheckedIn, ReservationsError> {
    let override_reason = override_.map(|given| validated_reason(&given.reason)).transpose()?;
    let row: Option<CheckInRow> = sqlx::query_as(
        "select reservation_id, room_id, status, lower(stay) as check_in, upper(stay) as check_out, version
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

    // After the room lock, before anything is written: shared, so a housekeeper's change waits for this commit.
    let condition: String = sqlx::query_scalar("select condition from room_condition where room_id = $1 for share")
        .bind(room)
        .fetch_one(&mut **tx)
        .await?;
    let condition = Condition::parse(&condition).ok_or_else(|| decode_error("condition", &condition))?;
    let ready = matches!(condition, Condition::Clean | Condition::Inspected);
    if !ready && override_reason.is_none() {
        return Err(ReservationsError::Conflict(format!("room {number} is {}", title_case(condition.as_str()))));
    }

    let (version, checked_in_at): (i32, OffsetDateTime) = sqlx::query_as(
        "update reservation_room
         set status = 'checked_in', checked_in_at = now(), checked_in_business_date = $2,
             condition_before_check_in = $3, version = version + 1
         where id = $1
         returning version, checked_in_at",
    )
    .bind(id)
    .bind(today)
    .bind(condition.as_str())
    .fetch_one(&mut **tx)
    .await?;
    // After this row and the room, before the reservation: the folios sit between them in the lock order.
    folio::move_deposit(tx, tenant, Some(actor), property, row.reservation_id, id).await?;
    bump_reservation(tx, row.reservation_id).await?;

    let mut data = serde_json::json!({
        "reservation_id": row.reservation_id,
        "room_id": room,
        "checked_in_business_date": today,
    });
    if let Some(reason) = override_reason.filter(|_| !ready) {
        data["override_reason"] = reason.into();
        data["condition"] = condition.as_str().into();
    }
    audit(tx, tenant, Some(actor), "reservation_room.checked_in", "reservation_room", id, data).await?;
    let keys = [reservations_key(property), reservation_key(row.reservation_id), hk_key(property)]
        .into_iter()
        .chain(rooms::tape_keys(property, row.check_in, row.check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

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
/// `Conflict "check-in can only be undone on the day it happened"`. Reverts to `confirmed`, clears
/// `checked_in_at` and `checked_in_business_date` and puts the room back to the condition it had before the
/// check-in (`clean` for a stay checked in before that was recorded), locking its condition after this row;
/// bumps both versions, audits `reservation_room.check_in_undone` (with the restored condition) and notifies as
/// [`check_in`] does, plus the housekeeping board. The lines already posted for the check-in date's night are
/// reversed ("Check-in undone", [`folio::reverse_released_nights`]; one on an invoiced folio is a `Conflict`), then a
/// deposit the check-in moved to this stay goes back to the deposit folio ([`folio::undo_deposit_move`]). The stay's
/// folios are locked together first ([`folio::lock_stay_folios`]), after the room's condition.
pub async fn undo_check_in(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<UndoneCheckIn, ReservationsError> {
    let row: Option<UndoRow> = sqlx::query_as(
        "select reservation_id, status, checked_in_business_date, version, lower(stay) as check_in,
                upper(stay) as check_out, room_id, condition_before_check_in as before
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let UndoRow { reservation_id, status, checked_in_business_date, version, check_in, check_out, room_id, before } =
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

    let restored = match before {
        Some(stored) => Condition::parse(&stored).ok_or_else(|| decode_error("condition_before_check_in", &stored))?,
        None => Condition::Clean,
    };
    if let Some(room) = room_id {
        set_room_condition(tx, actor, property, room, restored).await?;
    }
    let new_version: i32 = sqlx::query_scalar(
        "update reservation_room
         set status = 'confirmed', checked_in_at = null, checked_in_business_date = null,
             condition_before_check_in = null, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    // After this row and the room's condition, before the reservation: the folios sit between them in the lock order.
    // The night the check-in posted (tonight's, if charges were posted) is reversed first, so the stay's credit, and
    // with it the deposit moved back, is whole again.
    let tonight = [today];
    folio::lock_stay_folios(tx, tenant, property, id, &tonight).await?;
    folio::reverse_released_nights(tx, tenant, Some(actor), property, id, &tonight, CHECK_IN_UNDONE).await?;
    folio::undo_deposit_move(tx, tenant, Some(actor), property, id).await?;
    bump_reservation(tx, reservation_id).await?;

    let data = serde_json::json!({ "reservation_id": reservation_id, "condition": restored.as_str() });
    audit(tx, tenant, Some(actor), "reservation_room.check_in_undone", "reservation_room", id, data).await?;
    let keys = [reservations_key(property), reservation_key(reservation_id), hk_key(property)]
        .into_iter()
        .chain(rooms::tape_keys(property, check_in, check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

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
/// Then every night the stay keeps before `min(check-out, business date + 1)` whose charges are not yet posted is
/// posted ([`folio::post_unposted_nights`]), so no night is missed, having first locked every folio it and the
/// reversals below touch, together and in id order, after the inventory days ([`folio::lock_stay_folios`]); that needs
/// a rate for the stay's currency on each such night (`Conflict`). The lines already posted for the released nights are
/// reversed ([`folio::reverse_released_nights`]); one on an invoiced folio is a `Conflict`.
///
/// Sets the room's condition to `dirty` (locking `room_condition` after this row, bumping its version), bumps
/// both versions, audits `reservation_room.checked_out` (recording the released nights, if any, and the
/// condition the room had) and notifies the reservation list, this reservation's detail, the housekeeping
/// board, the released range's inventory months, and the original (unshortened) range's tape months.
pub async fn check_out(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<CheckedOut, ReservationsError> {
    let row: Option<CheckOutRow> = sqlx::query_as(
        "select reservation_id, room_type_id, room_id, status, lower(stay) as check_in, upper(stay) as check_out,
                version
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

    // Before the inventory days: room_condition comes ahead of room_block and inventory_day in the lock order.
    let was = match stay.room_id {
        Some(room) => Some(set_room_condition(tx, actor, property, room, Condition::Dirty).await?),
        None => None,
    };
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
    // Every night stayed and not yet posted, the business date's own included (the night audit has not posted it
    // yet): after the released nights are gone, before the stay is marked checked out, and after the inventory days
    // in the lock order.
    // Every folio either touches is locked first, at once, in id order.
    let before = new_check_out.min(today + Duration::days(1));
    folio::lock_stay_folios(tx, tenant, property, id, &released_nights).await?;
    folio::post_unposted_nights(tx, tenant, Some(actor), id, before).await?;
    if !released_nights.is_empty() {
        folio::reverse_released_nights(tx, tenant, Some(actor), property, id, &released_nights, NIGHT_RELEASED).await?;
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
    bump_reservation(tx, stay.reservation_id).await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "released_nights": released_nights,
        "condition_before": was.map(Condition::as_str),
    });
    audit(tx, tenant, Some(actor), "reservation_room.checked_out", "reservation_room", id, data).await?;
    let mut keys = vec![reservations_key(property), reservation_key(stay.reservation_id), hk_key(property)];
    if !released_nights.is_empty() {
        keys.extend(rooms::month_keys(property, new_check_out, stay.check_out));
    }
    // The original range, not the shortened one: an early departure still touches every month the booked
    // stay used to cover.
    keys.extend(rooms::tape_keys(property, stay.check_in, stay.check_out));
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

/// Locks `room`'s condition and sets it to `to`, returning the condition it had. The caller audits it inside
/// its own entry and notifies the board.
async fn set_room_condition(
    tx: &mut Tx,
    actor: UserId,
    property: Uuid,
    room: Uuid,
    to: Condition,
) -> Result<Condition, ReservationsError> {
    housekeeping::lock_and_set(tx, actor, property, room, to).await.map_err(|err| match err {
        housekeeping::HousekeepingError::NotFound(what) => ReservationsError::NotFound(what),
        housekeeping::HousekeepingError::Database(err) => ReservationsError::Database(err),
        // Locking a condition only fails when the row is missing or the database errs; anything else is a bug.
        other => ReservationsError::Database(sqlx::Error::Protocol(format!("locking a condition failed: {other}"))),
    })
}

/// `reason` trimmed, or `Invalid` when it is empty or longer than [`MAX_OVERRIDE_REASON`] characters.
fn validated_reason(reason: &str) -> Result<String, ReservationsError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_OVERRIDE_REASON {
        return Err(ReservationsError::Invalid(format!(
            "the override reason must be between 1 and {MAX_OVERRIDE_REASON} characters"
        )));
    }
    Ok(reason.to_owned())
}

/// `dirty` as `Dirty`.
fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
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
    check_out: Date,
    version: i32,
}

/// The parts of a `reservation_room` that checking out reads.
#[derive(sqlx::FromRow)]
struct CheckOutRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    check_out: Date,
    version: i32,
}

/// The parts of a `reservation_room` that undoing a check-in reads.
#[derive(sqlx::FromRow)]
struct UndoRow {
    reservation_id: Uuid,
    status: String,
    checked_in_business_date: Option<Date>,
    version: i32,
    check_in: Date,
    check_out: Date,
    room_id: Option<Uuid>,
    before: Option<String>,
}
