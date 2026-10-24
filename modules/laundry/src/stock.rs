//! Linen movements, the stock they add up to, and stock below par.
//!
//! Every movement changes `linen_stock` in its own transaction, so a location's stock of an item always equals what
//! moved in minus what moved out. A movement locks, in order: its locations and item (`for share`, so a concurrent
//! switch-off waits), the stock rows (ascending `(location, item)`), then the property's linen alert.

use crate::{LaundryError, LocationKind, audit, linen_key, notify};
use audit::AlertKind;
use audit::alerts;
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The most movements one page lists.
const MAX_PAGE_SIZE: u32 = 100;
/// The longest the linen alert's text may be, in characters.
const MAX_ALERT_CHARS: usize = 2000;

db::text_enum!(
    /// What a movement does: stock bought in, moved between clean stores, sent out or back with a batch, or lost.
    MovementKind { Receive = "receive", Transfer = "transfer", Send = "send", Return = "return", WriteOff = "write_off" }
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMovement {
    pub item_id: Uuid,
    pub kind: MovementKind,
    pub quantity: i32,
    pub from_location_id: Option<Uuid>,
    pub to_location_id: Option<Uuid>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct Movement {
    pub id: Uuid,
    pub item_id: Uuid,
    #[sqlx(try_from = "String")]
    pub kind: MovementKind,
    pub quantity: i32,
    pub from_location_id: Option<Uuid>,
    pub to_location_id: Option<Uuid>,
    pub batch_id: Option<Uuid>,
    pub reason: Option<String>,
    pub business_date: Date,
    pub moved_by: Option<Uuid>,
    pub moved_at: OffsetDateTime,
}

/// One location's stock of one item, with the par its rooms need (`None` when it has none) and whether it is below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct StockCell {
    pub location_id: Uuid,
    pub item_id: Uuid,
    pub quantity: i32,
    pub par: Option<i32>,
    pub low: bool,
}

/// Records a `receive`, `transfer` or `write_off`; `send` and `return` only go with a batch (`Invalid`).
pub async fn record(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    movement: NewMovement,
) -> Result<Movement, LaundryError> {
    if matches!(movement.kind, MovementKind::Send | MovementKind::Return) {
        return Err(LaundryError::Invalid("send and return go with a laundry batch".into()));
    }
    let mut recorded = apply(tx, tenant, actor, property, None, &[movement]).await?;
    Ok(recorded.remove(0))
}

/// Applies `movements` in order, all or none, against `batch` when given: checks each against its locations and item,
/// refuses one that would take a location below zero (`Conflict`), updates the stock, records the movements, then
/// re-checks the property's linen alert. The alert's advisory lock is held until commit, so this must be the
/// transaction's last locking step.
pub(crate) async fn apply(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    batch: Option<Uuid>,
    movements: &[NewMovement],
) -> Result<Vec<Movement>, LaundryError> {
    let movements = movements.iter().map(|movement| clean(movement, batch)).collect::<Result<Vec<_>, _>>()?;
    if movements.is_empty() {
        return Ok(Vec::new());
    }

    let location_ids: Vec<Uuid> = movements
        .iter()
        .flat_map(|movement| [movement.from_location_id, movement.to_location_id])
        .flatten()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let locations: BTreeMap<Uuid, (String, LocationKind)> = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "select id, name, kind, active from linen_location where property_id = $1 and id = any($2)
         order by id for share",
    )
    .bind(property)
    .bind(&location_ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|(id, name, kind, active)| {
        if !active {
            return Err(LaundryError::Invalid(format!("{name} is switched off")));
        }
        let kind = LocationKind::parse(&kind).ok_or_else(|| LaundryError::Invalid(format!("unknown kind {kind}")))?;
        Ok((id, (name, kind)))
    })
    .collect::<Result<_, LaundryError>>()?;
    if locations.len() != location_ids.len() {
        return Err(LaundryError::NotFound("location"));
    }

    let item_ids: Vec<Uuid> =
        movements.iter().map(|movement| movement.item_id).collect::<BTreeSet<_>>().into_iter().collect();
    let items: BTreeMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String, bool)>(
        "select id, code, active from linen_item where property_id = $1 and id = any($2) order by id for share",
    )
    .bind(property)
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(
        |(id, code, active)| {
            if active { Ok((id, code)) } else { Err(LaundryError::Invalid(format!("{code} is switched off"))) }
        },
    )
    .collect::<Result<_, LaundryError>>()?;
    if items.len() != item_ids.len() {
        return Err(LaundryError::NotFound("item"));
    }

    for movement in &movements {
        check_locations(movement, batch, |id| locations[&id].1)?;
    }

    let mut stock = lock_stock(tx, tenant, property, &movements).await?;
    let before = stock.clone();
    for movement in &movements {
        if let Some(from) = movement.from_location_id {
            let held = stock.get_mut(&(from, movement.item_id)).expect("locked above");
            if *held < movement.quantity {
                return Err(LaundryError::Conflict(format!(
                    "Only {held} {} at {}",
                    items[&movement.item_id], locations[&from].0
                )));
            }
            *held -= movement.quantity;
        }
        if let Some(to) = movement.to_location_id {
            let held = stock.get_mut(&(to, movement.item_id)).expect("locked above");
            *held = held.checked_add(movement.quantity).ok_or_else(|| {
                LaundryError::Invalid(format!("too many {} at {}", items[&movement.item_id], locations[&to].0))
            })?;
        }
    }
    let changed: Vec<(&(Uuid, Uuid), &i32)> =
        stock.iter().filter(|(key, quantity)| before[*key] != **quantity).collect();
    let changed_locations: Vec<Uuid> = changed.iter().map(|((location, _), _)| *location).collect();
    let changed_items: Vec<Uuid> = changed.iter().map(|((_, item), _)| *item).collect();
    let changed_quantities: Vec<i32> = changed.iter().map(|(_, quantity)| **quantity).collect();
    sqlx::query(
        "update linen_stock s set quantity = c.quantity
         from unnest($1::uuid[], $2::uuid[], $3::int[]) as c(location_id, item_id, quantity)
         where s.location_id = c.location_id and s.item_id = c.item_id",
    )
    .bind(&changed_locations)
    .bind(&changed_items)
    .bind(&changed_quantities)
    .execute(&mut **tx)
    .await?;

    let business_date = folio::business_date(tx, property).await?;
    let mut recorded = Vec::with_capacity(movements.len());
    for movement in movements {
        let row: Movement = sqlx::query_as(
            "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, from_location_id,
               to_location_id, batch_id, reason, business_date, moved_by)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             returning id, item_id, kind, quantity, from_location_id, to_location_id, batch_id, reason, business_date,
               moved_by, moved_at",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(property)
        .bind(movement.item_id)
        .bind(movement.kind.as_str())
        .bind(movement.quantity)
        .bind(movement.from_location_id)
        .bind(movement.to_location_id)
        .bind(batch)
        .bind(&movement.reason)
        .bind(business_date)
        .bind(actor.0)
        .fetch_one(&mut **tx)
        .await?;
        audit(
            tx,
            tenant,
            actor,
            "linen_movement.recorded",
            "linen_movement",
            row.id,
            json!({
                "item_id": row.item_id,
                "kind": row.kind.as_str(),
                "quantity": row.quantity,
                "from_location_id": row.from_location_id,
                "to_location_id": row.to_location_id,
                "batch_id": row.batch_id,
                "reason": row.reason,
            }),
        )
        .await?;
        recorded.push(row);
    }

    recheck_alert(tx, tenant, property).await?;
    notify(tx, tenant, property, vec![linen_key(property)]).await?;
    Ok(recorded)
}

/// `movement` with its reason trimmed, once its quantity, ends, reason and batch suit its kind (`Invalid`).
fn clean(movement: &NewMovement, batch: Option<Uuid>) -> Result<NewMovement, LaundryError> {
    if !(1..=100_000).contains(&movement.quantity) {
        return Err(LaundryError::Invalid("a quantity is 1 to 100000".into()));
    }
    let reason = movement.reason.as_deref().map(str::trim).filter(|reason| !reason.is_empty()).map(str::to_owned);
    if reason.as_ref().is_some_and(|reason| reason.chars().count() > 500) {
        return Err(LaundryError::Invalid("a reason is at most 500 characters".into()));
    }
    let (from, to) = (movement.from_location_id, movement.to_location_id);
    match movement.kind {
        MovementKind::Receive if from.is_some() || to.is_none() => {
            return Err(LaundryError::Invalid("a receive goes to a location and comes from none".into()));
        }
        MovementKind::WriteOff if from.is_none() || to.is_some() => {
            return Err(LaundryError::Invalid("a write-off comes from a location and goes to none".into()));
        }
        MovementKind::WriteOff if reason.is_none() => {
            return Err(LaundryError::Invalid("a write-off needs a reason".into()));
        }
        MovementKind::Transfer | MovementKind::Send | MovementKind::Return if from.is_none() || to.is_none() => {
            return Err(LaundryError::Invalid("the movement needs a location to move from and one to move to".into()));
        }
        MovementKind::Send | MovementKind::Return if batch.is_none() => {
            return Err(LaundryError::Invalid("send and return go with a laundry batch".into()));
        }
        _ => {}
    }
    if from.is_some() && from == to {
        return Err(LaundryError::Invalid("a movement goes between two different locations".into()));
    }
    Ok(NewMovement { reason, ..movement.clone() })
}

/// Whether `movement`'s ends are the kinds of location its kind moves between (`Invalid` otherwise). A write-off from a
/// laundry or vendor goes against its batch, so every short batch stays settleable.
fn check_locations(
    movement: &NewMovement,
    batch: Option<Uuid>,
    kind_of: impl Fn(Uuid) -> LocationKind,
) -> Result<(), LaundryError> {
    let clean = |id: Option<Uuid>| {
        id.map(&kind_of).is_none_or(|kind| matches!(kind, LocationKind::Store | LocationKind::Pantry))
    };
    let out = |id: Option<Uuid>| {
        id.map(&kind_of).is_none_or(|kind| matches!(kind, LocationKind::Laundry | LocationKind::Vendor))
    };
    let (from, to) = (movement.from_location_id, movement.to_location_id);
    let fits = match movement.kind {
        MovementKind::Receive | MovementKind::Transfer => clean(from) && clean(to),
        MovementKind::Send => clean(from) && out(to),
        MovementKind::Return => out(from) && clean(to),
        MovementKind::WriteOff => batch.is_some() || clean(from),
    };
    if fits {
        Ok(())
    } else {
        let message = match movement.kind {
            MovementKind::Receive => "a receive goes to a store or pantry",
            MovementKind::Transfer => "a transfer moves between stores and pantries",
            MovementKind::Send => "a send goes from a store or pantry to a laundry or vendor",
            MovementKind::WriteOff => "Write it off against its batch",
            _ => "a return comes from a laundry or vendor to a store or pantry",
        };
        Err(LaundryError::Invalid(message.into()))
    }
}

/// Creates any missing stock rows of `movements`' `(location, item)` pairs and locks them all, in ascending order.
async fn lock_stock(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    movements: &[NewMovement],
) -> Result<BTreeMap<(Uuid, Uuid), i32>, sqlx::Error> {
    let pairs: BTreeSet<(Uuid, Uuid)> = movements
        .iter()
        .flat_map(|movement| {
            [movement.from_location_id, movement.to_location_id]
                .into_iter()
                .flatten()
                .map(|location| (location, movement.item_id))
        })
        .collect();
    let (locations, items): (Vec<Uuid>, Vec<Uuid>) = pairs.iter().copied().unzip();
    // Inserted in ascending order too, so two movements creating the same rows wait on each other in one order.
    sqlx::query(
        "insert into linen_stock (tenant_id, property_id, location_id, item_id, quantity)
         select $1, $2, location_id, item_id, 0 from unnest($3::uuid[], $4::uuid[]) as p(location_id, item_id)
         order by location_id, item_id
         on conflict do nothing",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(&locations)
    .bind(&items)
    .execute(&mut **tx)
    .await?;
    let rows: Vec<(Uuid, Uuid, i32)> = sqlx::query_as(
        "select s.location_id, s.item_id, s.quantity from linen_stock s
         join unnest($1::uuid[], $2::uuid[]) as p(location_id, item_id)
           on s.location_id = p.location_id and s.item_id = p.item_id
         order by s.location_id, s.item_id for update of s",
    )
    .bind(&locations)
    .bind(&items)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(location, item, quantity)| ((location, item), quantity)).collect())
}

/// Raises the property's linen alert listing every cell below par, or clears it when none is. Concurrent movements
/// take turns here, so each one reads the stock the one before it committed and the last alert written is current.
async fn recheck_alert(tx: &mut Tx, tenant: TenantId, property: Uuid) -> Result<(), LaundryError> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('linen:' || $1::text, 0))")
        .bind(property)
        .execute(&mut **tx)
        .await?;
    let low: Vec<String> = cells(tx, property)
        .await?
        .into_iter()
        .filter_map(|cell| {
            let par = cell.par.filter(|par| cell.quantity < *par)?;
            Some(format!("{}: {} {} of {par}", cell.location, cell.code, cell.quantity))
        })
        .collect();
    if low.is_empty() {
        alerts::clear(tx, property, AlertKind::Linen).await?;
    } else {
        let message = low.join("; ");
        let message = if message.chars().count() > MAX_ALERT_CHARS {
            message.chars().take(MAX_ALERT_CHARS - 1).chain(['…']).collect()
        } else {
            message
        };
        alerts::raise(tx, tenant, property, AlertKind::Linen, &message).await?;
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct CellRow {
    location_id: Uuid,
    location: String,
    item_id: Uuid,
    code: String,
    quantity: i32,
    par: Option<i32>,
}

/// Every active location × active item of the property, by location name then item code. A pantry's par is the sum,
/// over the active rooms of its section, of their room type's par; the store's covers the active rooms in no active
/// pantry's section. Laundries and vendors have none.
async fn cells(tx: &mut Tx, property: Uuid) -> Result<Vec<CellRow>, sqlx::Error> {
    sqlx::query_as(
        "with location as (
           select id, name, kind, section_id from linen_location where property_id = $1 and active
         ), par as (
           select l.id as location_id, p.item_id, sum(p.quantity)::int as quantity
           from location l
           join room r on r.property_id = $1 and r.active and (
             (l.kind = 'pantry' and r.section_id = l.section_id)
             or (l.kind = 'store' and (r.section_id is null or r.section_id not in (
               select section_id from location where kind = 'pantry'))))
           join linen_par p on p.room_type_id = r.room_type_id
           group by l.id, p.item_id
         )
         select l.id as location_id, l.name as location, i.id as item_id, i.code, coalesce(s.quantity, 0) as quantity,
           par.quantity as par
         from location l
         cross join linen_item i
         left join linen_stock s on s.location_id = l.id and s.item_id = i.id
         left join par on par.location_id = l.id and par.item_id = i.id
         where i.property_id = $1 and i.active
         order by l.name, i.code",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// Every active location's stock of every active item, with its par and whether it is below.
pub async fn stock(tx: &mut Tx, property: Uuid) -> Result<Vec<StockCell>, LaundryError> {
    Ok(cells(tx, property)
        .await?
        .into_iter()
        .map(|cell| StockCell {
            location_id: cell.location_id,
            item_id: cell.item_id,
            quantity: cell.quantity,
            par: cell.par,
            low: cell.par.is_some_and(|par| cell.quantity < par),
        })
        .collect())
}

/// The property's movements, newest first: `first` of them (1 to 100, `Invalid` otherwise) older than the movement
/// `after`, or the newest when `after` is `None`; only those from or to `location` when given.
pub async fn movements(
    tx: &mut Tx,
    property: Uuid,
    first: u32,
    after: Option<Uuid>,
    location: Option<Uuid>,
) -> Result<Vec<Movement>, LaundryError> {
    if !(1..=MAX_PAGE_SIZE).contains(&first) {
        return Err(LaundryError::Invalid(format!("first is 1 to {MAX_PAGE_SIZE}")));
    }
    Ok(sqlx::query_as(
        "select id, item_id, kind, quantity, from_location_id, to_location_id, batch_id, reason, business_date,
           moved_by, moved_at
         from linen_movement
         where property_id = $1 and ($2::uuid is null or id < $2)
           and ($3::uuid is null or from_location_id = $3 or to_location_id = $3)
         order by id desc limit $4",
    )
    .bind(property)
    .bind(after)
    .bind(location)
    .bind(i64::from(first))
    .fetch_all(&mut **tx)
    .await?)
}
