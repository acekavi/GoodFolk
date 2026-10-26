//! The data each report's PDF template reads: the report computed from closed days, its head (the property's legal
//! name, the title and the period), and every printed figure formatted here, so the templates only lay text out and
//! the rounding rules stay in Rust. Money prints as whole rupees, `Rs 1,234,567`: the everyday figures (base LKR minor
//! units) are rounded once each, the liability figures are already whole rupees.

use crate::ReportsError;
use crate::everyday::{Dimension, ListKind, RangeKind, Summary, daily_list, ratio, summary};
use crate::liability::service_charge::{ServiceChargeReport, service_charge};
use crate::liability::sscl::{CalendarQuarter, SsclReport, sscl};
use crate::liability::tdl::{FiscalQuarter, TdlForm, tdl};
use crate::liability::vat::{VatPeriod, VatReport, percent, us_date, vat};
use db::Tx;
use media_types::ReportTemplate;
use serde_json::{Value, json};
use time::{Date, Month};
use uuid::Uuid;

/// One rupee in LKR minor units.
const RUPEE: i64 = 100;

/// What a liability report says when some day of its period is not closed yet.
const PERIOD_NOT_CLOSED: &str = "Some days of this period are not closed yet: the figures cover the closed days only.";

/// A report to print, with what its on-screen query takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderRequest {
    /// The everyday reports from `from` to `to`, both included.
    Summary {
        from: Date,
        to: Date,
    },
    DailyList {
        date: Date,
        kind: ListKind,
    },
    Tdl(FiscalQuarter),
    Sscl(CalendarQuarter),
    Vat(VatPeriod),
    ServiceCharge {
        year: i32,
        month: Month,
    },
}

impl RenderRequest {
    pub fn template(self) -> ReportTemplate {
        match self {
            Self::Summary { .. } => ReportTemplate::Summary,
            Self::DailyList { .. } => ReportTemplate::DailyList,
            Self::Tdl(_) => ReportTemplate::TdlFormA,
            Self::Sscl(_) => ReportTemplate::Sscl,
            Self::Vat(_) => ReportTemplate::Vat,
            Self::ServiceCharge { .. } => ReportTemplate::ServiceCharge,
        }
    }
}

/// A report ready to queue: its template, its period and the data the template reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportDocument {
    pub template: ReportTemplate,
    pub period_start: Date,
    pub period_end: Date,
    pub data: Value,
}

/// `property`'s report for `request`, from its closed days, as its template prints it. Refused as the report itself
/// is: `Invalid` for a malformed period or range, or VAT for a property that is not registered or has no TIN;
/// `NotFound` for a property not in the tenant.
pub async fn document(tx: &mut Tx, property: Uuid, request: RenderRequest) -> Result<ReportDocument, ReportsError> {
    let template = request.template();
    let (period_start, period_end, data) = match request {
        RenderRequest::Summary { from, to } => {
            let report = summary(tx, property, from, to).await?;
            (from, to, summary_data(&legal_name(tx, property).await?, from, to, &report))
        }
        RenderRequest::DailyList { date, kind } => {
            let rows = daily_list(tx, property, date, kind).await?;
            (date, date, daily_list_data(&legal_name(tx, property).await?, date, kind, &rows))
        }
        RenderRequest::Tdl(quarter) => {
            let (from, to) = quarter.range()?;
            (from, to, tdl_data(&tdl(tx, property, quarter).await?))
        }
        RenderRequest::Sscl(quarter) => {
            let (from, to) = quarter.range()?;
            let report = sscl(tx, property, quarter).await?;
            (from, to, sscl_data(&legal_name(tx, property).await?, quarter, &report))
        }
        RenderRequest::Vat(period) => {
            let (from, to) = period.range()?;
            let report = vat(tx, property, period).await?;
            (from, to, vat_data(&legal_name(tx, property).await?, period, &report))
        }
        RenderRequest::ServiceCharge { year, month } => {
            let report = service_charge(tx, property, year, month).await?;
            let from = Date::from_calendar_date(year, month, 1).expect("the report checked the year");
            let to = Date::from_calendar_date(year, month, month.length(year)).expect("the report checked the year");
            (from, to, service_charge_data(&legal_name(tx, property).await?, year, month, &report))
        }
    };
    Ok(ReportDocument { template, period_start, period_end, data })
}

/// The property's legal name, or its name when it has none.
async fn legal_name(tx: &mut Tx, property: Uuid) -> Result<String, ReportsError> {
    sqlx::query_scalar("select coalesce(legal_name, name) from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))
}

/// What every template prints at its head.
fn head(title: &str, subtitle: Option<&str>, property: &str, period: String, not_closed: Option<String>) -> Value {
    json!({ "title": title, "subtitle": subtitle, "property": property, "period": period, "not_closed": not_closed })
}

/// `head` with `body`'s fields after it.
fn with(mut head: Value, body: Value) -> Value {
    if let (Some(head), Value::Object(body)) = (head.as_object_mut(), body) {
        head.extend(body);
    }
    head
}

fn summary_data(property: &str, from: Date, to: Date, report: &Summary) -> Value {
    let title = match RangeKind::of(from, to) {
        RangeKind::Month => "Monthly report",
        RangeKind::Week => "Weekly report",
        RangeKind::Custom => "Summary report",
    };
    let not_closed = (!report.not_closed.is_empty()).then(|| format!("Not closed yet: {}", runs(&report.not_closed)));
    let last_year = report.last_year.as_ref();
    let headline = [
        ("Occupancy", percent(report.headline.occupancy_bp), last_year.map(|h| percent(h.occupancy_bp))),
        ("ADR", rupees_of(report.headline.adr), last_year.map(|h| rupees_of(h.adr))),
        ("RevPAR", rupees_of(report.headline.revpar), last_year.map(|h| rupees_of(h.revpar))),
        ("Room nights", grouped(report.headline.room_nights), last_year.map(|h| grouped(h.room_nights))),
        ("Total revenue", rupees_of(report.headline.total_revenue), last_year.map(|h| rupees_of(h.total_revenue))),
    ]
    .map(|(label, value, last_year)| json!({ "label": label, "value": value, "last_year": last_year }));
    let days: Vec<Value> = report
        .days
        .iter()
        .map(|day| {
            json!({
                "date": day_of(day.business_date),
                "rooms": format!("{} / {}", day.rooms_sold, day.rooms_available),
                "occupancy": percent(day.occupancy_bp),
                "adr": rupees_of(day.adr),
                "revpar": rupees_of(day.revpar),
                "room_revenue": rupees_of(day.room_revenue),
                "total_revenue": rupees_of(day.total_revenue),
            })
        })
        .collect();
    let breakdowns: Vec<Value> = report
        .breakdowns
        .iter()
        .map(|(dimension, keys)| {
            let title = match dimension {
                Dimension::Segment => "By segment",
                Dimension::Source => "By source",
                Dimension::Residency => "By residency",
                Dimension::RoomType => "By room type",
                Dimension::Outlet => "By outlet",
            };
            let rows: Vec<Value> = keys
                .iter()
                .map(|key| {
                    json!({
                        "key": key.key,
                        "room_nights": grouped(key.room_nights),
                        "room_revenue": rupees_of(key.room_revenue),
                        "revenue": rupees_of(key.revenue),
                    })
                })
                .collect();
            json!({ "title": title, "rows": rows })
        })
        .collect();
    with(
        head(title, None, property, span(from, to), not_closed),
        json!({ "headline": headline, "days": days, "breakdowns": breakdowns }),
    )
}

fn daily_list_data(property: &str, date: Date, kind: ListKind, rows: &[crate::ListRow]) -> Value {
    let title = match kind {
        ListKind::Arrivals => "Arrivals",
        ListKind::Departures => "Departures",
        ListKind::InHouse => "In house",
    };
    let stays: Vec<Value> = rows
        .iter()
        .map(|row| {
            let guests =
                if row.children > 0 { format!("{} + {}", row.adults, row.children) } else { row.adults.to_string() };
            json!({
                "room": row.room_number.as_deref().unwrap_or("Unassigned"),
                "guest": row.guest,
                "confirmation": row.confirmation,
                "arrival": day_of(row.arrival),
                "departure": day_of(row.departure),
                "status": sentence(row.status.as_str()),
                "guests": guests,
            })
        })
        .collect();
    let count = if rows.len() == 1 { "1 stay".to_owned() } else { format!("{} stays", rows.len()) };
    with(head(title, None, property, day_of(date), None), json!({ "rows": stays, "count": count }))
}

fn tdl_data(form: &TdlForm) -> Value {
    let line = |label: &str, value: i64| json!({ "label": label, "value": rupees_of(value) });
    let credit_note = form.credit_exceeds_sales.then_some("The quarter's reversals exceed its sales: no TDL is due.");
    with(
        head(
            "Form A",
            Some("Tourism Development Levy: tourist hotels"),
            &form.name,
            form.period_label.clone(),
            form.not_closed.then(|| PERIOD_NOT_CLOSED.to_owned()),
        ),
        json!({
            "registration_no": form.registration_no,
            "sales": [
                line("Room Sales", form.room),
                line("Food Sales", form.food),
                line("Bar Sales", form.bar),
                line("Other Sales", form.other),
            ],
            "total_turnover": rupees_of(form.total_turnover),
            "deductions": [line("Service Charge", form.service_charge), line("VAT", form.vat)],
            "base": rupees_of(form.base),
            "rate": percent(form.rate_bp),
            "tdl": rupees_of(form.tdl),
            "credit_note": credit_note,
        }),
    )
}

fn sscl_data(property: &str, quarter: CalendarQuarter, report: &SsclReport) -> Value {
    let totals = &report.quarter_totals;
    let row = |label: &str, month: fn(&crate::SsclMonth) -> i64, total: i64, strong: bool| {
        let mut values: Vec<String> = report.months.iter().map(|m| rupees_of(month(m))).collect();
        values.push(rupees_of(total));
        json!({ "label": label, "values": values, "strong": strong })
    };
    let mut due: Vec<String> = report.months.iter().map(|m| day_of(m.due)).collect();
    due.push(String::new());
    let rows = [
        row("Turnover", |m| m.turnover, totals.turnover, false),
        row(
            "Less: exempt foreign-currency turnover settled this month",
            |m| m.exempt_same_month,
            totals.exempt_same_month,
            false,
        ),
        row(
            "Less: exemption for folios settled this month (turnover of earlier months)",
            |m| m.exempt_earlier_months,
            totals.exempt_earlier_months,
            false,
        ),
        row("Less: bad debts", |m| m.bad_debts, totals.bad_debts, false),
        row("Liable turnover", |m| m.liable, totals.liable, true),
        row("Levy at 2.5%", |m| m.levy, totals.levy, true),
        json!({ "label": "Payment due", "values": due, "strong": false }),
    ];
    let foreign_cash = row("Foreign-currency cash (not exempt)", |m| m.foreign_cash, totals.foreign_cash, false);
    let months: Vec<String> = report.months.iter().map(|m| m.month.to_string()).collect();
    let period = format!("Q{} {}: {} – {}", quarter.q, quarter.year, months[0], months[2]);
    let not_closed = report.months.iter().any(|m| m.not_closed).then(|| PERIOD_NOT_CLOSED.to_owned());
    with(
        head("SSCL", Some("Social Security Contribution Levy"), property, period, not_closed),
        json!({
            "months": months,
            "rows": rows,
            "foreign_cash": { "label": foreign_cash["label"], "values": foreign_cash["values"] },
            "return_due": day_of(report.return_due),
            "exemption_note": report.exemption_note,
            "bad_debts_note": "Bad debts: none until the system writes off debts.",
            "method_note": report.method_note,
        }),
    )
}

fn vat_data(property: &str, period: VatPeriod, report: &VatReport) -> Value {
    let label = match period {
        VatPeriod::Month(year, month) => format!("{month} {year} ({})", report.period_code),
        VatPeriod::Quarter(year, q) => format!("Q{q} {year} ({})", report.period_code),
    };
    let figure = |label: &str, value: i64| json!({ "label": label, "value": rupees(value) });
    let schedule01: Vec<Value> = report
        .schedule01
        .iter()
        .map(|row| {
            json!({
                "serial_no": row.serial_no.to_string(),
                "date": us_date(row.invoice_date),
                "invoice_no": row.invoice_no,
                "tin": row.purchaser_tin,
                "name": row.purchaser_name,
                "description": row.description,
                "value": rupees(row.value_of_supply),
                "vat": rupees(row.vat_amount),
            })
        })
        .collect();
    let schedule04: Vec<Value> = report
        .schedule04
        .iter()
        .map(|row| {
            json!({
                "serial_no": row.serial_no.to_string(),
                "tin": row.tin,
                "invoice_no": row.invoice_no,
                "invoice_date": us_date(row.invoice_date),
                "note_no": row.note_no,
                "note_date": us_date(row.note_date),
                "value": rupees(row.value),
                "vat": rupees(row.vat_amount),
            })
        })
        .collect();
    let timing_difference: Vec<Value> = report
        .timing_difference
        .iter()
        .map(|row| json!({ "rate": percent(row.rate_bp), "value": rupees(row.value), "vat": rupees(row.vat) }))
        .collect();
    with(
        head("VAT return", None, property, label, report.not_closed.then(|| PERIOD_NOT_CLOSED.to_owned())),
        json!({
            "tin": report.tin,
            "period_code": report.period_code,
            "figures": [
                figure("Value of supply", report.value_of_supply),
                figure("Output VAT", report.output_vat),
                figure("Credit notes issued: value (already in the figures above)", report.credit_notes_value),
                figure("Credit notes issued: VAT (already in the figures above)", report.credit_notes_vat),
                figure("Net value of supply (cage A)", report.net_value),
                figure("Net output VAT (cage 0)", report.net_vat),
            ],
            "schedule01": schedule01,
            "schedule04": schedule04,
            "timing_difference": timing_difference,
        }),
    )
}

fn service_charge_data(property: &str, year: i32, month: Month, report: &ServiceChargeReport) -> Value {
    let row = |label: String, collected: i64, absorbed: i64| json!({ "label": label, "collected": rupees_of(collected), "absorbed": rupees_of(absorbed) });
    let mut rows: Vec<Value> = report
        .by_outlet
        .iter()
        .map(|outlet| row(format!("Outlet {}", outlet.outlet), outlet.collected, outlet.absorbed))
        .collect();
    rows.push(row("Rooms".into(), report.rooms.collected, report.rooms.absorbed));
    rows.push(row("Everything else".into(), report.other.collected, report.other.absorbed));
    with(
        head(
            "Service charge",
            None,
            property,
            format!("{month} {year}"),
            report.not_closed.then(|| PERIOD_NOT_CLOSED.to_owned()),
        ),
        json!({
            "rows": rows,
            "total": { "collected": rupees_of(report.total.collected), "absorbed": rupees_of(report.total.absorbed) },
        }),
    )
}

/// `n` with its thousands grouped: `-1,234,567`.
fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Whole rupees as printed: `Rs 1,234,567`, or `-Rs 1,234`.
fn rupees(whole: i64) -> String {
    let sign = if whole < 0 { "-" } else { "" };
    format!("{sign}Rs {}", grouped(whole).trim_start_matches('-'))
}

/// Base LKR minor units as whole rupees, rounded half away from zero.
fn rupees_of(minor: i64) -> String {
    rupees(ratio(minor, RUPEE))
}

/// `19 Oct 2026`.
fn day_of(date: Date) -> String {
    format!("{} {} {}", date.day(), &date.month().to_string()[..3], date.year())
}

/// `19 Oct 2026 – 25 Oct 2026`, or the one day.
fn span(from: Date, to: Date) -> String {
    if from == to { day_of(from) } else { format!("{} – {}", day_of(from), day_of(to)) }
}

/// Ascending dates as their runs of consecutive days: `1 Oct 2026, 3 Oct 2026 – 5 Oct 2026`.
fn runs(dates: &[Date]) -> String {
    let mut runs: Vec<(Date, Date)> = Vec::new();
    for &date in dates {
        match runs.last_mut() {
            Some((_, last)) if last.next_day() == Some(date) => *last = date,
            _ => runs.push((date, date)),
        }
    }
    runs.into_iter().map(|(from, to)| span(from, to)).collect::<Vec<_>>().join(", ")
}

/// `checked_in` as `Checked in`.
fn sentence(value: &str) -> String {
    let words = value.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn money_prints_as_whole_rupees() {
        assert_eq!(rupees(3_453_513), "Rs 3,453,513");
        assert_eq!(rupees(-1_234), "-Rs 1,234");
        assert_eq!(rupees(0), "Rs 0");
        assert_eq!(rupees(999), "Rs 999");
        assert_eq!(rupees_of(345_351_300), "Rs 3,453,513");
        // Rounded once, half away from zero.
        assert_eq!(rupees_of(1_234_550), "Rs 12,346");
        assert_eq!(rupees_of(-1_234_550), "-Rs 12,346");
        assert_eq!(rupees_of(1_234_549), "Rs 12,345");
        assert_eq!(grouped(1_000), "1,000");
    }

    #[test]
    fn dates_print_as_days_and_runs() {
        assert_eq!(day_of(date!(2026 - 10 - 05)), "5 Oct 2026");
        assert_eq!(span(date!(2026 - 10 - 19), date!(2026 - 10 - 25)), "19 Oct 2026 – 25 Oct 2026");
        let dates = [date!(2026 - 10 - 01), date!(2026 - 10 - 03), date!(2026 - 10 - 04), date!(2026 - 10 - 05)];
        assert_eq!(runs(&dates), "1 Oct 2026, 3 Oct 2026 – 5 Oct 2026");
        assert_eq!(sentence("checked_in"), "Checked in");
    }

    /// The TDL fixture: SLTDA's worked example prints as its form does.
    #[test]
    fn the_tdl_fixture_prints_its_form_a_figures() {
        let form = TdlForm {
            name: "Galle Fort Hotels (Pvt) Ltd".into(),
            registration_no: None,
            period_label: "July – September (2026)".into(),
            room: 55_800 * RUPEE,
            food: 237_973 * RUPEE,
            bar: 3_183_537 * RUPEE,
            other: 0,
            total_turnover: 3_477_310 * RUPEE,
            service_charge: 23_797 * RUPEE,
            vat: 0,
            base: 3_453_513 * RUPEE,
            rate_bp: 100,
            tdl: 34_535 * RUPEE,
            credit_exceeds_sales: false,
            not_closed: false,
        };
        let data = tdl_data(&form);
        assert_eq!(data["title"], "Form A");
        assert_eq!(data["property"], "Galle Fort Hotels (Pvt) Ltd");
        assert_eq!(data["period"], "July – September (2026)");
        assert_eq!(data["not_closed"], Value::Null);
        assert_eq!(data["sales"][2], json!({ "label": "Bar Sales", "value": "Rs 3,183,537" }));
        assert_eq!(data["total_turnover"], "Rs 3,477,310");
        assert_eq!(data["deductions"][0], json!({ "label": "Service Charge", "value": "Rs 23,797" }));
        assert_eq!(
            (&data["base"], &data["rate"], &data["tdl"]),
            (&json!("Rs 3,453,513"), &json!("1%"), &json!("Rs 34,535"))
        );
    }
}
