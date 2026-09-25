use crate::inventory::{adjust, business_date, contribute, extend_window, window_keys};
use crate::{RoomsError, audit, notify, reorder, rooms_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

/// Most rooms one range may create.
pub const MAX_ROOMS_PER_RANGE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Room {
    pub id: Uuid,
    pub property_id: Uuid,
    pub room_type_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
    pub active: bool,
    pub sort_order: i32,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRoom {
    pub room_type_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
}

/// Rooms `{prefix}{first}` to `{prefix}{last}`, e.g. 101 to 120, all of one type, floor and section.
#[derive(Debug, Clone)]
pub struct RoomRange {
    pub room_type_id: Uuid,
    pub prefix: String,
    pub first: u32,
    pub last: u32,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one.
#[derive(Debug, Clone, Default)]
pub struct RoomChanges {
    pub room_type_id: Option<Uuid>,
    pub number: Option<String>,
    pub floor: Option<Option<String>>,
    pub section_id: Option<Option<Uuid>>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, property_id, room_type_id, number, floor, section_id, active, sort_order, version";

/// Letters, digits and `-`, 1 to 10 characters (the database checks the same).
fn check_number(number: &str) -> Result<(), RoomsError> {
    let valid = (1..=10).contains(&number.len()) && number.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid { Ok(()) } else { Err(RoomsError::Invalid(format!("{number:?} is not a valid room number"))) }
}

/// The room type must be an active type of this property, and the section (if any) a section of it.
/// The type is read `for share`, so it cannot be retired until this transaction ends; retiring counts the
/// type's active rooms under `for update`, so it either waits for this room or this check sees it retired.
/// Sections have no active flag and are never deleted, so they need no lock.
async fn check_references(
    tx: &mut Tx,
    property: Uuid,
    room_type: Option<Uuid>,
    section: Option<Uuid>,
) -> Result<(), RoomsError> {
    if let Some(room_type) = room_type {
        let active: Option<bool> =
            sqlx::query_scalar("select active from room_type where id = $1 and property_id = $2 for share")
                .bind(room_type)
                .bind(property)
                .fetch_optional(&mut **tx)
                .await?;
        match active {
            None => return Err(RoomsError::Invalid("no such room type in this property".into())),
            Some(false) => return Err(RoomsError::Invalid("the room type is inactive".into())),
            Some(true) => {}
        }
    }
    if let Some(section) = section {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from housekeeping_section where id = $1 and property_id = $2)")
                .bind(section)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Err(RoomsError::Invalid("no such housekeeping section in this property".into()));
        }
    }
    Ok(())
}

fn numbers_taken(numbers: &[String]) -> RoomsError {
    match numbers {
        [one] => RoomsError::Conflict(format!("room {one} already exists")),
        many => RoomsError::Conflict(format!("rooms {} already exist", many.join(", "))),
    }
}

pub async fn create_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRoom,
) -> Result<Room, RoomsError> {
    let mut created = insert_rooms(
        tx,
        tenant,
        actor,
        property,
        input.room_type_id,
        vec![input.number],
        input.floor,
        input.section_id,
    )
    .await?;
    Ok(created.remove(0))
}

/// Creates every room in `range`, or none if any number is taken.
pub async fn create_rooms(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    range: RoomRange,
) -> Result<Vec<Room>, RoomsError> {
    if range.first > range.last {
        return Err(RoomsError::Invalid("the first room number must not be after the last".into()));
    }
    if range.last - range.first >= MAX_ROOMS_PER_RANGE {
        return Err(RoomsError::Invalid(format!("add at most {MAX_ROOMS_PER_RANGE} rooms at a time")));
    }
    let numbers = (range.first..=range.last).map(|n| format!("{}{n}", range.prefix)).collect();
    insert_rooms(tx, tenant, actor, property, range.room_type_id, numbers, range.floor, range.section_id).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_rooms(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_type: Uuid,
    numbers: Vec<String>,
    floor: Option<String>,
    section: Option<Uuid>,
) -> Result<Vec<Room>, RoomsError> {
    let today = business_date(tx, property).await?;
    numbers.iter().try_for_each(|number| check_number(number))?;
    check_references(tx, property, Some(room_type), section).await?;
    let taken: Vec<String> =
        sqlx::query_scalar("select number from room where property_id = $1 and number = any($2) order by number")
            .bind(property)
            .bind(&numbers)
            .fetch_all(&mut **tx)
            .await?;
    if !taken.is_empty() {
        return Err(numbers_taken(&taken));
    }
    extend_window(tx, property).await?;
    let ids: Vec<Uuid> = numbers.iter().map(|_| Uuid::now_v7()).collect();
    let inserted = sqlx::query_as::<_, Room>(sqlx::AssertSqlSafe(format!(
        "insert into room (id, tenant_id, property_id, room_type_id, number, floor, section_id, sort_order)
         select n.id, $1, $2, $3, n.number, $4, $5,
                (select coalesce(max(sort_order) + 1, 0) from room where property_id = $2) + n.position::integer - 1
         from unnest($6::uuid[], $7::text[]) with ordinality as n (id, number, position)
         returning {COLUMNS}"
    )))
    .bind(tenant.0)
    .bind(property)
    .bind(room_type)
    .bind(&floor)
    .bind(section)
    .bind(&ids)
    .bind(&numbers)
    .fetch_all(&mut **tx)
    .await;
    let mut created = match inserted {
        Ok(created) => created,
        // Another transaction took a number after the check above.
        Err(err) if violates(&err, "room_property_id_number_key") => return Err(numbers_taken(&numbers)),
        Err(err) => return Err(err.into()),
    };
    created.sort_by_key(|room| room.sort_order);
    let count = i32::try_from(created.len()).expect("at most MAX_ROOMS_PER_RANGE rooms");
    adjust(tx, property, room_type, today, None, count, 0).await?;
    // One entry for the batch, on the room type the rooms were added to.
    audit(tx, tenant, actor, "rooms.created", "room_type", room_type, serde_json::json!({ "numbers": numbers }))
        .await?;
    let mut keys = vec![rooms_key(property)];
    keys.extend(window_keys(property, today));
    notify(tx, tenant, property, keys).await?;
    Ok(created)
}

/// Changes a room. Retyping, deactivating or reactivating moves its share of the inventory counters
/// (including its out-of-order blocks) from the business date on.
pub async fn update_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomChanges,
) -> Result<Room, RoomsError> {
    let today: Date = business_date(tx, property).await?;
    let current: Room = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RoomsError::NotFound("room"))?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("room"));
    }
    if let Some(number) = &changes.number {
        check_number(number)?;
    }
    let room_type = changes.room_type_id.unwrap_or(current.room_type_id);
    let active = changes.active.unwrap_or(current.active);
    let type_to_check = (changes.room_type_id.is_some() || (active && !current.active)).then_some(room_type);
    check_references(tx, property, type_to_check, changes.section_id.flatten()).await?;
    let moves_counts = room_type != current.room_type_id || active != current.active;
    if moves_counts {
        extend_window(tx, property).await?;
    }
    let updated = sqlx::query_as::<_, Room>(sqlx::AssertSqlSafe(format!(
        "update room set room_type_id = $2, number = coalesce($3, number),
                floor = case when $4 then $5 else floor end,
                section_id = case when $6 then $7 else section_id end,
                active = $8, version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(room_type)
    .bind(&changes.number)
    .bind(changes.floor.is_some())
    .bind(changes.floor.clone().flatten())
    .bind(changes.section_id.is_some())
    .bind(changes.section_id.flatten())
    .bind(active)
    .fetch_one(&mut **tx)
    .await;
    let updated = match updated {
        Ok(updated) => updated,
        Err(err) if violates(&err, "room_property_id_number_key") => {
            return Err(numbers_taken(&[changes.number.unwrap_or_default()]));
        }
        Err(err) => return Err(err.into()),
    };
    let mut keys = vec![rooms_key(property)];
    if moves_counts {
        // Lock order: inventory_day rows are locked in ascending (room_type_id, date) order, so two opposite
        // retypes cannot deadlock. Each call keeps its own type's counters valid, so either order is correct.
        let mut shares = [(current.room_type_id, current.active, -1), (updated.room_type_id, updated.active, 1)];
        shares.sort_by_key(|&(room_type, _, _)| room_type);
        for (room_type, active, sign) in shares {
            contribute(tx, property, today, id, room_type, active, sign).await?;
        }
        keys.extend(window_keys(property, today));
    }
    audit(
        tx,
        tenant,
        actor,
        "room.updated",
        "room",
        id,
        serde_json::json!({ "room_type_id": room_type, "active": active, "number": updated.number }),
    )
    .await?;
    notify(tx, tenant, property, keys).await?;
    Ok(updated)
}

/// Sets the display order. `ids` must list every room of the property exactly once.
pub async fn reorder_rooms(tx: &mut Tx, tenant: TenantId, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    business_date(tx, property).await?;
    reorder(tx, "room", property, ids).await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(())
}

/// The property's rooms, active or not, in display order; only those of `room_type` if given.
pub async fn list_rooms(tx: &mut Tx, property: Uuid, room_type: Option<Uuid>) -> Result<Vec<Room>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room where property_id = $1 and ($2::uuid is null or room_type_id = $2)
         order by sort_order, number"
    )))
    .bind(property)
    .bind(room_type)
    .fetch_all(&mut **tx)
    .await
}
