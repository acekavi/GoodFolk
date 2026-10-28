//! The Social Security Contribution Levy of a calendar quarter, month by month, with the foreign-currency exemption.

use crate::everyday::ratio;
use crate::liability::lines::{folio_revenue_lines, revenue_lines};
use crate::liability::settlement::{Segment, segments_of};
use crate::{RUPEE, ReportsError};
use db::Tx;
use serde::Serialize;
use std::collections::HashMap;
use time::{Date, Month};
use uuid::Uuid;

/// The levy: 2.5% of the liable turnover, in basis points.
const LEVY_BP: i64 = 250;

/// The day of the following month a month's levy, and the quarter's return, are due.
const DUE_DAY: u8 = 20;

/// How the exemption is measured, printed on the report so the hotel's accountant can check it.
pub const METHOD_NOTE: &str = "Exempt: services paid in foreign currency by card or bank transfer (SSCL Act No. 25 of \
                               2022, First Schedule Part II item 10), measured when each bill is settled.";

/// What a month whose exemption exceeds its turnover shows.
pub const EXCESS_EXEMPTION_NOTE: &str =
    "Exemption exceeds turnover this month: the excess is not carried to other months";

/// Why a property without an SLTDA licence exempts nothing.
pub const NOT_REGISTERED_NOTE: &str = "Not SLTDA registered: no exemption";

/// A calendar quarter: Q1 is January–March, Q2 April–June, Q3 July–September and Q4 October–December.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarQuarter {
    pub year: i32,
    /// 1 to 4.
    pub q: u8,
}

impl CalendarQuarter {
    /// The quarter's first and last days. `Invalid` for a quarter outside 1 to 4 or a year out of range.
    pub fn range(self) -> Result<(Date, Date), ReportsError> {
        let months = self.months()?;
        Ok((months[0].0, months[2].1))
    }

    /// The first and last day of each of the quarter's months. `Invalid` for a quarter outside 1 to 4 or a year out of
    /// range.
    fn months(self) -> Result<[(Date, Date); 3], ReportsError> {
        if !(1..=4).contains(&self.q) {
            return Err(ReportsError::Invalid(format!("a quarter is 1 to 4, not {}", self.q)));
        }
        let invalid = || ReportsError::Invalid(format!("{} is not a year of the calendar", self.year));
        let mut months = [(Date::MIN, Date::MIN); 3];
        for (at, days) in months.iter_mut().enumerate() {
            let month = Month::try_from(self.q * 3 - 2 + at as u8).expect("a quarter's months are 1 to 12");
            *days = (
                Date::from_calendar_date(self.year, month, 1).map_err(|_| invalid())?,
                Date::from_calendar_date(self.year, month, month.length(self.year)).map_err(|_| invalid())?,
            );
        }
        Ok(months)
    }
}

/// The 20th of the month after the one ending `last_day`.
fn due_after(last_day: Date) -> Result<Date, ReportsError> {
    last_day
        .next_day()
        .and_then(|next| next.replace_day(DUE_DAY).ok())
        .ok_or_else(|| ReportsError::Invalid(format!("{} is not a year of the calendar", last_day.year())))
}

/// One month of the quarter. Money is in base LKR minor units, every figure whole rupees, so the month adds up as
/// printed: each figure is its sum rounded once, the liable turnover is the rounded turnover less the rounded
/// exemptions and bad debts, and the levy 2.5% of that, rounded once more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsclMonth {
    pub month: Month,
    /// Every revenue line dated in the month, with every tax but VAT passed on to the guest: the passed-on TDL and
    /// SSCL stay in. An absorbed tax is inside the line's net already.
    pub turnover: i64,
    /// The exempt share of the month's turnover settled in the month.
    pub exempt_same_month: i64,
    /// "Exemption for folios settled this month (turnover of earlier months)".
    pub exempt_earlier_months: i64,
    /// Always 0: the system writes off no debts.
    pub bad_debts: i64,
    /// Below 0 when an earlier month's exemption outweighs the month's turnover.
    pub liable: i64,
    /// Never below 0.
    pub levy: i64,
    /// The liable turnover is below 0: the month's exemption exceeds its turnover, and the excess is not carried to
    /// other months. Shown as [`EXCESS_EXEMPTION_NOTE`].
    pub excess_exemption: bool,
    /// Payments in foreign currency in cash in the month: listed, never exempt.
    pub foreign_cash: i64,
    /// The 20th of the following month.
    pub due: Date,
    /// Some day of the month is not closed yet: its figures cover the closed days only.
    pub not_closed: bool,
}

/// The quarter's totals: the sums of its months' printed figures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SsclTotals {
    pub turnover: i64,
    pub exempt_same_month: i64,
    pub exempt_earlier_months: i64,
    pub bad_debts: i64,
    pub liable: i64,
    pub levy: i64,
    pub foreign_cash: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsclReport {
    pub months: [SsclMonth; 3],
    pub quarter_totals: SsclTotals,
    /// The 20th of the month after the quarter.
    pub return_due: Date,
    /// The property's SLTDA licence is filled in: without it nothing is exempt.
    pub sltda_registered: bool,
    /// [`NOT_REGISTERED_NOTE`] for a property that is not SLTDA registered.
    pub exemption_note: Option<&'static str>,
    /// [`METHOD_NOTE`].
    pub method_note: &'static str,
}

/// `property`'s SSCL for `quarter`, from its closed days. The turnover is [`revenue_lines`]' net plus every tax but
/// VAT passed on to the guest (an absorbed one is inside the net), by month. For an SLTDA-registered property, each
/// settlement ([`segments`]) in a month exempts its exempt share of the turnover of the lines it covers: those dated
/// in the month as `exempt_same_month`, earlier ones as `exempt_earlier_months`. Turnover not settled by a month's end
/// is not exempt in it, so a closed month never changes. A month whose liable turnover is below 0 owes no levy and is
/// flagged `excess_exemption`. `Invalid` for a quarter outside 1 to 4; `NotFound` for a property not in the tenant.
pub async fn sscl(tx: &mut Tx, property: Uuid, quarter: CalendarQuarter) -> Result<SsclReport, ReportsError> {
    let months = quarter.months()?;
    let (from, to) = (months[0].0, months[2].1);
    let (sltda_licence, business_date): (Option<String>, Date) =
        sqlx::query_as("select sltda_licence, business_date from property where id = $1")
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(ReportsError::NotFound("property"))?;
    let sltda_registered = sltda_licence.is_some();
    let month_of = |date: Date| months.iter().position(|&(first, last)| first <= date && date <= last);

    let mut lines = revenue_lines(tx, property, from, to).await?;
    let settled = if sltda_registered { segments_of(tx, property, from, to, &lines).await? } else { Vec::new() };
    let mut by_subject: HashMap<Uuid, Vec<Segment>> = HashMap::new();
    for segment in &settled {
        by_subject.entry(segment.subject_id).or_default().push(*segment);
    }
    // The earlier lines of only the folios whose settlements in the quarter reach back before it (a bill settled at its
    // outlet covers its own date only).
    let reaching_back: Vec<&Segment> = settled.iter().filter(|segment| segment.covers_from < from).collect();
    if let Some(earliest) = reaching_back.iter().map(|segment| segment.covers_from).min() {
        let folios: Vec<Uuid> = reaching_back.iter().map(|segment| segment.subject_id).collect();
        let before = from.previous_day().expect("a quarter starting after the earliest settlement has a day before it");
        lines.extend(folio_revenue_lines(tx, property, &folios, earliest, before).await?);
    }

    // Sums in minor units, and the exemptions in minor units × basis points, rounded once per printed figure.
    let (mut turnover, mut same_month, mut earlier) = ([0i64; 3], [0i64; 3], [0i64; 3]);
    for line in lines {
        let passed_on = line.taxes.iter().filter(|tax| tax.code != "VAT" && tax.presentation == "on_bill");
        let sales = line.net + passed_on.map(|tax| tax.amount).sum::<i64>();
        if let Some(at) = month_of(line.business_date) {
            turnover[at] += sales;
        }
        let Some(subject) = line.folio_id.or(line.pos_bill_id) else { continue };
        let Some(segment) = covering(by_subject.get(&subject), line.business_date) else { continue };
        let at = month_of(segment.settled_on).expect("a settlement of the quarter is in one of its months");
        let exempt = sales.saturating_mul(i64::from(segment.exempt_share_bp));
        if line.business_date >= months[at].0 {
            same_month[at] += exempt;
        } else {
            earlier[at] += exempt;
        }
    }

    let closed_to = business_date.previous_day().map(|closed| to.min(closed));
    let mut foreign_cash = [0i64; 3];
    if let Some(closed_to) = closed_to.filter(|closed_to| from <= *closed_to) {
        let rows: Vec<(Date, i64)> = sqlx::query_as(
            "select p.business_date, sum(p.base_amount)::bigint
             from payment p
             left join folio f on f.id = p.folio_id
             left join pos_bill b on b.id = p.pos_bill_id
             where p.business_date between $2 and $3 and p.method = 'cash' and p.currency <> 'LKR'
               and (f.property_id = $1 or (p.folio_id is null and b.property_id = $1))
             group by p.business_date",
        )
        .bind(property)
        .bind(from)
        .bind(closed_to)
        .fetch_all(&mut **tx)
        .await?;
        for (date, amount) in rows {
            if let Some(at) = month_of(date) {
                foreign_cash[at] += amount;
            }
        }
    }

    let rupees = |sum: i64, unit: i64| ratio(sum, unit * RUPEE) * RUPEE;
    let mut totals = SsclTotals::default();
    let mut report_months = Vec::with_capacity(3);
    for (at, (first_day, last_day)) in months.into_iter().enumerate() {
        let turnover = rupees(turnover[at], 1);
        let exempt_same_month = rupees(same_month[at], 10_000);
        let exempt_earlier_months = rupees(earlier[at], 10_000);
        let bad_debts = 0;
        let liable = turnover - exempt_same_month - exempt_earlier_months - bad_debts;
        let levy = rupees(liable.saturating_mul(LEVY_BP), 10_000).max(0);
        let foreign_cash = rupees(foreign_cash[at], 1);
        totals.turnover += turnover;
        totals.exempt_same_month += exempt_same_month;
        totals.exempt_earlier_months += exempt_earlier_months;
        totals.bad_debts += bad_debts;
        totals.liable += liable;
        totals.levy += levy;
        totals.foreign_cash += foreign_cash;
        report_months.push(SsclMonth {
            month: first_day.month(),
            turnover,
            exempt_same_month,
            exempt_earlier_months,
            bad_debts,
            liable,
            levy,
            excess_exemption: liable < 0,
            foreign_cash,
            due: due_after(last_day)?,
            not_closed: last_day >= business_date,
        });
    }
    Ok(SsclReport {
        months: report_months.try_into().expect("a quarter has three months"),
        quarter_totals: totals,
        return_due: due_after(to)?,
        sltda_registered,
        exemption_note: (!sltda_registered).then_some(NOT_REGISTERED_NOTE),
        method_note: METHOD_NOTE,
    })
}

/// The segment of `segments` (one folio's or bill's, in date order) covering a line dated `date`.
fn covering(segments: Option<&Vec<Segment>>, date: Date) -> Option<&Segment> {
    let segments = segments?;
    let at = segments.partition_point(|segment| segment.settled_on < date);
    segments.get(at).filter(|segment| segment.covers_from <= date)
}
