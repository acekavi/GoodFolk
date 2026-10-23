//! The five review lists of a business date, and the count of what blocks its close.

use crate::AuditError;
use db::Tx;
use domain::RoomStatus;
use reservations::CancellationTerms;
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

/// The review of the property's business date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NightAudit {
    pub business_date: Date,
    /// Unresolved arrivals plus unresolved departures: `arrivals.len() + departures.len()`.
    pub blocking: u32,
    pub arrivals: Vec<ArrivalRow>,
    pub departures: Vec<DepartureRow>,
    pub pos_bills: Vec<OutletBills>,
    pub booking_bills: BookingBills,
    pub payments: Vec<PaymentGroup>,
}

/// A stay due by the business date and still tentative or confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrivalRow {
    pub reservation_room: Uuid,
    pub reservation_id: Uuid,
    pub confirmation: String,
    pub guest: String,
    pub room_number: Option<String>,
    pub arrival: Date,
    pub departure: Date,
    pub status: RoomStatus,
    /// The stay's room and meal amounts, all nights.
    pub total: i64,
    pub currency: String,
    /// What marking it a no-show would cost under its booked terms ([`reservations::no_show_penalty`]).
    pub no_show_penalty: i64,
    pub version: i32,
}

/// A stay due out by the business date and still checked in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartureRow {
    pub reservation_room: Uuid,
    pub reservation_id: Uuid,
    pub confirmation: String,
    pub guest: String,
    pub room_number: Option<String>,
    pub departure: Date,
    /// The guest folio's balance per currency.
    pub balances: Vec<(String, i64)>,
    pub version: i32,
}

/// One outlet's POS bills of the business date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutletBills {
    /// The outlet's name.
    pub outlet: String,
    pub bills: Vec<PosBillRow>,
    pub total_base: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosBillRow {
    pub id: Uuid,
    pub bill_no: String,
    /// The bill's own business date: earlier than the review's for a bill posted late, whose lines and payment are
    /// dated the day it was posted.
    pub bill_date: Date,
    pub currency: String,
    /// The sum of its lines' totals: its folio lines for a bill posted to a stay, its stored lines for a settled one.
    pub total: i64,
    pub total_base: i64,
    /// Room number of the stay it posted to; None when settled at the outlet.
    pub room_number: Option<String>,
    pub settled: bool,
    /// Posted to a stay now checked out whose guest folio is not settled.
    pub highlight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookingBills {
    /// Tonight's room and meal-plan charges, as posting them would go.
    pub nights: Vec<folio::NightPreview>,
    /// The business date's manual and fee lines, by category.
    pub posted: Vec<CategoryTotal>,
}

/// The manual and fee lines of the business date in one category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryTotal {
    pub category: String,
    pub lines: u32,
    pub total_base: i64,
}

/// The business date's payments in one method and currency. Refunds are in `amount` and `base_amount` (net), and
/// also in `refunds` (their sum, at most 0); `count` counts the payments, not the refunds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentGroup {
    /// `cash`, `card_terminal`, `bank_transfer`, or `account` (a charge to account: the city ledger).
    pub method: String,
    pub currency: String,
    pub count: u32,
    pub amount: i64,
    pub base_amount: i64,
    pub refunds: i64,
    /// LKR payments, not refunds, with no evidence, on the guest folio of a stay whose rate plan is in segment
    /// `FIT_F` (sold to non-residents).
    pub flagged: Vec<Uuid>,
}

/// The arrivals list: stays of the property (`$1`) due by the business date (`$2`) and still tentative or confirmed,
/// by room number then confirmation. Public so a test can `EXPLAIN` it: `arrival` is stored and its comparisons are
/// leakproof, so under row-level security `reservation_room_arrival_idx` serves it.
pub const ARRIVALS_SQL: &str = "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name,
        room.number, rr.arrival, rr.arrival + rr.nights as departure, rr.status, rr.currency::text,
        rr.cancellation_terms, rr.version,
        coalesce(n.dates, '{}') as dates, coalesce(n.rooms, '{}') as rooms, coalesce(n.meals, '{}') as meals
 from reservation_room rr
 join reservation r on r.id = rr.reservation_id
 join guest g on g.id = rr.primary_guest_id
 left join room on room.id = rr.room_id
 left join lateral (select array_agg(date order by date) as dates, array_agg(room_amount order by date) as rooms,
                           array_agg(meal_amount order by date) as meals
                    from reservation_night where reservation_room_id = rr.id) n on true
 where rr.property_id = $1 and rr.arrival <= $2 and rr.status in ('tentative', 'confirmed')
 order by room.number, r.confirmation_no, rr.id";

/// The departures list: stays of the property (`$1`) checked in and due out by the business date (`$2`), with their
/// guest folio's balance per currency, by room number then confirmation. Bounded by `arrival` (a stay leaving by
/// `$2` arrived before it) through `reservation_room_arrival_idx`, as [`ARRIVALS_SQL`] is.
pub const DEPARTURES_SQL: &str = "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name,
        room.number, rr.arrival + rr.nights as departure, rr.version,
        coalesce(b.currencies, '{}') as currencies, coalesce(b.amounts, '{}') as amounts
 from reservation_room rr
 join reservation r on r.id = rr.reservation_id
 join guest g on g.id = rr.primary_guest_id
 left join room on room.id = rr.room_id
 left join folio f on f.reservation_room_id = rr.id and f.kind = 'guest'
 left join lateral (
   select array_agg(currency order by currency) as currencies, array_agg(amount order by currency) as amounts
   from (select currency, sum(amount)::bigint as amount
         from (select currency::text as currency, total as amount from folio_line where folio_id = f.id
               union all
               select currency::text, -amount from payment where folio_id = f.id) as entries
         group by currency) as sums) b on true
 where rr.property_id = $1 and rr.arrival <= $2 and rr.status = 'checked_in' and rr.arrival + rr.nights <= $2
 order by room.number, r.confirmation_no, rr.id";

/// How many stays of `property` block closing `date`: tentative or confirmed with `arrival <= date`, plus checked in
/// with `departure <= date`.
pub async fn blocking(tx: &mut Tx, property: Uuid, date: Date) -> Result<u32, AuditError> {
    let count: i64 = sqlx::query_scalar(
        "select count(*) from reservation_room
         where property_id = $1 and arrival <= $2
           and (status in ('tentative', 'confirmed') or (status = 'checked_in' and arrival + nights <= $2))",
    )
    .bind(property)
    .bind(date)
    .fetch_one(&mut **tx)
    .await?;
    Ok(count_u32(count))
}

/// The review of `property`'s business date (`NotFound` for a property not in the tenant): each list is one
/// statement, and the booking bills' nights are [`folio::preview_business_date_charges`]'s.
pub async fn night_audit(tx: &mut Tx, property: Uuid) -> Result<NightAudit, AuditError> {
    let date: Date = sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AuditError::NotFound("property"))?;
    let arrivals = arrivals(tx, property, date).await?;
    let departures = departures(tx, property, date).await?;
    let pos_bills = pos_bills(tx, property, date).await?;
    let nights = folio::preview_business_date_charges(tx, property, date).await?;
    let posted = posted_lines(tx, property, date).await?;
    let payments = payments(tx, property, date).await?;
    Ok(NightAudit {
        business_date: date,
        blocking: u32::try_from(arrivals.len() + departures.len()).unwrap_or(u32::MAX),
        arrivals,
        departures,
        pos_bills,
        booking_bills: BookingBills { nights, posted },
        payments,
    })
}

#[derive(sqlx::FromRow)]
struct Arrival {
    id: Uuid,
    reservation_id: Uuid,
    confirmation_no: String,
    first_name: String,
    last_name: String,
    number: Option<String>,
    arrival: Date,
    departure: Date,
    status: String,
    currency: String,
    cancellation_terms: Option<Json<CancellationTerms>>,
    version: i32,
    dates: Vec<Date>,
    rooms: Vec<i64>,
    meals: Vec<i64>,
}

async fn arrivals(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<ArrivalRow>, AuditError> {
    let rows: Vec<Arrival> = sqlx::query_as(ARRIVALS_SQL).bind(property).bind(date).fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            let nights: Vec<(Date, i64, i64)> =
                row.dates.iter().zip(&row.rooms).zip(&row.meals).map(|((&d, &room), &meal)| (d, room, meal)).collect();
            let status = RoomStatus::parse(&row.status).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "status".into(),
                source: format!("unknown value {:?}", row.status).into(),
            })?;
            Ok(ArrivalRow {
                reservation_room: row.id,
                reservation_id: row.reservation_id,
                confirmation: row.confirmation_no,
                guest: guest_name(row.first_name, row.last_name),
                room_number: row.number,
                arrival: row.arrival,
                departure: row.departure,
                status,
                total: nights.iter().map(|&(_, room, meal)| room + meal).sum(),
                currency: row.currency,
                no_show_penalty: reservations::no_show_penalty(
                    row.cancellation_terms.as_ref().map(|terms| &terms.0),
                    &nights,
                ),
                version: row.version,
            })
        })
        .collect()
}

#[derive(sqlx::FromRow)]
struct Departure {
    id: Uuid,
    reservation_id: Uuid,
    confirmation_no: String,
    first_name: String,
    last_name: String,
    number: Option<String>,
    departure: Date,
    version: i32,
    currencies: Vec<String>,
    amounts: Vec<i64>,
}

async fn departures(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<DepartureRow>, AuditError> {
    let rows: Vec<Departure> = sqlx::query_as(DEPARTURES_SQL).bind(property).bind(date).fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(|row| DepartureRow {
            reservation_room: row.id,
            reservation_id: row.reservation_id,
            confirmation: row.confirmation_no,
            guest: guest_name(row.first_name, row.last_name),
            room_number: row.number,
            departure: row.departure,
            balances: row.currencies.into_iter().zip(row.amounts).collect(),
            version: row.version,
        })
        .collect())
}

/// The payment of the POS bill `b` settled at its outlet, as a lateral subquery: its `currency`, `base_amount`,
/// `rate` and `business_date` (the day it posted, which is the bill's own date unless the bill came in late).
/// `pos_bill` has no currency of its own; its payment's is the bill's.
pub(crate) const SETTLED_PAYMENT: &str = "select currency::text as currency, base_amount, rate, business_date
                                          from payment where pos_bill_id = b.id and refund_of is null limit 1";

#[derive(sqlx::FromRow)]
struct Bill {
    outlet: String,
    id: Uuid,
    bill_no: String,
    bill_date: Date,
    currency: String,
    total: i64,
    total_base: i64,
    room_number: Option<String>,
    settled: bool,
    highlight: bool,
}

/// The POS bills posted on D, by outlet code then posting order: those with folio lines dated D (found among D's lines
/// by their posting keys, `pos:<bill>:<n>`, in one pass) and those settled with a payment dated D. A bill that came
/// in late is listed on the day it posted, with its own earlier date. A settled bill's base total is its payment's.
async fn pos_bills(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<OutletBills>, AuditError> {
    let rows: Vec<Bill> = sqlx::query_as(
        "with posted as (
           select split_part(l.posting_key, ':', 2)::uuid as bill_id, min(l.currency::text) as currency,
                  sum(l.total) as total, sum(l.base_total) as base
           from folio_line l join folio f on f.id = l.folio_id
           where l.business_date = $2 and f.property_id = $1 and l.posting_key like 'pos:%'
           group by split_part(l.posting_key, ':', 2)
         ), paid as (
           select pos_bill_id as bill_id, currency::text as currency, base_amount
           from payment
           where business_date = $2 and pos_bill_id is not null and refund_of is null
         )
         select o.name as outlet, b.id, b.bill_no, b.business_date as bill_date,
                coalesce(paid.currency, posted.currency, '') as currency,
                case when b.settled
                     then (select coalesce(sum((line ->> 'total')::bigint), 0) from jsonb_array_elements(b.lines) line)
                     else coalesce(posted.total, 0) end::bigint as total,
                coalesce(paid.base_amount, posted.base, 0)::bigint as total_base,
                room.number as room_number, b.settled,
                coalesce(rr.status = 'checked_out' and exists (
                  select 1
                  from (select currency, total as amount from folio_line where folio_id = gf.id
                        union all
                        select currency, -amount from payment where folio_id = gf.id) as entries
                  group by currency having sum(amount) <> 0), false) as highlight
         from (select bill_id from posted union select bill_id from paid) day
         join pos_bill b on b.id = day.bill_id
         join outlet o on o.id = b.outlet_id
         left join posted on posted.bill_id = b.id and not b.settled
         left join paid on paid.bill_id = b.id and b.settled
         left join reservation_room rr on rr.id = b.reservation_room_id
         left join room on room.id = rr.room_id
         left join folio gf on gf.reservation_room_id = rr.id and gf.kind = 'guest'
         where b.property_id = $1
         order by o.code, b.posted_at, b.id",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    let mut outlets: Vec<OutletBills> = Vec::new();
    for row in rows {
        let bill = PosBillRow {
            id: row.id,
            bill_no: row.bill_no,
            bill_date: row.bill_date,
            currency: row.currency,
            total: row.total,
            total_base: row.total_base,
            room_number: row.room_number,
            settled: row.settled,
            highlight: row.highlight,
        };
        match outlets.last_mut() {
            Some(outlet) if outlet.outlet == row.outlet => {
                outlet.total_base += bill.total_base;
                outlet.bills.push(bill);
            }
            _ => outlets.push(OutletBills { outlet: row.outlet, total_base: bill.total_base, bills: vec![bill] }),
        }
    }
    Ok(outlets)
}

/// D's manual and fee lines on the property's folios, by category.
async fn posted_lines(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<CategoryTotal>, AuditError> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "select l.category, count(*), sum(l.base_total)::bigint
         from folio_line l join folio f on f.id = l.folio_id
         where f.property_id = $1 and l.business_date = $2
           and l.source in ('manual', 'cancellation_fee', 'no_show_fee')
         group by l.category
         order by l.category",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(category, lines, total_base)| CategoryTotal { category, lines: count_u32(lines), total_base })
        .collect())
}

#[derive(sqlx::FromRow)]
struct Payment {
    id: Uuid,
    method: String,
    currency: String,
    amount: i64,
    base_amount: i64,
    refund: bool,
    flagged: bool,
}

/// D's payments on the property's folios and its POS bills, but not the transfers between folios, by method then
/// currency.
async fn payments(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<PaymentGroup>, AuditError> {
    let rows: Vec<Payment> = sqlx::query_as(
        "select p.id, p.method, p.currency::text as currency, p.amount, p.base_amount,
                p.refund_of is not null as refund,
                coalesce(p.currency = 'LKR' and p.refund_of is null and coalesce(p.evidence, '') = ''
                         and rp.segment = 'FIT_F', false) as flagged
         from payment p
         left join folio f on f.id = p.folio_id
         left join pos_bill b on b.id = p.pos_bill_id
         left join reservation_room rr on rr.id = f.reservation_room_id
         left join rate_plan rp on rp.id = rr.rate_plan_id
         where p.business_date = $2 and p.method <> 'transfer'
           and (f.property_id = $1 or (p.folio_id is null and b.property_id = $1))
         order by p.method, p.currency, p.id",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    let mut groups: Vec<PaymentGroup> = Vec::new();
    for row in rows {
        let group = match groups.last_mut() {
            Some(group) if group.method == row.method && group.currency == row.currency => group,
            _ => {
                groups.push(PaymentGroup {
                    method: row.method,
                    currency: row.currency,
                    count: 0,
                    amount: 0,
                    base_amount: 0,
                    refunds: 0,
                    flagged: Vec::new(),
                });
                groups.last_mut().expect("just pushed")
            }
        };
        group.amount += row.amount;
        group.base_amount += row.base_amount;
        if row.refund {
            group.refunds += row.amount;
        } else {
            group.count += 1;
        }
        if row.flagged {
            group.flagged.push(row.id);
        }
    }
    Ok(groups)
}

/// "First Last", or the last name alone when there is no first name.
fn guest_name(first_name: String, last_name: String) -> String {
    if first_name.is_empty() { last_name } else { format!("{first_name} {last_name}") }
}

fn count_u32(count: i64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}
