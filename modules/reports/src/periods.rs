//! The reports the close renders: each weekly (Monday to Sunday) and monthly report, TDL quarter (of the April–March
//! financial year), SSCL quarter (calendar), VAT return period (the property's, when it is VAT registered with a TIN)
//! and service charge month whose last day is closed and that is not stored yet, or failed to render.
//!
//! **Builder's call:** the scan reaches back 13 months at most, and to periods ending on or after the property's first
//! closed day, so a property closing for the first time gets its current periods, never years of empty ones.

use crate::ReportsError;
use crate::document::{RenderRequest, document};
use crate::liability::sscl::CalendarQuarter;
use crate::liability::tdl::FiscalQuarter;
use crate::liability::vat::VatPeriod;
use crate::store::{ReportKind, clear_reports_alert, queue_render};
use db::{TenantId, Tx};
use time::{Date, Duration, Month};
use uuid::Uuid;

/// How far back the scan reaches, in months.
const MONTHS_BACK: i32 = 13;

/// `property`'s periods closed through `closed_through` that the close renders, by kind and start: each one whose
/// last day is from the scan's start (see the module) to `closed_through`, with no stored report or one that failed.
pub async fn due_periods(
    tx: &mut Tx,
    property: Uuid,
    closed_through: Date,
) -> Result<Vec<(ReportKind, Date, Date)>, ReportsError> {
    let (vat_registered, tin, return_period, first_closed): (bool, Option<String>, String, Option<Date>) =
        sqlx::query_as(
            "select vat_registered, tin, vat_return_period,
                    (select min(business_date) from daily_stats where property_id = p.id)
             from property p where id = $1",
        )
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;
    let Some(first_closed) = first_closed else { return Ok(vec![]) };
    let from = first_closed.max(months_back(closed_through));
    let mut kinds = vec![ReportKind::Weekly, ReportKind::Monthly, ReportKind::Tdl, ReportKind::Sscl];
    if vat_registered && tin.is_some() {
        kinds.push(ReportKind::Vat);
    }
    kinds.push(ReportKind::ServiceCharge);
    let stored: Vec<(String, Date)> = sqlx::query_as(
        "select kind, period_start from report
         where property_id = $1 and kind <> 'on_demand' and period_end >= $2 and status <> 'failed'",
    )
    .bind(property)
    .bind(from)
    .fetch_all(&mut **tx)
    .await?;
    let mut due = Vec::new();
    for kind in kinds {
        let periods = match kind {
            ReportKind::Weekly => weeks(from, closed_through),
            ReportKind::Vat if return_period == "monthly" => months(from, closed_through, 1),
            ReportKind::Tdl | ReportKind::Sscl | ReportKind::Vat => months(from, closed_through, 3),
            _ => months(from, closed_through, 1),
        };
        due.extend(
            periods
                .into_iter()
                .filter(|(start, _)| !stored.iter().any(|(k, s)| k == kind.as_str() && s == start))
                .map(|(start, end)| (kind, start, end)),
        );
    }
    Ok(due)
}

/// Queues the render of each of `property`'s [`due_periods`] closed through `closed_through`, with its data computed
/// now (a failed one is queued again), and clears the `reports` alert unless a report still failed. Returns how many
/// were queued. The night audit's `render_reports` step runs it.
pub async fn render_due(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    closed_through: Date,
) -> Result<usize, ReportsError> {
    let due = due_periods(tx, property, closed_through).await?;
    for &(kind, from, to) in &due {
        let report = document(tx, property, request(kind, from, to)).await?;
        queue_render(tx, tenant, property, kind, report.period_start, report.period_end, report.template, report.data)
            .await?;
    }
    clear_reports_alert(tx, property).await?;
    Ok(due.len())
}

/// The report of `kind` for the period `from` to `to`, one [`due_periods`] gave.
fn request(kind: ReportKind, from: Date, to: Date) -> RenderRequest {
    let quarter = (u8::from(from.month()) - 1) / 3 + 1;
    match kind {
        ReportKind::Tdl if quarter == 1 => RenderRequest::Tdl(FiscalQuarter { fy_start_year: from.year() - 1, q: 4 }),
        ReportKind::Tdl => RenderRequest::Tdl(FiscalQuarter { fy_start_year: from.year(), q: quarter - 1 }),
        ReportKind::Sscl => RenderRequest::Sscl(CalendarQuarter { year: from.year(), q: quarter }),
        ReportKind::Vat if from.month() == to.month() => {
            RenderRequest::Vat(VatPeriod::Month(from.year(), from.month()))
        }
        ReportKind::Vat => RenderRequest::Vat(VatPeriod::Quarter(from.year(), quarter)),
        ReportKind::ServiceCharge => RenderRequest::ServiceCharge { year: from.year(), month: from.month() },
        ReportKind::Weekly | ReportKind::Monthly | ReportKind::OnDemand => RenderRequest::Summary { from, to },
    }
}

/// `date` 13 months earlier, on the same day or its month's last.
fn months_back(date: Date) -> Date {
    let index = date.year() * 12 + i32::from(u8::from(date.month())) - 1 - MONTHS_BACK;
    let (year, month) = (index.div_euclid(12), month_of(index));
    let day = date.day().min(month.length(year));
    Date::from_calendar_date(year, month, day).unwrap_or(Date::MIN)
}

fn month_of(index: i32) -> Month {
    Month::try_from(u8::try_from(index.rem_euclid(12) + 1).expect("1 to 12")).expect("1 to 12")
}

/// The Monday-to-Sunday weeks whose Sunday is from `from` to `to`.
fn weeks(from: Date, to: Date) -> Vec<(Date, Date)> {
    let mut sunday = from + Duration::days(i64::from((7 - from.weekday().number_days_from_sunday()) % 7));
    let mut weeks = Vec::new();
    while sunday <= to {
        weeks.push((sunday - Duration::days(6), sunday));
        sunday += Duration::weeks(1);
    }
    weeks
}

/// The periods of `length` months (1, or 3 for calendar quarters) whose last day is from `from` to `to`.
fn months(from: Date, to: Date, length: i32) -> Vec<(Date, Date)> {
    let mut periods = Vec::new();
    let mut index = from.year() * 12 + i32::from(u8::from(from.month())) - 1;
    index -= index.rem_euclid(length);
    loop {
        let start = Date::from_calendar_date(index.div_euclid(12), month_of(index), 1).expect("a month's first day");
        let last = index + length - 1;
        let (year, month) = (last.div_euclid(12), month_of(last));
        let end = Date::from_calendar_date(year, month, month.length(year)).expect("a month's last day");
        if end > to {
            return periods;
        }
        if end >= from {
            periods.push((start, end));
        }
        index += length;
    }
}

#[cfg(test)]
mod tests {
    use super::{months, months_back, weeks};
    use time::macros::date;

    #[test]
    fn periods_ending_in_a_range() {
        assert_eq!(
            weeks(date!(2026 - 10 - 19), date!(2026 - 10 - 25)),
            [(date!(2026 - 10 - 19), date!(2026 - 10 - 25))]
        );
        assert_eq!(weeks(date!(2026 - 10 - 26), date!(2026 - 10 - 31)), []);
        assert_eq!(
            months(date!(2026 - 9 - 30), date!(2026 - 10 - 31), 1),
            [(date!(2026 - 9 - 1), date!(2026 - 9 - 30)), (date!(2026 - 10 - 1), date!(2026 - 10 - 31))]
        );
        assert_eq!(
            months(date!(2026 - 11 - 15), date!(2027 - 3 - 31), 3),
            [(date!(2026 - 10 - 1), date!(2026 - 12 - 31)), (date!(2027 - 1 - 1), date!(2027 - 3 - 31)),]
        );
        assert_eq!(months_back(date!(2026 - 10 - 25)), date!(2025 - 9 - 25));
        assert_eq!(months_back(date!(2027 - 3 - 31)), date!(2026 - 2 - 28));
    }
}
