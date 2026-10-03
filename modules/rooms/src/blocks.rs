use crate::inventory::{WINDOW_DAYS, adjust, business_date, clamped_month_keys, extend_window, lock_days};
use crate::{RoomsError, assigned_stay, audit, notify, rooms_key, tape_keys, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use time::{Date, Duration};
use uuid::Uuid;

/// `OutOfOrder` takes the room out of inventory (renovation, construction); `OutOfService` leaves it
/// sellable and only flags it (a short fix, a deep clean).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    OutOfOrder,
    OutOfService,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::OutOfOrder => "out_of_order",
            BlockKind::OutOfService => "out_of_service",
        }
    }

    pub fn parse(value: &str) -> Option<BlockKind> {
        [BlockKind::OutOfOrder, BlockKind::OutOfService].into_iter().find(|kind| kind.as_str() == value)
    }
}

impl TryFrom<String> for BlockKind {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        BlockKind::parse(&value).ok_or_else(|| format!("unknown block kind {value:?}"))
    }
}

/// Why a room is blocked. Each property starts with [`DEFAULT_BLOCK_REASONS`] and may add its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct BlockReason {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub label: String,
    #[sqlx(try_from = "String")]
    pub default_kind: BlockKind,
    pub active: bool,
    pub version: i32,
}

/// Reasons every new property starts with (`migrations/0004_rooms_inventory.sql` seeds existing ones).
pub const DEFAULT_BLOCK_REASONS: [(&str, &str, BlockKind); 5] = [
    ("RENOVATION", "Renovation", BlockKind::OutOfOrder),
    ("CONSTRUCTION", "Construction", BlockKind::OutOfOrder),
    ("MAINTENANCE", "Maintenance", BlockKind::OutOfOrder),
    ("DEEP_CLEAN", "Deep clean", BlockKind::OutOfService),
    ("OTHER", "Other", BlockKind::OutOfOrder),
];

#[derive(Debug, Clone)]
pub struct NewBlockReason {
    pub code: String,
    pub label: String,
    pub default_kind: BlockKind,
}

/// `None` leaves a field unchanged. The code never changes.
#[derive(Debug, Clone, Default)]
pub struct BlockReasonChanges {
    pub label: Option<String>,
    pub default_kind: Option<BlockKind>,
    pub active: Option<bool>,
}

/// A room blocked for `[from, to)`: `to` is the first day it is back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Block {
    pub id: Uuid,
    pub property_id: Uuid,
    pub room_id: Uuid,
    pub from: Date,
    pub to: Date,
    #[sqlx(try_from = "String")]
    pub kind: BlockKind,
    pub reason_id: Uuid,
    pub note: String,
    /// Cancelled before it started. Released blocks no longer block anything.
    pub released: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewBlock {
    pub room_id: Uuid,
    pub from: Date,
    pub to: Date,
    pub kind: BlockKind,
    pub reason_id: Uuid,
    pub note: String,
}

const REASON_COLUMNS: &str = "id, property_id, code, label, default_kind, active, version";
const BLOCK_COLUMNS: &str = "id, property_id, room_id, lower(period) as \"from\", upper(period) as \"to\", kind, \
                             reason_id, note, released_at is not null as released, version";

/// Gives a new property the default block reasons, in the property's creation transaction.
pub async fn seed_block_reasons(tx: &mut Tx, tenant: TenantId, property: Uuid) -> Result<(), sqlx::Error> {
    for (code, label, kind) in DEFAULT_BLOCK_REASONS {
        sqlx::query(
            "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(property)
        .bind(code)
        .bind(label)
        .bind(kind.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn create_block_reason(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewBlockReason,
) -> Result<BlockReason, RoomsError> {
    business_date(tx, property).await?;
    let inserted = sqlx::query_as::<_, BlockReason>(sqlx::AssertSqlSafe(format!(
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         values ($1, $2, $3, $4, $5, $6)
         returning {REASON_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.label)
    .bind(input.default_kind.as_str())
    .fetch_one(&mut **tx)
    .await;
    let reason = match inserted {
        Ok(reason) => reason,
        Err(err) if violates(&err, "block_reason_property_id_code_key") => {
            return Err(RoomsError::Conflict(format!("a block reason with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    };
    audit(
        tx,
        tenant,
        actor,
        "block_reason.created",
        "block_reason",
        reason.id,
        serde_json::json!({ "code": input.code }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(reason)
}

pub async fn update_block_reason(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: BlockReasonChanges,
) -> Result<BlockReason, RoomsError> {
    let updated: Option<BlockReason> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update block_reason set label = coalesce($4, label), default_kind = coalesce($5, default_kind),
                active = coalesce($6, active), version = version + 1
         where id = $1 and property_id = $2 and version = $3
         returning {REASON_COLUMNS}"
    )))
    .bind(id)
    .bind(property)
    .bind(expected_version)
    .bind(&changes.label)
    .bind(changes.default_kind.map(BlockKind::as_str))
    .bind(changes.active)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(reason) = updated else {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from block_reason where id = $1 and property_id = $2)")
                .bind(id)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        return Err(if exists {
            RoomsError::VersionMismatch("block reason")
        } else {
            RoomsError::NotFound("block reason")
        });
    };
    audit(
        tx,
        tenant,
        actor,
        "block_reason.updated",
        "block_reason",
        id,
        serde_json::json!({ "active": changes.active }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(reason)
}

/// Every block reason of the property, active or not, by code.
pub async fn list_block_reasons(tx: &mut Tx, property: Uuid) -> Result<Vec<BlockReason>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {REASON_COLUMNS} from block_reason where property_id = $1 order by code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// Active blocks of `room` that overlap `[from, to)`.
async fn overlapping(tx: &mut Tx, room: Uuid, from: Date, to: Date) -> Result<Vec<Block>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {BLOCK_COLUMNS} from room_block
         where room_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period)"
    )))
    .bind(room)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}

/// Locks the room (serializing its blocks, retyping and assignments) and returns its type, whether it is
/// active and its number.
async fn lock_room(tx: &mut Tx, property: Uuid, room: Uuid) -> Result<(Uuid, bool, String), RoomsError> {
    sqlx::query_as("select room_type_id, active, number from room where id = $1 and property_id = $2 for update")
        .bind(room)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RoomsError::NotFound("room"))
}

/// Blocks a room for `[from, to)`. Out-of-order blocks take it out of its type's availability on those days.
/// Fails with [`RoomsError::Overlap`], listing the blocks in the way, if the room is already blocked then, and
/// with [`RoomsError::Conflict`] if a stay is assigned to it on any of those days.
pub async fn create_block(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewBlock,
) -> Result<Block, RoomsError> {
    let today = business_date(tx, property).await?;
    if input.to <= input.from {
        return Err(RoomsError::Invalid("a block must end after it starts".into()));
    }
    if input.from < today {
        return Err(RoomsError::Invalid(format!("blocks cannot start before the business date ({today})")));
    }
    // Counters exist only this far ahead, and every month of a block goes into its change event.
    if input.to > today + Duration::days(WINDOW_DAYS) {
        return Err(RoomsError::Invalid(format!("a block can end at most {WINDOW_DAYS} days after the business date")));
    }
    let (room_type, active, number) = lock_room(tx, property, input.room_id).await?;
    if !active {
        return Err(RoomsError::Invalid("the room is inactive".into()));
    }
    let reason_active: Option<bool> =
        sqlx::query_scalar("select active from block_reason where id = $1 and property_id = $2")
            .bind(input.reason_id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    if reason_active != Some(true) {
        return Err(RoomsError::Invalid("no such active block reason in this property".into()));
    }
    let conflicts = overlapping(tx, input.room_id, input.from, input.to).await?;
    if !conflicts.is_empty() {
        return Err(RoomsError::Overlap(conflicts));
    }
    if let Some((confirmation, _)) = assigned_stay(tx, input.room_id, input.from, Some(input.to)).await? {
        return Err(RoomsError::Conflict(format!("room {number} is assigned to {confirmation} on those nights")));
    }
    extend_window(tx, property).await?;
    if input.kind == BlockKind::OutOfOrder {
        lock_days(tx, property, &[room_type], input.from, input.to).await?;
    }
    let inserted = sqlx::query_as::<_, Block>(sqlx::AssertSqlSafe(format!(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id, note, created_by)
         values ($1, $2, $3, $4, daterange($5, $6), $7, $8, $9, $10)
         returning {BLOCK_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(input.room_id)
    .bind(input.from)
    .bind(input.to)
    .bind(input.kind.as_str())
    .bind(input.reason_id)
    .bind(&input.note)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await;
    let block = match inserted {
        Ok(block) => block,
        // The room lock above makes this unreachable in practice; the constraint is the last line of defence.
        Err(err) if violates(&err, "room_block_no_overlap") => return Err(RoomsError::Overlap(Vec::new())),
        Err(err) => return Err(err.into()),
    };
    if block.kind == BlockKind::OutOfOrder {
        adjust(tx, property, room_type, block.from, block.to, 0, 1).await?;
    }
    audit(
        tx,
        tenant,
        actor,
        "room_block.created",
        "room_block",
        block.id,
        serde_json::json!({ "room_id": block.room_id, "from": block.from, "to": block.to, "kind": block.kind }),
    )
    .await?;
    let keys = clamped_month_keys(property, today, block.from, block.to)
        .into_iter()
        .chain(tape_keys(property, block.from, block.to))
        .collect();
    notify(tx, tenant, property, keys).await?;
    Ok(block)
}

/// Ends a block early: the room is back from `to`, which may not be before the business date and must be
/// before the block's current end. If `to` is on or before the start, the block is cancelled (released).
/// Counters are restored for the days the block no longer covers.
pub async fn shorten_block(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    to: Date,
) -> Result<Block, RoomsError> {
    let today = business_date(tx, property).await?;
    let room: Option<Uuid> =
        sqlx::query_scalar("select room_id from room_block where id = $1 and property_id = $2 and released_at is null")
            .bind(id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let room = room.ok_or(RoomsError::NotFound("block"))?;
    let (room_type, room_active, _) = lock_room(tx, property, room).await?;
    let current: Block =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {BLOCK_COLUMNS} from room_block where id = $1 for update")))
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("block"));
    }
    if to < today || to >= current.to {
        return Err(RoomsError::Invalid(format!(
            "a block can only be shortened, to end between the business date ({today}) and {}",
            current.to
        )));
    }
    let cancelled = to <= current.from;
    let block: Block = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update room_block set
             period = case when $2 then period else daterange(lower(period), $3) end,
             released_at = case when $2 then now() end,
             version = version + 1
         where id = $1
         returning {BLOCK_COLUMNS}"
    )))
    .bind(id)
    .bind(cancelled)
    .bind(to)
    .fetch_one(&mut **tx)
    .await?;
    // Days the block covered from the business date on, and no longer does.
    let restored_from = if cancelled { current.from.max(today) } else { to };
    if current.kind == BlockKind::OutOfOrder && room_active {
        lock_days(tx, property, &[room_type], restored_from, current.to).await?;
        adjust(tx, property, room_type, restored_from, current.to, 0, -1).await?;
    }
    audit(
        tx,
        tenant,
        actor,
        if cancelled { "room_block.cancelled" } else { "room_block.shortened" },
        "room_block",
        id,
        serde_json::json!({ "to": to }),
    )
    .await?;
    // The original range, before shortening: the tape chart shows the block's whole span, not just the days
    // whose counters changed.
    let keys = clamped_month_keys(property, today, restored_from, current.to)
        .into_iter()
        .chain(tape_keys(property, current.from, current.to))
        .collect();
    notify(tx, tenant, property, keys).await?;
    Ok(block)
}

/// Active blocks overlapping `[from, to)`, by start date.
pub async fn list_blocks(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Vec<Block>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {BLOCK_COLUMNS} from room_block
         where property_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period), room_id"
    )))
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
