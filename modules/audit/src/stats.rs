//! A business date's statistics: what the night audit snapshots in the roll's transaction, and reads back by range.
//!
//! Every money figure is in the property's base currency, net of tax, on the posting business date: a correction
//! counts on the day it is posted, never on the day it corrects. Lines with source `adjustment` move money between
//! ledgers and are not revenue.

use crate::AuditError;
use crate::review::SETTLED_PAYMENT;
use db::{TenantId, Tx};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::types::Json;
use std::collections::{BTreeMap, HashMap, HashSet};
use time::Date;
use uuid::Uuid;

/// The most days [`daily_stats`] reads at once.
const MAX_RANGE_DAYS: i64 = 366;

/// The key of the revenue no stay accounts for.
const UNATTRIBUTED: &str = "unattributed";

/// The furthest a line's attribution follows reversals and transfers back to a posting key.
const MAX_CHAIN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DailyStats {
    pub business_date: Date,
    pub rooms_total: i32,
    pub rooms_out_of_order: i32,
    pub rooms_available: i32,
    pub rooms_sold: i32,
    pub complimentary_rooms: i32,
    pub occupancy_bp: i32,
    pub adults: i32,
    pub children: i32,
    pub arrivals: i32,
    pub departures: i32,
    pub no_shows: i32,
    pub cancellations: i32,
    pub walk_ins: i32,
    pub room_revenue: i64,
    pub meal_plan_revenue: i64,
    pub pos_revenue: i64,
    pub fee_revenue: i64,
    pub other_revenue: i64,
    pub adr: i64,
    pub revpar: i64,
    pub taxes: BTreeMap<String, i64>,
    pub payments: BTreeMap<String, i64>,
    /// By dimension, then key (byte order).
    pub breakdown: Vec<BreakdownRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BreakdownRow {
    pub dimension: String,
    pub key: String,
    pub room_nights: i32,
    pub room_revenue: i64,
    pub revenue: i64,
}

/// D's folio lines on the property's folios (`$1`, `$2`), adjustments aside, with their taxes. Public so a test can
/// `EXPLAIN` it: `business_date` is a leakproof comparison, so `folio_line_business_date_idx` serves it under RLS.
pub const DAY_LINES_SQL: &str = "select l.id, l.category, l.currency::text as currency, l.base_total, l.rate,
        l.posting_key, l.reverses, l.transfer_id,
        coalesce(t.amounts, '{}') as tax_amounts, coalesce(t.codes, '{}') as tax_codes,
        coalesce(t.on_bill, '{}') as tax_on_bill
 from folio_line l
 join folio f on f.id = l.folio_id
 left join lateral (select array_agg(code order by code) as codes, array_agg(amount order by code) as amounts,
                           array_agg(presentation = 'on_bill' order by code) as on_bill
                    from folio_line_tax where line_id = l.id) t on true
 where l.business_date = $2 and f.property_id = $1 and l.source <> 'adjustment'";

/// Computes D's statistics (no writes). Called inside the roll's transaction. `NotFound` for a property not in the
/// tenant.
///
/// - **Rooms:** active rooms; those with an unreleased `out_of_order` block covering D; available is the difference.
/// - **Sold:** the stays [`folio::in_house_stays`] returns for D, the complimentary ones among them (D's night has a
///   room amount of 0), and their adults and children.
/// - **Counts:** arrivals (checked in on D), departures (checked out, leaving D), no-shows and cancellations (on D),
///   and walk-ins (arrivals booked on D).
/// - **Revenue:** each of D's lines, net (its base total less its on-bill taxes, each converted at the line's rate),
///   by category: `room`, `meal_plan`, `pos:*`, `fee`, and the rest as other. The items of the POS bills settled at
///   their outlet with a payment dated D (a late bill's own date may be earlier) count the same way by their own
///   category, converted at their payment's rate; a settled bill with no payment is `Invalid`.
/// - **Taxes** by code, on-bill and absorbed; **payments** by method in base, refunds in, transfers out.
/// - **Breakdown:** per stay dimension (`segment`, `source`, `residency`, `room_type`), the sold stays and the room
///   and meal-plan revenue of the stay each line's posting key names (through a reversal to the line it reverses, and
///   through a transfer to the line its sibling reversal reverses), the rest under `unattributed`; per `outlet`, POS
///   revenue.
pub async fn compute(tx: &mut Tx, property: Uuid, date: Date) -> Result<DailyStats, AuditError> {
    let base: String = sqlx::query_scalar("select base_currency::text from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AuditError::NotFound("property"))?;
    let base_exponent = fx::exponent(&base);
    let convert = |amount: i64, currency: &str, rate: Decimal| -> Result<i64, AuditError> {
        Ok(fx::to_base(amount, fx::exponent(currency), base_exponent, rate)?)
    };

    let (rooms_total, rooms_out_of_order): (i32, i32) = sqlx::query_as(
        "select count(*)::int,
                (count(*) filter (where exists (
                   select 1 from room_block b
                   where b.room_id = room.id and b.kind = 'out_of_order' and b.released_at is null
                     and b.starts <= $2 and $2 < b.starts + b.days)))::int
         from room where property_id = $1 and active",
    )
    .bind(property)
    .bind(date)
    .fetch_one(&mut **tx)
    .await?;
    let counts: Counts = sqlx::query_as(
        "select (count(*) filter (where rr.checked_in_business_date = $2))::int as arrivals,
                (count(*) filter (where rr.status = 'checked_out' and rr.arrival + rr.nights = $2))::int as departures,
                (count(*) filter (where rr.status = 'no_show' and rr.cancelled_business_date = $2))::int as no_shows,
                (count(*) filter (where rr.status = 'cancelled' and rr.cancelled_business_date = $2))::int
                  as cancellations,
                (count(*) filter (where rr.checked_in_business_date = $2 and r.booked_business_date = $2))::int
                  as walk_ins
         from reservation_room rr
         join reservation r on r.id = rr.reservation_id
         where rr.property_id = $1",
    )
    .bind(property)
    .bind(date)
    .fetch_one(&mut **tx)
    .await?;
    let sold = folio::in_house_stays(tx, property, date).await?;

    let lines: Vec<Line> = sqlx::query_as(DAY_LINES_SQL).bind(property).bind(date).fetch_all(&mut **tx).await?;
    let origins = attribute(tx, &lines).await?;

    let mut stay_ids: Vec<Uuid> = sold.clone();
    stay_ids.extend(origins.values().flatten());
    stay_ids.sort_unstable();
    stay_ids.dedup();
    let stays: HashMap<Uuid, StayRow> = sqlx::query_as::<_, StayRow>(
        "select rr.id, rr.adults, rr.children, rp.segment, r.source, g.residency, rt.code as room_type,
                coalesce(n.room_amount = 0, false) as complimentary
         from reservation_room rr
         join reservation r on r.id = rr.reservation_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         join guest g on g.id = rr.primary_guest_id
         join room_type rt on rt.id = rr.room_type_id
         left join reservation_night n on n.reservation_room_id = rr.id and n.date = $3
         where rr.property_id = $1 and rr.id = any($2)",
    )
    .bind(property)
    .bind(&stay_ids)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|stay| (stay.id, stay))
    .collect();

    let mut stats = DailyStats {
        business_date: date,
        rooms_total,
        rooms_out_of_order,
        rooms_available: rooms_total - rooms_out_of_order,
        rooms_sold: 0,
        complimentary_rooms: 0,
        occupancy_bp: 0,
        adults: 0,
        children: 0,
        arrivals: counts.arrivals,
        departures: counts.departures,
        no_shows: counts.no_shows,
        cancellations: counts.cancellations,
        walk_ins: counts.walk_ins,
        room_revenue: 0,
        meal_plan_revenue: 0,
        pos_revenue: 0,
        fee_revenue: 0,
        other_revenue: 0,
        adr: 0,
        revpar: 0,
        taxes: BTreeMap::new(),
        payments: BTreeMap::new(),
        breakdown: Vec::new(),
    };
    // (dimension, key) → (room nights, room revenue, revenue)
    let mut breakdown: BTreeMap<(&str, String), (i32, i64, i64)> = BTreeMap::new();

    for stay in sold.iter().filter_map(|id| stays.get(id)) {
        stats.rooms_sold += 1;
        stats.complimentary_rooms += i32::from(stay.complimentary);
        stats.adults += stay.adults;
        stats.children += stay.children;
        for (dimension, key) in stay.dimensions() {
            breakdown.entry((dimension, key.to_owned())).or_default().0 += 1;
        }
    }

    for line in &lines {
        let mut on_bill = 0;
        for ((code, &amount), &is_on_bill) in line.tax_codes.iter().zip(&line.tax_amounts).zip(&line.tax_on_bill) {
            let amount = convert(amount, &line.currency, line.rate)?;
            *stats.taxes.entry(code.clone()).or_default() += amount;
            if is_on_bill {
                on_bill += amount;
            }
        }
        let net = line.base_total - on_bill;
        let category = line.category.as_str();
        match category {
            "room" | "meal_plan" => {
                let room = if category == "room" { net } else { 0 };
                stats.room_revenue += room;
                stats.meal_plan_revenue += net - room;
                let stay = origins.get(&line.id).copied().flatten().and_then(|id| stays.get(&id));
                let dimensions = match stay {
                    Some(stay) => stay.dimensions(),
                    None => STAY_DIMENSIONS.map(|dimension| (dimension, UNATTRIBUTED)),
                };
                for (dimension, key) in dimensions {
                    let entry = breakdown.entry((dimension, key.to_owned())).or_default();
                    entry.1 += room;
                    entry.2 += net;
                }
            }
            "fee" => stats.fee_revenue += net,
            _ => match category.strip_prefix("pos:") {
                Some(outlet) => {
                    stats.pos_revenue += net;
                    breakdown.entry(("outlet", outlet.to_owned())).or_default().2 += net;
                }
                None => stats.other_revenue += net,
            },
        }
    }

    // The bills settled on D: by their payment's date, so a late bill counts on the day it posted. A bill dated D
    // with no payment at all is read too, to be refused: every settled bill has one.
    let settled: Vec<SettledBill> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select b.bill_no, b.lines, paid.currency, paid.rate
         from pos_bill b
         left join lateral ({SETTLED_PAYMENT}) paid on true
         where b.property_id = $1 and b.settled
           and (b.id in (select pos_bill_id from payment
                         where business_date = $2 and pos_bill_id is not null and refund_of is null)
                or b.business_date = $2)
           and (paid.business_date = $2 or paid.business_date is null)"
    )))
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    for bill in &settled {
        let (Some(currency), Some(rate)) = (&bill.currency, bill.rate) else {
            return Err(AuditError::Invalid(format!("POS bill {} is settled but has no payment", bill.bill_no)));
        };
        for line in &bill.lines.0 {
            let mut net = convert(line.total, currency, rate)?;
            for tax in &line.taxes {
                let amount = convert(tax.amount, currency, rate)?;
                *stats.taxes.entry(tax.code.clone()).or_default() += amount;
                if tax.presentation == "on_bill" {
                    net -= amount;
                }
            }
            // By the item's own category, as a posted bill's line is: a misc item is other revenue, at no outlet.
            match line.category.strip_prefix("pos:") {
                Some(outlet) => {
                    stats.pos_revenue += net;
                    breakdown.entry(("outlet", outlet.to_owned())).or_default().2 += net;
                }
                None => stats.other_revenue += net,
            }
        }
    }

    let payments: Vec<(String, i64)> = sqlx::query_as(
        "select p.method, sum(p.base_amount)::bigint
         from payment p
         left join folio f on f.id = p.folio_id
         left join pos_bill b on b.id = p.pos_bill_id
         where p.business_date = $2 and p.method <> 'transfer'
           and (f.property_id = $1 or (p.folio_id is null and b.property_id = $1))
         group by p.method",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    stats.payments = payments.into_iter().collect();

    stats.occupancy_bp =
        i32::try_from(div_half_up(i64::from(stats.rooms_sold) * 10_000, i64::from(stats.rooms_available)))
            .unwrap_or(i32::MAX);
    stats.adr = div_half_up(stats.room_revenue, i64::from(stats.rooms_sold));
    stats.revpar = div_half_up(stats.room_revenue, i64::from(stats.rooms_available));
    stats.breakdown = breakdown
        .into_iter()
        .map(|((dimension, key), (room_nights, room_revenue, revenue))| BreakdownRow {
            dimension: dimension.to_owned(),
            key,
            room_nights,
            room_revenue,
            revenue,
        })
        .collect();
    Ok(stats)
}

/// Inserts `stats` (row + breakdown). A second insert for the date is a unique violation — the caller's
/// step row makes that unreachable.
pub async fn store(tx: &mut Tx, tenant: TenantId, property: Uuid, stats: &DailyStats) -> Result<(), AuditError> {
    sqlx::query(
        "insert into daily_stats (tenant_id, property_id, business_date, rooms_total, rooms_out_of_order,
                                  rooms_available, rooms_sold, complimentary_rooms, occupancy_bp, adults, children,
                                  arrivals, departures, no_shows, cancellations, walk_ins, room_revenue,
                                  meal_plan_revenue, pos_revenue, fee_revenue, other_revenue, adr, revpar, taxes,
                                  payments)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22,
                 $23, $24, $25)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(stats.business_date)
    .bind(stats.rooms_total)
    .bind(stats.rooms_out_of_order)
    .bind(stats.rooms_available)
    .bind(stats.rooms_sold)
    .bind(stats.complimentary_rooms)
    .bind(stats.occupancy_bp)
    .bind(stats.adults)
    .bind(stats.children)
    .bind(stats.arrivals)
    .bind(stats.departures)
    .bind(stats.no_shows)
    .bind(stats.cancellations)
    .bind(stats.walk_ins)
    .bind(stats.room_revenue)
    .bind(stats.meal_plan_revenue)
    .bind(stats.pos_revenue)
    .bind(stats.fee_revenue)
    .bind(stats.other_revenue)
    .bind(stats.adr)
    .bind(stats.revpar)
    .bind(Json(&stats.taxes))
    .bind(Json(&stats.payments))
    .execute(&mut **tx)
    .await?;
    let rows = &stats.breakdown;
    sqlx::query(
        "insert into daily_stats_breakdown (tenant_id, property_id, business_date, dimension, key, room_nights,
                                            room_revenue, revenue)
         select $1, $2, $3, * from unnest($4::text[], $5::text[], $6::int[], $7::bigint[], $8::bigint[])",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(stats.business_date)
    .bind(rows.iter().map(|row| row.dimension.as_str()).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.room_nights).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.room_revenue).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.revenue).collect::<Vec<_>>())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The stored statistics of `property` from `from` to `to`, both included, by date, each with its breakdown in
/// [`compute`]'s order. At most 366 days, and `from` not after `to` (`Invalid`); `NotFound` for a property not in the
/// tenant.
pub async fn daily_stats(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Vec<DailyStats>, AuditError> {
    if to < from {
        return Err(AuditError::Invalid("the range ends before it starts".into()));
    }
    if (to - from).whole_days() >= MAX_RANGE_DAYS {
        return Err(AuditError::Invalid(format!("a range covers at most {MAX_RANGE_DAYS} days")));
    }
    let found: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    if !found {
        return Err(AuditError::NotFound("property"));
    }
    let rows: Vec<StoredStats> = sqlx::query_as(
        "select business_date, rooms_total, rooms_out_of_order, rooms_available, rooms_sold, complimentary_rooms,
                occupancy_bp, adults, children, arrivals, departures, no_shows, cancellations, walk_ins, room_revenue,
                meal_plan_revenue, pos_revenue, fee_revenue, other_revenue, adr, revpar, taxes, payments
         from daily_stats
         where property_id = $1 and business_date between $2 and $3
         order by business_date",
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let breakdown: Vec<StoredBreakdown> = sqlx::query_as(
        r#"select business_date, dimension, key, room_nights, room_revenue, revenue
           from daily_stats_breakdown
           where property_id = $1 and business_date between $2 and $3
           order by business_date, dimension collate "C", key collate "C""#,
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let mut by_date: HashMap<Date, Vec<BreakdownRow>> = HashMap::new();
    for row in breakdown {
        by_date.entry(row.business_date).or_default().push(BreakdownRow {
            dimension: row.dimension,
            key: row.key,
            room_nights: row.room_nights,
            room_revenue: row.room_revenue,
            revenue: row.revenue,
        });
    }
    Ok(rows
        .into_iter()
        .map(|row| DailyStats {
            breakdown: by_date.remove(&row.business_date).unwrap_or_default(),
            business_date: row.business_date,
            rooms_total: row.rooms_total,
            rooms_out_of_order: row.rooms_out_of_order,
            rooms_available: row.rooms_available,
            rooms_sold: row.rooms_sold,
            complimentary_rooms: row.complimentary_rooms,
            occupancy_bp: row.occupancy_bp,
            adults: row.adults,
            children: row.children,
            arrivals: row.arrivals,
            departures: row.departures,
            no_shows: row.no_shows,
            cancellations: row.cancellations,
            walk_ins: row.walk_ins,
            room_revenue: row.room_revenue,
            meal_plan_revenue: row.meal_plan_revenue,
            pos_revenue: row.pos_revenue,
            fee_revenue: row.fee_revenue,
            other_revenue: row.other_revenue,
            adr: row.adr,
            revpar: row.revpar,
            taxes: row.taxes.0,
            payments: row.payments.0,
        })
        .collect())
}

/// The stay dimensions of the breakdown.
const STAY_DIMENSIONS: [&str; 4] = ["segment", "source", "residency", "room_type"];

#[derive(sqlx::FromRow)]
struct Counts {
    arrivals: i32,
    departures: i32,
    no_shows: i32,
    cancellations: i32,
    walk_ins: i32,
}

#[derive(sqlx::FromRow)]
struct Line {
    id: Uuid,
    category: String,
    currency: String,
    base_total: i64,
    rate: Decimal,
    posting_key: Option<String>,
    reverses: Option<Uuid>,
    transfer_id: Option<Uuid>,
    tax_amounts: Vec<i64>,
    tax_codes: Vec<String>,
    tax_on_bill: Vec<bool>,
}

#[derive(sqlx::FromRow)]
struct StayRow {
    id: Uuid,
    adults: i32,
    children: i32,
    segment: String,
    source: String,
    residency: String,
    room_type: String,
    complimentary: bool,
}

impl StayRow {
    /// `(dimension, key)` for each of [`STAY_DIMENSIONS`].
    fn dimensions(&self) -> [(&'static str, &str); 4] {
        [
            ("segment", self.segment.as_str()),
            ("source", self.source.as_str()),
            ("residency", self.residency.as_str()),
            ("room_type", self.room_type.as_str()),
        ]
    }
}

/// A settled bill and its payment's currency and rate (`None` without a payment).
#[derive(sqlx::FromRow)]
struct SettledBill {
    bill_no: String,
    lines: Json<Vec<folio::PosLine>>,
    currency: Option<String>,
    rate: Option<Decimal>,
}

#[derive(sqlx::FromRow)]
struct StoredStats {
    business_date: Date,
    rooms_total: i32,
    rooms_out_of_order: i32,
    rooms_available: i32,
    rooms_sold: i32,
    complimentary_rooms: i32,
    occupancy_bp: i32,
    adults: i32,
    children: i32,
    arrivals: i32,
    departures: i32,
    no_shows: i32,
    cancellations: i32,
    walk_ins: i32,
    room_revenue: i64,
    meal_plan_revenue: i64,
    pos_revenue: i64,
    fee_revenue: i64,
    other_revenue: i64,
    adr: i64,
    revpar: i64,
    taxes: Json<BTreeMap<String, i64>>,
    payments: Json<BTreeMap<String, i64>>,
}

#[derive(sqlx::FromRow)]
struct StoredBreakdown {
    business_date: Date,
    dimension: String,
    key: String,
    room_nights: i32,
    room_revenue: i64,
    revenue: i64,
}

/// What attribution needs of a line: its key, the line it reverses and its transfer.
#[derive(sqlx::FromRow)]
struct Link {
    id: Uuid,
    posting_key: Option<String>,
    reverses: Option<Uuid>,
    transfer_id: Option<Uuid>,
}

/// Where a line's attribution goes next.
enum Next {
    /// It ends here: the stay the key names, or none.
    Done(Option<Uuid>),
    /// A line not yet read.
    Line(Uuid),
    /// A transfer whose reversal is not yet read.
    Transfer(Uuid),
}

/// The stay each room and meal-plan line of `lines` belongs to (`None`: unattributed), by line id. The lines a chain
/// of reversals and transfers leads back to are read by id, a few at a time; a transfer's reversal is almost always
/// among D's lines, since both halves of a transfer are posted together.
async fn attribute(tx: &mut Tx, lines: &[Line]) -> Result<HashMap<Uuid, Option<Uuid>>, AuditError> {
    let mut links: HashMap<Uuid, Link> = HashMap::new();
    // transfer → the line its reversal reverses
    let mut reversed: HashMap<Uuid, Uuid> = HashMap::new();
    let mut read_transfers: HashSet<Uuid> = HashSet::new();
    let add = |links: &mut HashMap<Uuid, Link>, reversed: &mut HashMap<Uuid, Uuid>, link: Link| {
        if let (Some(transfer), Some(original)) = (link.transfer_id, link.reverses) {
            reversed.insert(transfer, original);
        }
        links.insert(link.id, link);
    };
    for line in lines {
        let link = Link {
            id: line.id,
            posting_key: line.posting_key.clone(),
            reverses: line.reverses,
            transfer_id: line.transfer_id,
        };
        add(&mut links, &mut reversed, link);
    }
    let attributed: Vec<Uuid> = lines
        .iter()
        .filter(|line| matches!(line.category.as_str(), "room" | "meal_plan"))
        .map(|line| line.id)
        .collect();
    let mut origins = HashMap::new();
    for _ in 0..MAX_CHAIN {
        let mut missing_lines = Vec::new();
        let mut missing_transfers = Vec::new();
        for &id in &attributed {
            if origins.contains_key(&id) {
                continue;
            }
            match follow(id, &links, &reversed, &read_transfers) {
                Next::Done(stay) => {
                    origins.insert(id, stay);
                }
                Next::Line(line) => missing_lines.push(line),
                Next::Transfer(transfer) => missing_transfers.push(transfer),
            }
        }
        if missing_lines.is_empty() && missing_transfers.is_empty() {
            return Ok(origins);
        }
        let mut found: Vec<Link> = Vec::new();
        if !missing_lines.is_empty() {
            found.extend(
                sqlx::query_as::<_, Link>(
                    "select id, posting_key, reverses, transfer_id from folio_line where id = any($1)",
                )
                .bind(&missing_lines)
                .fetch_all(&mut **tx)
                .await?,
            );
        }
        if !missing_transfers.is_empty() {
            found.extend(
                sqlx::query_as::<_, Link>(
                    "select id, posting_key, reverses, transfer_id from folio_line
                     where transfer_id = any($1) and reverses is not null",
                )
                .bind(&missing_transfers)
                .fetch_all(&mut **tx)
                .await?,
            );
            read_transfers.extend(missing_transfers);
        }
        for link in found {
            add(&mut links, &mut reversed, link);
        }
    }
    // Chains longer than MAX_CHAIN reads stay unattributed.
    for id in attributed {
        origins.entry(id).or_insert(None);
    }
    Ok(origins)
}

/// Follows `id` back through reversals and transfers as far as the lines read so far go.
fn follow(
    id: Uuid,
    links: &HashMap<Uuid, Link>,
    reversed: &HashMap<Uuid, Uuid>,
    read_transfers: &HashSet<Uuid>,
) -> Next {
    let mut at = id;
    for _ in 0..MAX_CHAIN {
        let Some(link) = links.get(&at) else {
            return Next::Line(at);
        };
        if let Some(key) = &link.posting_key {
            return Next::Done(stay_of_key(key));
        }
        match (link.reverses, link.transfer_id) {
            (Some(original), _) => at = original,
            (None, Some(transfer)) => match reversed.get(&transfer) {
                Some(&original) => at = original,
                None if read_transfers.contains(&transfer) => return Next::Done(None),
                None => return Next::Transfer(transfer),
            },
            (None, None) => return Next::Done(None),
        }
    }
    Next::Done(None)
}

/// The stay a room or meal-plan posting key (`room:<stay>:<date>`, `meal_plan:<stay>:<date>`) names.
fn stay_of_key(key: &str) -> Option<Uuid> {
    let mut parts = key.split(':');
    match (parts.next(), parts.next()) {
        (Some("room" | "meal_plan"), Some(stay)) => Uuid::parse_str(stay).ok(),
        _ => None,
    }
}

/// `n / d` rounded half away from zero; 0 when `d` is 0.
fn div_half_up(n: i64, d: i64) -> i64 {
    if d == 0 {
        return 0;
    }
    let (n, d) = (i128::from(n), i128::from(d));
    let quotient = (2 * n.abs() + d.abs()) / (2 * d.abs());
    let signed = if (n < 0) != (d < 0) { -quotient } else { quotient };
    i64::try_from(signed).unwrap_or(if signed < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_rounds_half_away_from_zero() {
        assert_eq!(div_half_up(44_444, 10), 4_444);
        assert_eq!(div_half_up(5, 2), 3);
        assert_eq!(div_half_up(-5, 2), -3);
        assert_eq!(div_half_up(7, 0), 0);
        assert_eq!(div_half_up(40_000, 9), 4_444);
    }

    #[test]
    fn a_key_names_its_stay_only_for_room_and_meal_plan() {
        let stay = Uuid::now_v7();
        assert_eq!(stay_of_key(&format!("room:{stay}:2026-10-20")), Some(stay));
        assert_eq!(stay_of_key(&format!("meal_plan:{stay}:2026-10-20")), Some(stay));
        assert_eq!(stay_of_key(&format!("no_show:{stay}")), None);
        assert_eq!(stay_of_key("room:not-a-uuid:2026-10-20"), None);
    }
}
