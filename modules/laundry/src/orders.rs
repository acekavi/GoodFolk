//! Guest laundry orders: items collected from an in-house stay, priced from the price list, and charged to the
//! guest's bill on delivery.
//!
//! A delivery locks the order row first, then reads the business date `for share`, then posts each line through
//! `folio::post_for_stay`, which takes its own folio locks; it never locks the `reservation_room` row (the folio lock
//! order puts it before the folios).

use crate::prices::Service;
use crate::{LaundryError, audit, laundry_key, notify};
use db::{TenantId, Tx, UserId};
use folio::{NewCharge, Source};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;
use uuid::Uuid;

/// The most lines one order may have.
const MAX_LINES: usize = 50;
/// The most of one line's item.
const MAX_QUANTITY: i32 = 100;
/// The most orders one listing returns.
const MAX_LISTED: i64 = 500;
/// The longest an order note may be, in characters.
const MAX_NOTE_CHARS: usize = 500;

db::text_enum!(
    /// Where an order is. It moves one step at a time to `delivered`, or to `cancelled` from any step before it.
    OrderStatus {
        Collected = "collected",
        Processing = "processing",
        Ready = "ready",
        Delivered = "delivered",
        Cancelled = "cancelled"
    }
);

impl OrderStatus {
    /// The step after this one, if any.
    fn next(self) -> Option<OrderStatus> {
        match self {
            OrderStatus::Collected => Some(OrderStatus::Processing),
            OrderStatus::Processing => Some(OrderStatus::Ready),
            OrderStatus::Ready => Some(OrderStatus::Delivered),
            OrderStatus::Delivered | OrderStatus::Cancelled => None,
        }
    }

    fn is_open(self) -> bool {
        self.next().is_some()
    }
}

/// One line of an order, with the price row's code, service, name and price as they were when it was taken.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct OrderLine {
    pub position: i32,
    pub price_id: Uuid,
    pub item_code: String,
    #[sqlx(try_from = "String")]
    pub service: Service,
    pub name: String,
    /// Net of tax, in minor units of the order's currency.
    pub unit_price: i64,
    pub quantity: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Order {
    pub id: Uuid,
    pub reservation_room_id: Uuid,
    /// The stay's room when the order was taken.
    pub room_number: String,
    /// The stay's primary guest.
    pub guest: String,
    pub status: OrderStatus,
    pub note: Option<String>,
    pub lines: Vec<OrderLine>,
    /// The lines' unit prices times their quantities, net of tax.
    pub total: i64,
    /// The property's base currency.
    pub currency: String,
    pub created_at: OffsetDateTime,
    pub status_at: OffsetDateTime,
    pub delivered_at: Option<OffsetDateTime>,
    pub version: i32,
}

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: Uuid,
    reservation_room_id: Uuid,
    room_number: String,
    guest: String,
    #[sqlx(try_from = "String")]
    status: OrderStatus,
    note: Option<String>,
    currency: String,
    created_at: OffsetDateTime,
    status_at: OffsetDateTime,
    delivered_at: Option<OffsetDateTime>,
    version: i32,
}

/// `select` of an order's columns from `laundry_order o`, followed by `$rest`.
macro_rules! select_order {
    ($rest:literal) => {
        concat!(
            "select o.id, o.reservation_room_id, o.room_number, btrim(g.first_name || ' ' || g.last_name) as guest, ",
            "o.status, o.note, p.base_currency as currency, o.created_at, o.status_at, o.delivered_at, o.version ",
            "from laundry_order o ",
            "join property p on p.id = o.property_id ",
            "join reservation_room rr on rr.id = o.reservation_room_id ",
            "join guest g on g.id = rr.primary_guest_id ",
            $rest
        )
    };
}

impl OrderRow {
    fn with_lines(self, lines: Vec<OrderLine>) -> Order {
        let total = lines.iter().map(|line| line.unit_price * i64::from(line.quantity)).sum();
        Order {
            id: self.id,
            reservation_room_id: self.reservation_room_id,
            room_number: self.room_number,
            guest: self.guest,
            status: self.status,
            note: self.note,
            lines,
            total,
            currency: self.currency,
            created_at: self.created_at,
            status_at: self.status_at,
            delivered_at: self.delivered_at,
            version: self.version,
        }
    }
}

/// Takes an order for the stay `reservation_room`, which must be checked in at the property. `lines` are 1 to 50
/// `(price, quantity)` pairs, each an active price of the property (a price may repeat) and a quantity of 1 to 100.
#[allow(clippy::too_many_arguments)]
pub async fn create_order(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    reservation_room: Uuid,
    lines: &[(Uuid, i32)],
    note: Option<String>,
) -> Result<Order, LaundryError> {
    let note = clean_note(note)?;
    let priced = priced_lines(tx, property, lines).await?;
    let room_number: Option<String> = sqlx::query_scalar(
        "select room.number from reservation_room rr join room on room.id = rr.room_id
         where rr.id = $1 and rr.property_id = $2 and rr.status = 'checked_in'",
    )
    .bind(reservation_room)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let room_number =
        room_number.ok_or_else(|| LaundryError::Invalid("Laundry is taken for an in-house stay".into()))?;

    let id = Uuid::now_v7();
    sqlx::query(
        "insert into laundry_order (id, tenant_id, property_id, reservation_room_id, room_number, status, note,
                                    created_by, status_by)
         values ($1, $2, $3, $4, $5, 'collected', $6, $7, $7)",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(reservation_room)
    .bind(&room_number)
    .bind(&note)
    .bind(actor.0)
    .execute(&mut **tx)
    .await?;
    insert_lines(tx, tenant, property, id, &priced).await?;
    audit(
        tx,
        tenant,
        actor,
        "laundry_order.created",
        "laundry_order",
        id,
        json!({ "reservation_room_id": reservation_room, "room_number": room_number, "lines": lines_json(&priced) }),
    )
    .await?;
    notify(tx, tenant, property, vec![laundry_key(property)]).await?;
    load(tx, property, id).await
}

/// Replaces a `collected` order's lines (as for [`create_order`]), and its note when `note` is given (`Some(None)`
/// clears it).
#[allow(clippy::too_many_arguments)]
pub async fn update_lines(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    lines: &[(Uuid, i32)],
    note: Option<Option<String>>,
) -> Result<Order, LaundryError> {
    let current = lock(tx, property, id, expected_version).await?;
    if current.status != OrderStatus::Collected {
        return Err(LaundryError::Conflict(format!(
            "The order is {}; its lines can only be changed while it is collected",
            current.status.as_str()
        )));
    }
    let note = match note {
        Some(note) => clean_note(note)?,
        None => current.note,
    };
    let priced = priced_lines(tx, property, lines).await?;
    sqlx::query("delete from laundry_order_line where order_id = $1").bind(id).execute(&mut **tx).await?;
    insert_lines(tx, tenant, property, id, &priced).await?;
    sqlx::query("update laundry_order set note = $2, version = version + 1 where id = $1")
        .bind(id)
        .bind(&note)
        .execute(&mut **tx)
        .await?;
    audit(
        tx,
        tenant,
        actor,
        "laundry_order.updated",
        "laundry_order",
        id,
        json!({ "note": note, "lines": lines_json(&priced) }),
    )
    .await?;
    notify(tx, tenant, property, vec![laundry_key(property)]).await?;
    load(tx, property, id).await
}

/// Moves the order to `to`: the next step, or `cancelled` from any step before `delivered` (`Conflict` otherwise).
/// Delivery posts each line, in position order, to the stay's bill through `folio::post_for_stay` (so routing
/// applies) under the posting key `laundry:<order>:<position>`; the stay must still be checked in, and a posting the
/// folio refuses refuses the delivery.
#[allow(clippy::too_many_arguments)]
pub async fn set_status(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    to: OrderStatus,
) -> Result<Order, LaundryError> {
    let current = lock(tx, property, id, expected_version).await?;
    let from = current.status;
    let allowed = from.next() == Some(to) || (to == OrderStatus::Cancelled && from.is_open());
    if !allowed {
        return Err(LaundryError::Conflict(format!("An order can't move from {} to {}", from.as_str(), to.as_str())));
    }

    let mut keys = BTreeSet::from([laundry_key(property)]);
    if to == OrderStatus::Delivered {
        let in_house: bool = sqlx::query_scalar(
            "select exists (select 1 from reservation_room
                            where id = $1 and property_id = $2 and status = 'checked_in')",
        )
        .bind(current.reservation_room_id)
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
        if !in_house {
            return Err(LaundryError::Conflict("The guest has checked out; post the charge on their folio".into()));
        }
        let business_date = folio::business_date(tx, property).await?;
        for line in order_lines(tx, &[id]).await?.remove(&id).unwrap_or_default() {
            let charge = NewCharge {
                source: Source::Laundry,
                category: "laundry".into(),
                description: format!("Laundry: {} × {}", line.name, line.quantity),
                quantity: line.quantity,
                unit_net: line.unit_price,
                currency: current.currency.clone(),
                business_date,
                posting_key: Some(format!("laundry:{id}:{}", line.position)),
                inclusive: false,
            };
            let posted = folio::post_for_stay(tx, tenant, Some(actor), current.reservation_room_id, charge).await?;
            keys.insert(folio::folio_key(posted.folio_id));
        }
    }

    sqlx::query(
        "update laundry_order
         set status = $2, status_by = $3, status_at = now(),
             delivered_at = case when $2 = 'delivered' then now() end, version = version + 1
         where id = $1",
    )
    .bind(id)
    .bind(to.as_str())
    .bind(actor.0)
    .execute(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "laundry_order.status_changed",
        "laundry_order",
        id,
        json!({ "from": from.as_str(), "to": to.as_str() }),
    )
    .await?;
    notify(tx, tenant, property, keys.into_iter().collect()).await?;
    load(tx, property, id).await
}

/// The property's orders, open ones first, each group newest first (at most 500), only those in `status` when given.
pub async fn list_orders(tx: &mut Tx, property: Uuid, status: Option<OrderStatus>) -> Result<Vec<Order>, LaundryError> {
    let rows: Vec<OrderRow> = sqlx::query_as(select_order!(
        "where o.property_id = $1 and ($2::text is null or o.status = $2)
         order by o.status in ('delivered', 'cancelled'), o.id desc limit $3"
    ))
    .bind(property)
    .bind(status.map(OrderStatus::as_str))
    .bind(MAX_LISTED)
    .fetch_all(&mut **tx)
    .await?;
    with_lines(tx, rows).await
}

/// The orders of the stay `reservation_room` in the property, newest first.
pub async fn orders_for_stay(tx: &mut Tx, property: Uuid, reservation_room: Uuid) -> Result<Vec<Order>, LaundryError> {
    let rows: Vec<OrderRow> =
        sqlx::query_as(select_order!("where o.property_id = $1 and o.reservation_room_id = $2 order by o.id desc"))
            .bind(property)
            .bind(reservation_room)
            .fetch_all(&mut **tx)
            .await?;
    with_lines(tx, rows).await
}

/// The trimmed note, `None` when blank, at most 500 characters (`Invalid`).
fn clean_note(note: Option<String>) -> Result<Option<String>, LaundryError> {
    let note = note.as_deref().map(str::trim).filter(|note| !note.is_empty()).map(str::to_owned);
    if note.as_ref().is_some_and(|note| note.chars().count() > MAX_NOTE_CHARS) {
        return Err(LaundryError::Invalid(format!("a note is at most {MAX_NOTE_CHARS} characters")));
    }
    Ok(note)
}

/// `lines` as order lines in their order, priced from the property's active prices (`Invalid` for a count out of
/// range or a price that is not active in the property).
async fn priced_lines(tx: &mut Tx, property: Uuid, lines: &[(Uuid, i32)]) -> Result<Vec<OrderLine>, LaundryError> {
    if lines.is_empty() || lines.len() > MAX_LINES {
        return Err(LaundryError::Invalid(format!("an order has 1 to {MAX_LINES} lines")));
    }
    if lines.iter().any(|(_, quantity)| !(1..=MAX_QUANTITY).contains(quantity)) {
        return Err(LaundryError::Invalid(format!("a quantity is 1 to {MAX_QUANTITY}")));
    }
    let ids: Vec<Uuid> = lines.iter().map(|(price, _)| *price).collect();
    let prices: Vec<(Uuid, String, String, String, i64)> = sqlx::query_as(
        "select id, item_code, service, name, price from laundry_price
         where property_id = $1 and id = any($2) and active",
    )
    .bind(property)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let prices: BTreeMap<Uuid, _> =
        prices.into_iter().map(|(id, code, service, name, price)| (id, (code, service, name, price))).collect();
    lines
        .iter()
        .zip(1..)
        .map(|(&(price_id, quantity), position)| {
            let (item_code, service, name, unit_price) = prices
                .get(&price_id)
                .ok_or_else(|| LaundryError::Invalid("a price is not on the property's active price list".into()))?;
            let service = Service::try_from(service.clone()).map_err(LaundryError::Invalid)?;
            Ok(OrderLine {
                position,
                price_id,
                item_code: item_code.clone(),
                service,
                name: name.clone(),
                unit_price: *unit_price,
                quantity,
            })
        })
        .collect()
}

async fn insert_lines(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    order: Uuid,
    lines: &[OrderLine],
) -> Result<(), sqlx::Error> {
    let positions: Vec<i32> = lines.iter().map(|line| line.position).collect();
    let prices: Vec<Uuid> = lines.iter().map(|line| line.price_id).collect();
    let codes: Vec<&str> = lines.iter().map(|line| line.item_code.as_str()).collect();
    let services: Vec<&str> = lines.iter().map(|line| line.service.as_str()).collect();
    let names: Vec<&str> = lines.iter().map(|line| line.name.as_str()).collect();
    let unit_prices: Vec<i64> = lines.iter().map(|line| line.unit_price).collect();
    let quantities: Vec<i32> = lines.iter().map(|line| line.quantity).collect();
    sqlx::query(
        "insert into laundry_order_line (tenant_id, property_id, order_id, position, price_id, item_code, service, name,
                                         unit_price, quantity)
         select $1, $2, $3, l.* from unnest($4::int[], $5::uuid[], $6::text[], $7::text[], $8::text[], $9::bigint[],
                                            $10::int[]) as l",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(order)
    .bind(positions)
    .bind(prices)
    .bind(codes)
    .bind(services)
    .bind(names)
    .bind(unit_prices)
    .bind(quantities)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn lines_json(lines: &[OrderLine]) -> serde_json::Value {
    lines
        .iter()
        .map(|line| {
            json!({
                "price_id": line.price_id, "item_code": line.item_code, "service": line.service.as_str(),
                "unit_price": line.unit_price, "quantity": line.quantity,
            })
        })
        .collect()
}

/// Locks the order row and checks its version.
async fn lock(tx: &mut Tx, property: Uuid, id: Uuid, expected_version: i32) -> Result<OrderRow, LaundryError> {
    let current: OrderRow = sqlx::query_as(select_order!("where o.id = $1 and o.property_id = $2 for update of o"))
        .bind(id)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(LaundryError::NotFound("order"))?;
    if current.version != expected_version {
        return Err(LaundryError::VersionMismatch("order"));
    }
    Ok(current)
}

async fn load(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Order, LaundryError> {
    let row: OrderRow = sqlx::query_as(select_order!("where o.id = $1 and o.property_id = $2"))
        .bind(id)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(LaundryError::NotFound("order"))?;
    let lines = order_lines(tx, &[id]).await?.remove(&id).unwrap_or_default();
    Ok(row.with_lines(lines))
}

async fn with_lines(tx: &mut Tx, rows: Vec<OrderRow>) -> Result<Vec<Order>, LaundryError> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut lines = order_lines(tx, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let lines = lines.remove(&row.id).unwrap_or_default();
            row.with_lines(lines)
        })
        .collect())
}

/// Each order's lines in position order.
async fn order_lines(tx: &mut Tx, orders: &[Uuid]) -> Result<BTreeMap<Uuid, Vec<OrderLine>>, sqlx::Error> {
    let rows: Vec<(Uuid, OrderLine)> = sqlx::query_as::<_, OrderLineRow>(
        "select order_id, position, price_id, item_code, service, name, unit_price, quantity
         from laundry_order_line where order_id = any($1) order by order_id, position",
    )
    .bind(orders)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|row| (row.order_id, row.line))
    .collect();
    let mut by_order: BTreeMap<Uuid, Vec<OrderLine>> = BTreeMap::new();
    for (order, line) in rows {
        by_order.entry(order).or_default().push(line);
    }
    Ok(by_order)
}

#[derive(sqlx::FromRow)]
struct OrderLineRow {
    order_id: Uuid,
    #[sqlx(flatten)]
    line: OrderLine,
}
