//! When folios and outlet bills are settled, and what share of each settlement was paid in foreign currency through
//! a bank: what the SSCL exemption is measured by.

use crate::ReportsError;
use crate::everyday::ratio;
use db::Tx;
use std::collections::HashMap;
use time::Date;
use uuid::Uuid;

/// The whole of a share, in basis points.
const WHOLE_BP: i64 = 10_000;

/// The base of the payment `alias` when it is in foreign currency through a bank (card terminal or bank transfer),
/// else 0.
fn exempt_base(alias: &str) -> String {
    format!(
        "case when {alias}.currency <> 'LKR' and {alias}.method in ('card_terminal', 'bank_transfer') \
         then {alias}.base_amount else 0 end"
    )
}

/// A settlement: a folio's balance coming back to zero, covering its lines since its previous settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// What was settled: a folio, or a POS bill settled at its outlet (each is settled on its own, by its own
    /// payment).
    pub subject_id: Uuid,
    /// The business date after which the balance was zero.
    pub settled_on: Date,
    /// The share of the segment's payments (refunds negative) made in a currency other than LKR by card terminal or
    /// bank transfer, 0 to 10,000.
    pub exempt_share_bp: u32,
    /// The first business date of the segment's lines and payments: it covers its folio's lines from then to
    /// `settled_on`.
    pub covers_from: Date,
}

/// One folio's lines and payments of one business date in one currency.
#[derive(sqlx::FromRow)]
struct Entry {
    folio_id: Uuid,
    business_date: Date,
    currency: String,
    /// Lines less payments.
    amount: i64,
    /// The base of the payments in foreign currency by card terminal or bank transfer, refunds negative, and each
    /// transfer in's share of its source's.
    exempt: i64,
    /// The base of the payments, refunds negative, and of the transfers in (transfers out left out).
    paid: i64,
}

/// The settlements of `property` dated `from` to `upto` (both included), up to its last closed date: each folio's
/// lines and payments walked in business-date order, and each business date after which its balance is zero in every
/// currency (as an invoice needs: the base rarely nets to zero, since each night's line and the payment convert at
/// their own day's rate) a settlement point, when something happened since the previous one. A segment runs
/// from the previous point (or the folio's first business date) to that one. Payments are signed as stored: a refund
/// is negative, and a transfer between folios negative on the folio it leaves. A transfer in (a deposit moved to its
/// stay at check-in) carries its source folio's share: its base times the foreign-through-a-bank share of the source's
/// own payments (refunds included) up to the transfer's business date. One level deep: a source that was itself funded by a transfer
/// counts its direct payments only. A charge to account (`account`) settles the folio but is neither foreign currency
/// nor through a bank, so it lowers the exempt share. A POS bill settled at its outlet is a settlement of its own on
/// its payment's date.
///
/// Reads every folio with a line or payment in the range, with all its earlier history, in one statement, and the
/// outlet bills in another. `NotFound` for a property not in the tenant.
pub async fn segments(tx: &mut Tx, property: Uuid, from: Date, upto: Date) -> Result<Vec<Segment>, ReportsError> {
    let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;
    let Some(upto) = business_date.previous_day().map(|closed| upto.min(closed)).filter(|upto| from <= *upto) else {
        return Ok(Vec::new());
    };

    let entries: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "with folios as (
           select l.folio_id from folio_line l join folio f on f.id = l.folio_id
           where f.property_id = $1 and l.business_date between $2 and $3
           union
           select p.folio_id from payment p join folio f on f.id = p.folio_id
           where f.property_id = $1 and p.business_date between $2 and $3
         )
         select folio_id, business_date, currency, sum(amount)::bigint as amount, sum(exempt)::bigint as exempt,
                sum(paid)::bigint as paid
         from (select folio_id, business_date, currency::text as currency, total as amount, 0::bigint as exempt,
                      0::bigint as paid
               from folio_line where folio_id in (select folio_id from folios) and business_date <= $3
               union all
               select p.folio_id, p.business_date, p.currency::text, -p.amount,
                      case when p.method = 'transfer' then coalesce(source.exempt, 0) else {exempt_p} end,
                      case when p.method = 'transfer' then greatest(p.base_amount, 0) else p.base_amount end
               from payment p
               cross join lateral (
                 select round(p.base_amount * least(greatest(
                          sum({exempt_q})::numeric / nullif(sum(q.base_amount), 0), 0), 1))::bigint as exempt
                 from payment q
                 where p.method = 'transfer' and p.base_amount > 0
                   and q.folio_id = coalesce(p.details->>'from_folio', p.details->>'to_folio')::uuid
                   and q.method <> 'transfer' and q.business_date <= p.business_date
               ) source
               where p.folio_id in (select folio_id from folios) and p.business_date <= $3) as entries
         group by folio_id, business_date, currency
         order by folio_id, business_date, currency",
        exempt_p = exempt_base("p"),
        exempt_q = exempt_base("q"),
    )))
    .bind(property)
    .bind(from)
    .bind(upto)
    .fetch_all(&mut **tx)
    .await?;

    let mut found = Vec::new();
    for history in entries.chunk_by(|a, b| a.folio_id == b.folio_id) {
        let mut balances: HashMap<&str, i64> = HashMap::new();
        let (mut covers_from, mut exempt, mut paid) = (None, 0, 0);
        for day in history.chunk_by(|a, b| a.business_date == b.business_date) {
            for entry in day {
                *balances.entry(&entry.currency).or_default() += entry.amount;
                exempt += entry.exempt;
                paid += entry.paid;
            }
            let settled_on = day[0].business_date;
            let start = *covers_from.get_or_insert(settled_on);
            if balances.values().all(|&balance| balance == 0) {
                if settled_on >= from {
                    found.push(Segment {
                        subject_id: day[0].folio_id,
                        settled_on,
                        exempt_share_bp: share_bp(exempt, paid),
                        covers_from: start,
                    });
                }
                (covers_from, exempt, paid) = (None, 0, 0);
            }
        }
    }

    let bills: Vec<(Uuid, Date, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select p.pos_bill_id, min(p.business_date), sum({exempt})::bigint, sum(p.base_amount)::bigint
         from payment p join pos_bill b on b.id = p.pos_bill_id
         where b.property_id = $1 and b.settled and p.folio_id is null and p.business_date between $2 and $3
         group by p.pos_bill_id
         order by p.pos_bill_id",
        exempt = exempt_base("p"),
    )))
    .bind(property)
    .bind(from)
    .bind(upto)
    .fetch_all(&mut **tx)
    .await?;
    found.extend(bills.into_iter().map(|(bill, settled_on, exempt, paid)| Segment {
        subject_id: bill,
        settled_on,
        exempt_share_bp: share_bp(exempt, paid),
        covers_from: settled_on,
    }));
    Ok(found)
}

/// `exempt` of `paid`, in basis points clamped to the whole: 0 when the payments net to nothing. Both may be below 0
/// (a segment of only a refund), and the share is then the refund's.
fn share_bp(exempt: i64, paid: i64) -> u32 {
    let (exempt, paid) = if paid < 0 { (-exempt, -paid) } else { (exempt, paid) };
    u32::try_from(ratio(exempt.saturating_mul(WHOLE_BP), paid).clamp(0, WHOLE_BP)).unwrap_or_default()
}
