//! A room's cleaning condition: housekeepers move their assigned rooms one step forward, supervisors set any
//! value.

use crate::{HousekeepingError, audit, hk_key, notify, role_holders};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

/// The most rooms one bulk change names.
pub const MAX_BULK_ROOMS: usize = 500;

db::text_enum!(
    /// How ready a room is for a guest.
    Condition { Dirty = "dirty", Cleaning = "cleaning", Clean = "clean", Inspected = "inspected" }
);

/// Who changes the condition; the route decides from the caller's permissions.
#[derive(Debug, Clone, Copy)]
pub enum Actor {
    /// Limited to their assigned rooms for the business date, one step forward.
    Housekeeper(UserId),
    /// Any room, any value.
    Supervisor(UserId),
}

impl Actor {
    fn user(self) -> UserId {
        match self {
            Actor::Housekeeper(user) | Actor::Supervisor(user) => user,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RoomConditionRow {
    pub room_id: Uuid,
    pub condition: Condition,
    pub version: i32,
}

/// Whether a housekeeper may move a room from `from` to `to`: dirty to cleaning and cleaning to clean.
pub fn housekeeper_may(from: Condition, to: Condition) -> bool {
    matches!((from, to), (Condition::Dirty, Condition::Cleaning) | (Condition::Cleaning, Condition::Clean))
}

fn parse(value: &str) -> Result<Condition, sqlx::Error> {
    Condition::parse(value).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "condition".into(),
        source: format!("unknown value {value:?}").into(),
    })
}

/// Changes the condition of `room` in the property, which must be at `expected_version` (`VersionMismatch`).
/// Setting the condition it already has is `Invalid`. A `Housekeeper` needs the room assigned to them for the
/// business date (`Forbidden`), must still hold a housekeeping role at the property (`Forbidden`) and may only
/// take the steps [`housekeeper_may`] allows (`Invalid`).
pub async fn set_condition(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Actor,
    property: Uuid,
    room: Uuid,
    expected_version: i32,
    to: Condition,
) -> Result<RoomConditionRow, HousekeepingError> {
    let current: Option<(String, i32)> = sqlx::query_as(
        "select condition, version from room_condition where room_id = $1 and property_id = $2 for update",
    )
    .bind(room)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (from, version) = current.ok_or(HousekeepingError::NotFound("room"))?;
    if version != expected_version {
        return Err(HousekeepingError::VersionMismatch("room condition"));
    }
    let from = parse(&from)?;
    if let Actor::Housekeeper(user) = actor {
        require_housekeeper(tx, property, room, user).await?;
        if !housekeeper_may(from, to) {
            return Err(HousekeepingError::Invalid(format!(
                "a housekeeper cannot change a room from {} to {}",
                from.as_str(),
                to.as_str()
            )));
        }
    }
    if from == to {
        return Err(HousekeepingError::Invalid(format!("the room is already {}", to.as_str())));
    }
    let row = write(tx, tenant, actor.user(), property, room, from, to).await?;
    notify(tx, tenant, property, vec![hk_key(property)]).await?;
    Ok(row)
}

/// Sets the condition of every room in `rooms`, at most [`MAX_BULK_ROOMS`], for a supervisor. A room outside
/// the property is `NotFound` and nothing is written. No version is checked, and a room already at `to`
/// is written like any other.
pub async fn set_conditions(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    rooms: &[Uuid],
    to: Condition,
) -> Result<Vec<RoomConditionRow>, HousekeepingError> {
    if rooms.is_empty() || rooms.len() > MAX_BULK_ROOMS {
        return Err(HousekeepingError::Invalid(format!("name between 1 and {MAX_BULK_ROOMS} rooms")));
    }
    let mut ids = rooms.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let locked: Vec<(Uuid, String)> = sqlx::query_as(
        "select room_id, condition from room_condition where property_id = $1 and room_id = any($2)
         order by room_id for update",
    )
    .bind(property)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked.len() != ids.len() {
        return Err(HousekeepingError::NotFound("room"));
    }
    let mut written = Vec::with_capacity(locked.len());
    for (room, from) in locked {
        written.push(write(tx, tenant, actor, property, room, parse(&from)?, to).await?);
    }
    notify(tx, tenant, property, vec![hk_key(property)]).await?;
    Ok(written)
}

/// The housekeeper must be assigned the room for the business date and still hold a housekeeping role.
async fn require_housekeeper(tx: &mut Tx, property: Uuid, room: Uuid, user: UserId) -> Result<(), HousekeepingError> {
    let assigned: bool = sqlx::query_scalar(
        "select exists (select 1 from hk_assignment a join property p on p.id = a.property_id
                        where a.property_id = $1 and a.room_id = $2 and a.user_id = $3
                          and a.business_date = p.business_date)",
    )
    .bind(property)
    .bind(room)
    .bind(user.0)
    .fetch_one(&mut **tx)
    .await?;
    if !assigned {
        return Err(HousekeepingError::Forbidden("the room is not assigned to you today"));
    }
    let holds_role = !role_holders(tx, property, &[user.0]).await?.is_empty();
    if !holds_role {
        return Err(HousekeepingError::Forbidden("you no longer hold a housekeeping role here"));
    }
    Ok(())
}

/// Writes `to` onto a locked row, with its audit entry.
async fn write(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room: Uuid,
    from: Condition,
    to: Condition,
) -> Result<RoomConditionRow, HousekeepingError> {
    let version: i32 = sqlx::query_scalar(
        "update room_condition set condition = $1, version = version + 1, changed_at = now(), changed_by = $2
         where room_id = $3 and property_id = $4 returning version",
    )
    .bind(to.as_str())
    .bind(actor.0)
    .bind(room)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "room.condition_changed",
        "room",
        room,
        serde_json::json!({ "from": from.as_str(), "to": to.as_str() }),
    )
    .await?;
    Ok(RoomConditionRow { room_id: room, condition: to, version })
}
