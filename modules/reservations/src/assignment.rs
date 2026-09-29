//! Putting a confirmed stay in a room of its booked type, taking it out again, and the rooms a stay could be
//! put in. Assigning never changes the inventory counters: the stay was sold when it was booked.

use crate::{ReservationsError, audit, notify, reservation_key, reservations_key, violates};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use serde::Serialize;
use sqlx::Acquire;
use time::Date;
use uuid::Uuid;

/// A stay's room after assigning or unassigning it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct AssignedRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_id: Option<Uuid>,
    pub room_number: Option<String>,
    pub version: i32,
}

/// A room a stay could be put in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct FreeRoom {
    pub id: Uuid,
    pub number: String,
    /// The housekeeping section's name.
    pub section: Option<String>,
}

/// Puts the stay `id` of the property, at `expected_version` (`VersionMismatch`) and confirmed (`Conflict`), in
/// `room`, or moves it there from the room it has. The room must be an active room of the property (`NotFound`
/// otherwise) of the stay's booked type, not blocked on any of its nights and not taken by another stay on any
/// of them (`Conflict`, naming the booking that has it).
///
/// Locks the stay's row, then the room's (`for update`): room and block commands lock the room row before they
/// read its assignments, so an assignment and a block or retype of the same room run one at a time, and so do
/// two assignments of one room. The exclusion constraint `reservation_room_no_double_booking` stays the final
/// guard against double booking.
pub async fn assign_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    room: Uuid,
) -> Result<AssignedRoom, ReservationsError> {
    let stay = lock_confirmed_stay(tx, property, id, expected_version, "be assigned a room").await?;
    // Locks only `room`: joining `room_type` into the locked query risks the join condition being checked
    // against a stale `room_type` row when a concurrent retype forces Postgres to re-evaluate it (EvalPlanQual),
    // which can make a genuinely mismatched type look like no row at all. Reading the type code as a second,
    // unlocked query avoids that, so a concurrent retype is correctly a wrong-type `Conflict`, not `NotFound`.
    let target: Option<RoomRow> =
        sqlx::query_as("select number, active, room_type_id from room where id = $1 and property_id = $2 for update")
            .bind(room)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let target = target.ok_or(ReservationsError::NotFound("room"))?;
    let number = &target.number;
    if !target.active {
        return Err(ReservationsError::Conflict(format!("room {number} is inactive")));
    }
    if target.room_type_id != stay.room_type_id {
        let type_code: String = sqlx::query_scalar("select code from room_type where id = $1")
            .bind(target.room_type_id)
            .fetch_one(&mut **tx)
            .await?;
        let booked: String = sqlx::query_scalar("select code from room_type where id = $1")
            .bind(stay.room_type_id)
            .fetch_one(&mut **tx)
            .await?;
        return Err(ReservationsError::Conflict(format!(
            "room {number} is a {type_code}, this booking is for {booked}"
        )));
    }
    if stay.room_id == Some(room) {
        return Err(ReservationsError::Conflict(format!("the stay is already in room {number}")));
    }
    let block: Option<(Date, Date)> = sqlx::query_as(
        "select lower(period), upper(period) from room_block
         where room_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period)
         limit 1",
    )
    .bind(room)
    .bind(stay.check_in)
    .bind(stay.check_out)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((from, to)) = block {
        return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
    }

    // A violated constraint aborts the transaction; the savepoint keeps it usable to name the booking in the
    // way. With the room locked, the other booking has committed by the time the constraint sees it.
    let mut savepoint = tx.begin().await?;
    let updated = sqlx::query_scalar(
        "update reservation_room set room_id = $2, version = version + 1 where id = $1 returning version",
    )
    .bind(id)
    .bind(room)
    .fetch_one(&mut *savepoint)
    .await;
    let version: i32 = match updated {
        Ok(version) => {
            savepoint.commit().await?;
            version
        }
        Err(err) if violates(&err, "reservation_room_no_double_booking") => {
            savepoint.rollback().await?;
            return Err(room_taken_conflict(tx, room, number, id, stay.check_in, stay.check_out).await?);
        }
        Err(err) => return Err(err.into()),
    };

    let previous = match stay.room_id {
        Some(previous) => Some(room_number(tx, previous).await?),
        None => None,
    };
    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "room_id": room,
        "number": number,
        "previous": previous,
    });
    finish(tx, tenant, actor, property, id, &stay, "reservation_room.assigned", data).await?;
    Ok(AssignedRoom {
        id,
        reservation_id: stay.reservation_id,
        room_id: Some(room),
        room_number: Some(target.number),
        version,
    })
}

/// Takes the stay `id` of the property, at `expected_version` (`VersionMismatch`) and confirmed (`Conflict`),
/// out of its room (`Conflict` if it has none). Locks only the stay's row: freeing a room cannot clash with a
/// block or another stay.
pub async fn unassign_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<AssignedRoom, ReservationsError> {
    let stay = lock_confirmed_stay(tx, property, id, expected_version, "have its room unassigned").await?;
    let room = stay.room_id.ok_or_else(|| ReservationsError::Conflict("the stay has no room assigned".into()))?;
    let version: i32 = sqlx::query_scalar(
        "update reservation_room set room_id = null, version = version + 1 where id = $1 returning version",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let number = room_number(tx, room).await?;
    let data = serde_json::json!({ "reservation_id": stay.reservation_id, "room_id": room, "number": number });
    finish(tx, tenant, actor, property, id, &stay, "reservation_room.unassigned", data).await?;
    Ok(AssignedRoom { id, reservation_id: stay.reservation_id, room_id: None, room_number: None, version })
}

/// Active rooms of `room_type` in the property that no stay holds and no block covers on any night of
/// `[check_in, check_out)`, in display order: the rooms a stay on those nights could be assigned.
pub async fn free_rooms(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    check_in: Date,
    check_out: Date,
) -> Result<Vec<FreeRoom>, ReservationsError> {
    if check_out <= check_in {
        return Err(ReservationsError::Invalid("check-out is after check-in".into()));
    }
    let rooms = sqlx::query_as(
        "select r.id, r.number, s.name as section
         from room r left join housekeeping_section s on s.id = r.section_id
         where r.property_id = $1 and r.room_type_id = $2 and r.active
           and not exists (
             select 1 from reservation_room a
             where a.room_id = r.id and a.status not in ('cancelled', 'no_show') and a.stay && daterange($3, $4))
           and not exists (
             select 1 from room_block b
             where b.room_id = r.id and b.released_at is null and b.period && daterange($3, $4))
         order by r.sort_order, r.number",
    )
    .bind(property)
    .bind(room_type)
    .bind(check_in)
    .bind(check_out)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rooms)
}

/// Locks the stay `id` of the property and checks its version and that it is confirmed; `doing` completes
/// "only a confirmed stay can …" in the refusal.
async fn lock_confirmed_stay(
    tx: &mut Tx,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    doing: &str,
) -> Result<StayRow, ReservationsError> {
    let row: Option<StayRow> = sqlx::query_as(
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
    let status = RoomStatus::parse(&stay.status).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "status".into(),
        source: format!("unknown {:?}", stay.status).into(),
    })?;
    if status != RoomStatus::Confirmed {
        return Err(ReservationsError::Conflict(format!(
            "only a confirmed stay can {doing}; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    Ok(stay)
}

async fn room_number(tx: &mut Tx, room: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("select number from room where id = $1").bind(room).fetch_one(&mut **tx).await
}

/// The `Conflict` naming the booking that already holds `room` on `[check_in, check_out)`, for the
/// `reservation_room_no_double_booking` savepoint-failure branch `assign_room` and `modify_room` both hit: the
/// stay `excluding` lost the room to whichever other stay got there first.
pub(crate) async fn room_taken_conflict(
    tx: &mut Tx,
    room: Uuid,
    number: &str,
    excluding: Uuid,
    check_in: Date,
    check_out: Date,
) -> Result<ReservationsError, sqlx::Error> {
    let taken_by: Option<String> = sqlx::query_scalar(
        "select r.confirmation_no
         from reservation_room s join reservation r on r.id = s.reservation_id
         where s.room_id = $1 and s.id <> $2 and s.status not in ('cancelled', 'no_show')
           and s.stay && daterange($3, $4)
         order by lower(s.stay)
         limit 1",
    )
    .bind(room)
    .bind(excluding)
    .bind(check_in)
    .bind(check_out)
    .fetch_optional(&mut **tx)
    .await?;
    let by = taken_by.map(|confirmation| format!(" by {confirmation}")).unwrap_or_default();
    Ok(ReservationsError::Conflict(format!("room {number} is taken{by} on those nights")))
}

/// Bumps the reservation's version (its detail shows the room), audits and queues the list and detail events.
#[allow(clippy::too_many_arguments)]
async fn finish(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    stay: &StayRow,
    action: &str,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;
    audit(tx, tenant, actor, action, "reservation_room", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(stay.reservation_id)]).await
}

/// The parts of a `reservation_room` that assigning reads.
#[derive(sqlx::FromRow)]
struct StayRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    check_out: Date,
    version: i32,
}

/// The room being assigned, locked alone (see `assign_room`).
#[derive(sqlx::FromRow)]
struct RoomRow {
    number: String,
    active: bool,
    room_type_id: Uuid,
}
