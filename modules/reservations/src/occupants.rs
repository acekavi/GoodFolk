//! Additional occupants of a booked room: who besides the primary guest is staying in it, added or removed
//! without touching the stay's dates, type, price or the primary guest itself.

use crate::guests::COLUMNS as GUEST_COLUMNS;
use crate::{Guest, ReservationsError, audit, decode_error, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use serde::Serialize;
use uuid::Uuid;

/// A room after [`add_occupant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RoomOccupant {
    pub room_id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    /// The guest just added, masked exactly as [`crate::get_guest`] shows it.
    pub guest: Guest,
}

/// A room after [`remove_occupant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RemovedOccupant {
    pub room_id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub guest_id: Uuid,
}

/// Adds `guest_id` as an occupant of room `room_id` of the property, at `expected_version` (`VersionMismatch`).
///
/// The room must be confirmed or checked in ([`Conflict`](ReservationsError::Conflict) naming its actual status
/// otherwise, exactly as [`crate::modify_room`] refuses every other status). `guest_id` must be a guest of this
/// tenant (`Invalid "no such guest"` otherwise -- this also keeps another tenant's guest from ever being added);
/// it must not be the room's own primary guest (`Invalid "<name> is already the room's primary guest"`), and it
/// must not already be listed (`Conflict`). The room's type caps how many occupants it can hold in total,
/// primary guest included: at most `max_occupancy - 1` rows in `reservation_guest`, or `Invalid "a <code> room
/// holds at most <max_occupancy> guests"`.
///
/// Locks the `reservation_room` row (no other lock is needed: `reservation_guest` rows are only ever written
/// while it is held). Bumps both versions, audits `reservation_room.occupant_added` (the guest's id and name,
/// never anything else about them) and notifies the reservation list and this reservation's detail.
pub async fn add_occupant(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_id: Uuid,
    expected_version: i32,
    guest_id: Uuid,
) -> Result<RoomOccupant, ReservationsError> {
    let room = lock_room(tx, property, room_id, expected_version, "have an occupant added").await?;

    let guest: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {GUEST_COLUMNS} from guest where id = $1")))
        .bind(guest_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| invalid("no such guest"))?;
    if guest.id == room.primary_guest_id {
        return Err(invalid(format!(
            "{} is already the room's primary guest",
            full_name(&guest.first_name, &guest.last_name)
        )));
    }
    let already: bool = sqlx::query_scalar(
        "select exists(select 1 from reservation_guest where reservation_room_id = $1 and guest_id = $2)",
    )
    .bind(room_id)
    .bind(guest_id)
    .fetch_one(&mut **tx)
    .await?;
    if already {
        return Err(ReservationsError::Conflict(format!(
            "{} is already an occupant of this room",
            full_name(&guest.first_name, &guest.last_name)
        )));
    }

    let (type_code, max_occupancy): (String, i32) =
        sqlx::query_as("select code, max_occupancy from room_type where id = $1 and property_id = $2")
            .bind(room.room_type_id)
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
    let occupants: i64 = sqlx::query_scalar("select count(*) from reservation_guest where reservation_room_id = $1")
        .bind(room_id)
        .fetch_one(&mut **tx)
        .await?;
    if occupants >= i64::from(max_occupancy - 1) {
        return Err(invalid(format!("a {type_code} room holds at most {max_occupancy} guests")));
    }

    sqlx::query(
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         values ($1, $2, $3, $4)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(room_id)
    .bind(guest_id)
    .execute(&mut **tx)
    .await?;

    let version = bump(tx, room_id, room.reservation_id).await?;
    let name = full_name(&guest.first_name, &guest.last_name);
    let data = serde_json::json!({ "guest_id": guest_id, "name": name });
    audit(tx, tenant, actor, "reservation_room.occupant_added", "reservation_room", room_id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(room.reservation_id)]).await?;

    Ok(RoomOccupant { room_id, reservation_id: room.reservation_id, version, guest })
}

/// Removes `guest_id` from room `room_id` of the property's occupants, at `expected_version`
/// (`VersionMismatch`).
///
/// The room must be confirmed or checked in, exactly as [`add_occupant`] requires. `guest_id` must currently be
/// listed as an occupant of this room, or this is `NotFound "occupant"` (this also refuses another tenant's
/// guest, and the room's own primary guest, who is never in the occupant list).
///
/// Locks the `reservation_room` row, as [`add_occupant`] does. Bumps both versions, audits
/// `reservation_room.occupant_removed` (the guest's id and name) and notifies as [`add_occupant`] does.
pub async fn remove_occupant(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_id: Uuid,
    expected_version: i32,
    guest_id: Uuid,
) -> Result<RemovedOccupant, ReservationsError> {
    let room = lock_room(tx, property, room_id, expected_version, "have an occupant removed").await?;

    let occupant: Option<(String, String)> = sqlx::query_as(
        "select g.first_name, g.last_name from reservation_guest rg join guest g on g.id = rg.guest_id
         where rg.reservation_room_id = $1 and rg.guest_id = $2",
    )
    .bind(room_id)
    .bind(guest_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (first_name, last_name) = occupant.ok_or(ReservationsError::NotFound("occupant"))?;

    sqlx::query("delete from reservation_guest where reservation_room_id = $1 and guest_id = $2")
        .bind(room_id)
        .bind(guest_id)
        .execute(&mut **tx)
        .await?;

    let version = bump(tx, room_id, room.reservation_id).await?;
    let data = serde_json::json!({ "guest_id": guest_id, "name": full_name(&first_name, &last_name) });
    audit(tx, tenant, actor, "reservation_room.occupant_removed", "reservation_room", room_id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(room.reservation_id)]).await?;

    Ok(RemovedOccupant { room_id, reservation_id: room.reservation_id, version, guest_id })
}

fn invalid(message: impl Into<String>) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

/// `first last`, or just `last` for a guest with a single name (the same shape a reservation list row's guest
/// name takes).
fn full_name(first_name: &str, last_name: &str) -> String {
    if first_name.is_empty() { last_name.to_owned() } else { format!("{first_name} {last_name}") }
}

/// Locks the `reservation_room` row `id` of the property and checks its version and that it is confirmed or
/// checked in; `doing` completes "only a confirmed or checked-in room can …" in the refusal.
async fn lock_room(
    tx: &mut Tx,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    doing: &str,
) -> Result<RoomRow, ReservationsError> {
    let row: Option<RoomRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, primary_guest_id, version
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
    let status = RoomStatus::parse(&row.status).ok_or_else(|| decode_error("status", &row.status))?;
    if !matches!(status, RoomStatus::Confirmed | RoomStatus::CheckedIn) {
        return Err(ReservationsError::Conflict(format!(
            "only a confirmed or checked-in room can {doing}; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    Ok(row)
}

/// Bumps the room's and the reservation's version, returning the room's new one.
async fn bump(tx: &mut Tx, room_id: Uuid, reservation_id: Uuid) -> Result<i32, sqlx::Error> {
    let version: i32 =
        sqlx::query_scalar("update reservation_room set version = version + 1 where id = $1 returning version")
            .bind(room_id)
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;
    Ok(version)
}

/// The parts of a `reservation_room` that adding or removing an occupant reads.
#[derive(sqlx::FromRow)]
struct RoomRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    primary_guest_id: Uuid,
    version: i32,
}
