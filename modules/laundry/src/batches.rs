//! Laundry batches: linen sent to a laundry or vendor, counted back in, and the differences settled later.
//!
//! Each command locks the batch row first, then calls [`stock::apply`] exactly once (its advisory lock is held until
//! commit, so it is the transaction's last locking step) and only then updates the batch row it already holds.

use crate::stock::{self, MovementKind, NewMovement};
use crate::{LaundryError, audit, linen_key, notify};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;
use uuid::Uuid;

/// The most items one batch may list.
const MAX_ITEMS: usize = 200;
/// The most batches one listing returns.
const MAX_LISTED: i64 = 200;
/// The longest a batch note or a write-off reason may be, in characters.
const MAX_TEXT_CHARS: usize = 500;

db::text_enum!(
    /// Where a batch is: out at the laundry, or counted back in (it may still have items outstanding).
    BatchStatus { Sent = "sent", Received = "received" }
);

/// One item of a batch. Outstanding is `sent - returned - written_off`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BatchLine {
    pub item_id: Uuid,
    pub sent: i32,
    pub returned: i32,
    pub written_off: i32,
}

impl BatchLine {
    fn outstanding(&self) -> i32 {
        self.sent - self.returned - self.written_off
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Batch {
    pub id: Uuid,
    pub from_location_id: Uuid,
    pub to_location_id: Uuid,
    pub status: BatchStatus,
    pub note: Option<String>,
    pub sent_by: Option<Uuid>,
    pub sent_at: OffsetDateTime,
    pub received_by: Option<Uuid>,
    pub received_at: Option<OffsetDateTime>,
    pub version: i32,
    pub lines: Vec<BatchLine>,
}

impl Batch {
    /// Items still at the laundry: sent, less returned and written off.
    pub fn outstanding(&self) -> i32 {
        self.lines.iter().map(BatchLine::outstanding).sum()
    }

    /// Counted back in with nothing outstanding.
    pub fn settled(&self) -> bool {
        self.status == BatchStatus::Received && self.outstanding() == 0
    }
}

/// How to settle what a received batch still has out: items found and returned, or lost and written off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settle {
    Return { back_to: Option<Uuid>, counts: Vec<(Uuid, i32)> },
    WriteOff { reason: String, counts: Vec<(Uuid, i32)> },
}

#[derive(sqlx::FromRow)]
struct BatchRow {
    id: Uuid,
    from_location_id: Uuid,
    to_location_id: Uuid,
    #[sqlx(try_from = "String")]
    status: BatchStatus,
    note: Option<String>,
    sent_by: Option<Uuid>,
    sent_at: OffsetDateTime,
    received_by: Option<Uuid>,
    received_at: Option<OffsetDateTime>,
    version: i32,
}

/// `select` of a batch's columns, followed by `$rest`.
macro_rules! select_batch {
    ($rest:literal) => {
        concat!(
            "select id, from_location_id, to_location_id, status, note, sent_by, sent_at, received_by, received_at, ",
            "version from laundry_batch ",
            $rest
        )
    };
}

impl BatchRow {
    fn with_lines(self, lines: Vec<BatchLine>) -> Batch {
        Batch {
            id: self.id,
            from_location_id: self.from_location_id,
            to_location_id: self.to_location_id,
            status: self.status,
            note: self.note,
            sent_by: self.sent_by,
            sent_at: self.sent_at,
            received_by: self.received_by,
            received_at: self.received_at,
            version: self.version,
            lines,
        }
    }
}

/// Sends `counts` (1 to 200 distinct items, each > 0) from `from` (a store or pantry) to `to` (a laundry or vendor).
#[allow(clippy::too_many_arguments)]
pub async fn send(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    from: Uuid,
    to: Uuid,
    counts: &[(Uuid, i32)],
    note: Option<String>,
) -> Result<Batch, LaundryError> {
    if counts.is_empty() || counts.len() > MAX_ITEMS {
        return Err(LaundryError::Invalid(format!("a batch lists 1 to {MAX_ITEMS} items")));
    }
    if counts.iter().map(|(item, _)| item).collect::<BTreeSet<_>>().len() != counts.len() {
        return Err(LaundryError::Invalid("an item is listed once per batch".into()));
    }
    if counts.iter().any(|(_, quantity)| *quantity <= 0) {
        return Err(LaundryError::Invalid("a count is more than 0".into()));
    }
    if from == to {
        return Err(LaundryError::Invalid("a batch goes between two different locations".into()));
    }
    let note = note.as_deref().map(str::trim).filter(|note| !note.is_empty()).map(str::to_owned);
    if note.as_ref().is_some_and(|note| note.chars().count() > MAX_TEXT_CHARS) {
        return Err(LaundryError::Invalid(format!("a note is at most {MAX_TEXT_CHARS} characters")));
    }
    let found: i64 = sqlx::query_scalar("select count(*) from linen_location where property_id = $1 and id = any($2)")
        .bind(property)
        .bind([from, to])
        .fetch_one(&mut **tx)
        .await?;
    if found != 2 {
        return Err(LaundryError::NotFound("location"));
    }

    let id = Uuid::now_v7();
    sqlx::query(
        "insert into laundry_batch (id, tenant_id, property_id, from_location_id, to_location_id, status, note, sent_by)
         values ($1, $2, $3, $4, $5, 'sent', $6, $7)",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(from)
    .bind(to)
    .bind(&note)
    .bind(actor.0)
    .execute(&mut **tx)
    .await?;

    let movements: Vec<NewMovement> = counts
        .iter()
        .map(|&(item_id, quantity)| NewMovement {
            item_id,
            kind: MovementKind::Send,
            quantity,
            from_location_id: Some(from),
            to_location_id: Some(to),
            reason: None,
        })
        .collect();
    stock::apply(tx, tenant, actor, property, Some(id), &movements).await?;

    let items: Vec<_> = counts.iter().map(|&(item, quantity)| json!({ "item_id": item, "sent": quantity })).collect();
    audit(
        tx,
        tenant,
        actor,
        "laundry_batch.sent",
        "laundry_batch",
        id,
        json!({ "from_location_id": from, "to_location_id": to, "items": items }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    load(tx, property, id).await
}

/// Counts the batch back in: per batch item `0 <= count <= outstanding`, items left out count as 0. Returns each
/// non-zero count to `back_to` (default the location the batch was sent from) and marks the batch `received`; the
/// difference stays in the laundry's stock.
#[allow(clippy::too_many_arguments)]
pub async fn receive(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    batch: Uuid,
    expected_version: i32,
    back_to: Option<Uuid>,
    counts: &[(Uuid, i32)],
) -> Result<Batch, LaundryError> {
    let current = lock(tx, property, batch, expected_version).await?;
    if current.status != BatchStatus::Sent {
        return Err(LaundryError::Conflict("the batch was already received".into()));
    }
    let lines = lines(tx, &[batch]).await?.remove(&batch).unwrap_or_default();
    let counted = checked_counts(tx, &lines, counts, true).await?;

    let to = back_to.unwrap_or(current.from_location_id);
    let movements = movements(&counted, MovementKind::Return, current.to_location_id, Some(to), None);
    stock::apply(tx, tenant, actor, property, Some(batch), &movements).await?;

    sqlx::query("update laundry_batch set status = 'received', received_by = $2, received_at = now(), version = version + 1 where id = $1")
        .bind(batch)
        .bind(actor.0)
        .execute(&mut **tx)
        .await?;
    let items: Vec<_> = lines
        .iter()
        .map(|line| {
            let received = counted.get(&line.item_id).copied().unwrap_or(0);
            json!({ "item_id": line.item_id, "sent": line.sent, "received": received, "difference": line.sent - received })
        })
        .collect();
    audit(
        tx,
        tenant,
        actor,
        "laundry_batch.received",
        "laundry_batch",
        batch,
        json!({ "back_to_location_id": to, "items": items }),
    )
    .await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    load(tx, property, batch).await
}

/// Settles part of what a received batch still has out, by a further return or a write-off with a reason; each count
/// is at most what is outstanding and at least one is above 0.
pub async fn settle(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    batch: Uuid,
    expected_version: i32,
    settle: Settle,
) -> Result<Batch, LaundryError> {
    let current = lock(tx, property, batch, expected_version).await?;
    if current.status != BatchStatus::Received {
        return Err(LaundryError::Conflict("the batch is not received yet".into()));
    }
    let lines = lines(tx, &[batch]).await?.remove(&batch).unwrap_or_default();
    if lines.iter().all(|line| line.outstanding() == 0) {
        return Err(LaundryError::Conflict("the batch has nothing outstanding".into()));
    }
    let (counts, action, movements, data) = match &settle {
        Settle::Return { back_to, counts } => {
            let counted = checked_counts(tx, &lines, counts, false).await?;
            let to = back_to.unwrap_or(current.from_location_id);
            let movements = movements(&counted, MovementKind::Return, current.to_location_id, Some(to), None);
            (counted, "laundry_batch.settled", movements, json!({ "how": "return", "back_to_location_id": to }))
        }
        Settle::WriteOff { reason, counts } => {
            let reason = reason.trim();
            if reason.is_empty() || reason.chars().count() > MAX_TEXT_CHARS {
                return Err(LaundryError::Invalid(format!("a reason is 1 to {MAX_TEXT_CHARS} characters")));
            }
            let counted = checked_counts(tx, &lines, counts, false).await?;
            let movements = movements(&counted, MovementKind::WriteOff, current.to_location_id, None, Some(reason));
            (counted, "laundry_batch.settled", movements, json!({ "how": "write_off", "reason": reason }))
        }
    };
    stock::apply(tx, tenant, actor, property, Some(batch), &movements).await?;

    sqlx::query("update laundry_batch set version = version + 1 where id = $1").bind(batch).execute(&mut **tx).await?;
    let items: Vec<_> =
        counts.iter().map(|(item, quantity)| json!({ "item_id": item, "quantity": quantity })).collect();
    let mut data = data;
    data["items"] = items.into();
    audit(tx, tenant, actor, action, "laundry_batch", batch, data).await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    load(tx, property, batch).await
}

/// The property's batches, newest first (at most 200), only those in `status` when given.
pub async fn list_batches(
    tx: &mut Tx,
    property: Uuid,
    status: Option<BatchStatus>,
) -> Result<Vec<Batch>, LaundryError> {
    let rows: Vec<BatchRow> = sqlx::query_as(select_batch!(
        "where property_id = $1 and ($2::text is null or status = $2) order by id desc limit $3"
    ))
    .bind(property)
    .bind(status.map(BatchStatus::as_str))
    .bind(MAX_LISTED)
    .fetch_all(&mut **tx)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut lines = lines(tx, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let lines = lines.remove(&row.id).unwrap_or_default();
            row.with_lines(lines)
        })
        .collect())
}

/// Locks the batch row and checks its version.
async fn lock(tx: &mut Tx, property: Uuid, batch: Uuid, expected_version: i32) -> Result<BatchRow, LaundryError> {
    let current: BatchRow = sqlx::query_as(select_batch!("where id = $1 and property_id = $2 for update"))
        .bind(batch)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(LaundryError::NotFound("batch"))?;
    if current.version != expected_version {
        return Err(LaundryError::VersionMismatch("batch"));
    }
    Ok(current)
}

async fn load(tx: &mut Tx, property: Uuid, batch: Uuid) -> Result<Batch, LaundryError> {
    let row: BatchRow = sqlx::query_as(select_batch!("where id = $1 and property_id = $2"))
        .bind(batch)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(LaundryError::NotFound("batch"))?;
    let lines = lines(tx, &[batch]).await?.remove(&batch).unwrap_or_default();
    Ok(row.with_lines(lines))
}

/// Each batch's lines by item, worked out from the movements that carry the batch.
async fn lines(tx: &mut Tx, batches: &[Uuid]) -> Result<BTreeMap<Uuid, Vec<BatchLine>>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, i32, i32, i32)> = sqlx::query_as(
        "select batch_id, item_id,
           coalesce(sum(quantity) filter (where kind = 'send'), 0)::int,
           coalesce(sum(quantity) filter (where kind = 'return'), 0)::int,
           coalesce(sum(quantity) filter (where kind = 'write_off'), 0)::int
         from linen_movement where batch_id = any($1)
         group by batch_id, item_id order by batch_id, item_id",
    )
    .bind(batches)
    .fetch_all(&mut **tx)
    .await?;
    let mut by_batch: BTreeMap<Uuid, Vec<BatchLine>> = BTreeMap::new();
    for (batch, item_id, sent, returned, written_off) in rows {
        by_batch.entry(batch).or_default().push(BatchLine { item_id, sent, returned, written_off });
    }
    Ok(by_batch)
}

/// `counts` as a map: each item is one of the batch's, listed once, from 0 to its outstanding (`Invalid`). When
/// `allow_zero_total` is false at least one count must be above 0.
async fn checked_counts(
    tx: &mut Tx,
    lines: &[BatchLine],
    counts: &[(Uuid, i32)],
    allow_zero_total: bool,
) -> Result<BTreeMap<Uuid, i32>, LaundryError> {
    let mut counted = BTreeMap::new();
    for &(item, count) in counts {
        let line = lines
            .iter()
            .find(|line| line.item_id == item)
            .ok_or_else(|| LaundryError::Invalid("an item is not in this batch".into()))?;
        if counted.insert(item, count).is_some() {
            return Err(LaundryError::Invalid("an item is listed once".into()));
        }
        if count < 0 {
            return Err(LaundryError::Invalid("a count is 0 or more".into()));
        }
        if count > line.outstanding() {
            let code: String =
                sqlx::query_scalar("select code from linen_item where id = $1").bind(item).fetch_one(&mut **tx).await?;
            return Err(LaundryError::Invalid(format!("Only {} {code} are still out", line.outstanding())));
        }
    }
    if !allow_zero_total && counted.values().all(|count| *count == 0) {
        return Err(LaundryError::Invalid("count at least one item".into()));
    }
    Ok(counted)
}

/// One movement of `kind` per non-zero count, from `from` to `to`.
fn movements(
    counted: &BTreeMap<Uuid, i32>,
    kind: MovementKind,
    from: Uuid,
    to: Option<Uuid>,
    reason: Option<&str>,
) -> Vec<NewMovement> {
    counted
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(&item_id, &quantity)| NewMovement {
            item_id,
            kind,
            quantity,
            from_location_id: Some(from),
            to_location_id: to,
            reason: reason.map(str::to_owned),
        })
        .collect()
}
