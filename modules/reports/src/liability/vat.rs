//! VAT per return period, from the revenue lines, invoices and credit notes of closed business dates, with IRD's
//! Schedule 01 (output tax invoices) and Schedule 04 (credit and debit notes) upload files.

use crate::everyday::ratio;
use crate::liability::lines::revenue_lines;
use crate::{RUPEE, ReportsError};
use db::Tx;
use media_types::{InvoiceKind, InvoiceTax, Purchaser};
use serde::Serialize;
use sqlx::types::Json;
use std::collections::{BTreeMap, HashSet};
use time::{Date, Month};
use uuid::Uuid;

/// IRD's TIN for purchasers without one: their supplies are one combined row per rate.
pub const DUMMY_TIN: &str = "100902885";

/// The name the combined row of invoices without a purchaser TIN carries.
const NO_TIN_PURCHASERS: &str = "Purchasers without a TIN";

/// The code of the VAT tax rule.
const VAT: &str = "VAT";

/// A VAT return period: a calendar month or a calendar quarter (Q1 is January–March).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VatPeriod {
    Month(i32, Month),
    /// The year and its quarter, 1 to 4.
    Quarter(i32, u8),
}

impl VatPeriod {
    /// IRD's period code: `YYQ0` for a quarter and `YYQM` for a month, M being its month within quarter Q, so Q3 2026
    /// is `2630` and August 2026 `2632`.
    pub fn code(self) -> String {
        let (year, quarter, month) = match self {
            Self::Month(year, month) => {
                let at = u8::from(month) - 1;
                (year, at / 3 + 1, at % 3 + 1)
            }
            Self::Quarter(year, q) => (year, q, 0),
        };
        format!("{:02}{quarter}{month}", year.rem_euclid(100))
    }

    /// The period an IRD period code names, of the years 2000 to 2099. `Invalid` for anything else.
    pub fn from_code(code: &str) -> Result<Self, ReportsError> {
        let invalid = || ReportsError::Invalid(format!("{code} is not a VAT period code"));
        let digits: Vec<u8> = code.bytes().map(|byte| byte.wrapping_sub(b'0')).collect();
        let [y1, y2, quarter, month] = digits[..] else { return Err(invalid()) };
        if [y1, y2].iter().any(|digit| *digit > 9) || !(1..=4).contains(&quarter) || month > 3 {
            return Err(invalid());
        }
        let year = 2000 + i32::from(y1 * 10 + y2);
        if month == 0 {
            return Ok(Self::Quarter(year, quarter));
        }
        let month = Month::try_from((quarter - 1) * 3 + month).map_err(|_| invalid())?;
        Ok(Self::Month(year, month))
    }

    /// The period's first and last days. `Invalid` for a quarter outside 1 to 4 or a year out of range.
    pub fn range(self) -> Result<(Date, Date), ReportsError> {
        let (year, first, last) = match self {
            Self::Month(year, month) => (year, month, month),
            Self::Quarter(year, q @ 1..=4) => {
                let first = Month::try_from(q * 3 - 2).expect("a quarter's months are 1 to 12");
                (year, first, first.nth_next(2))
            }
            Self::Quarter(_, q) => return Err(ReportsError::Invalid(format!("a quarter is 1 to 4, not {q}"))),
        };
        let invalid = || ReportsError::Invalid(format!("{year} is not a year of the calendar"));
        let from = Date::from_calendar_date(year, first, 1).map_err(|_| invalid())?;
        let to = Date::from_calendar_date(year, last, last.length(year)).map_err(|_| invalid())?;
        Ok((from, to))
    }
}

/// One row of Schedule 01: an invoice to a purchaser with a TIN at one rate, or the rest of the period's supplies at a
/// rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schedule01Row {
    /// From 1, in the file's order.
    pub serial_no: u32,
    /// The invoice's business date; the period's last day for the combined row.
    pub invoice_date: Date,
    /// The invoice's serial; `INCL01_<period code>` for the combined row.
    pub invoice_no: String,
    pub purchaser_tin: String,
    pub purchaser_name: String,
    pub description: String,
    pub value_of_supply: i64,
    pub vat_amount: i64,
}

/// One row of Schedule 04: a credit note at one rate, with the invoice it credits. Its figures are positive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schedule04Row {
    /// From 1, in the file's order.
    pub serial_no: u32,
    /// The purchaser's TIN, or [`DUMMY_TIN`].
    pub tin: String,
    pub invoice_date: Date,
    pub invoice_no: String,
    pub note_date: Date,
    pub note_no: String,
    pub value: i64,
    pub vat_amount: i64,
}

/// Tax invoices issued in a period for supplies of an earlier one, at one rate: what the TIN rows of Schedule 01 hold
/// beyond the period's own supplies (with its credit notes), which no combined row can carry. Whole LKR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VatTimingDifference {
    pub rate_bp: i32,
    pub value: i64,
    pub vat: i64,
}

/// A VAT return period, in whole LKR.
///
/// The totals are the period's supplies: every revenue line of its closed days with VAT on it, on the bill or
/// absorbed, so a folio never invoiced counts too, and a reversal or transfer reduces the period of its own business
/// date. The value of supply excludes VAT and, under the cascade, includes the service charge and the TDL and SSCL.
/// Credit notes do not reduce the totals again (their reversals already do): Schedule 01's combined row carries them,
/// so Schedule 01 less Schedule 04 is the net, unless there is a timing difference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VatReport {
    pub period_code: String,
    /// The property's TIN, which names the schedule files.
    pub tin: String,
    /// The property's own date today, which dates the schedule files.
    pub generated_on: Date,
    pub value_of_supply: i64,
    pub output_vat: i64,
    /// The credit notes issued in the period (Schedule 04), positive; the totals are already net of them.
    pub credit_notes_value: i64,
    pub credit_notes_vat: i64,
    /// The return's cage A: the value of supply, already net of the period's reversals.
    pub net_value: i64,
    /// The return's cage 0: the output VAT, already net of the period's reversals.
    pub net_vat: i64,
    pub schedule01: Vec<Schedule01Row>,
    pub schedule04: Vec<Schedule04Row>,
    /// By rate, the rates whose TIN invoices exceed the period's supplies; shown as "Tax invoices issued this period
    /// for earlier supplies".
    pub timing_difference: Vec<VatTimingDifference>,
    /// Some day of the period is not closed yet: the figures cover the closed days only.
    pub not_closed: bool,
}

/// The invoices and credit notes of a property (`$1`) issued from `$2` to `$3`, both included, by date and serial,
/// with the serial and date of the invoice each credit note credits. Public so a test can `EXPLAIN` it: `issued_on` is
/// a leakproof date comparison, so `invoice_issued_on_idx` serves it under row-level security.
pub const VAT_DOCUMENTS_SQL: &str = "select i.kind, i.serial, i.issued_on, i.data->'purchaser' as purchaser,
        i.data->'taxes' as taxes, i.total_lkr, o.serial as credited_serial, o.issued_on as credited_on
 from invoice i
 left join invoice o on o.id = i.credits
 where i.property_id = $1 and i.issued_on between $2 and $3
 order by i.issued_on, i.serial";

/// One invoice or credit note of the period, as its snapshot printed it.
#[derive(sqlx::FromRow)]
struct Document {
    #[sqlx(try_from = "String")]
    kind: InvoiceKind,
    serial: String,
    issued_on: Date,
    purchaser: Json<Purchaser>,
    taxes: Json<Vec<InvoiceTax>>,
    total_lkr: i64,
    /// The serial and business date of the invoice a credit note credits.
    credited_serial: Option<String>,
    credited_on: Option<Date>,
}

/// `property`'s VAT for `period`, from its closed days.
///
/// - **The totals, by rate,** come from the period's revenue lines ([`revenue_lines`]) with a `VAT` tax, on the bill
///   or absorbed: the value of supply is the line's net plus every other tax on it, the VAT its VAT. Each is summed in
///   minor units and rounded once to whole rupees per rate.
/// - **Schedule 01:** each VAT rate of a tax invoice (`kind = 'invoice'`) issued in the period to a purchaser with a
///   TIN is a row, from the invoice's snapshot, so it matches the printed invoice. A tax there is VAT by its code, or,
///   on an invoice issued before taxes carried their code, by the name of one of the property's `VAT` rules. Its
///   value of supply is the VAT over the rate ([`by_rate`]).
/// - **Schedule 04:** each VAT rate of a credit note issued in the period is a row, from its snapshot.
/// - **The combined row** of each rate, under [`DUMMY_TIN`], is the rate's total plus its Schedule 04 values less its
///   TIN rows: every supply without a TIN invoice, invoiced or not, so that Schedule 01 less Schedule 04 is the net.
///   When that is not above 0 in both value and VAT (TIN invoices issued this period for supplies of an earlier one),
///   there is no combined row and the pair, negated, is that rate's [`VatTimingDifference`].
///
/// `Invalid` with "This property is not VAT registered", "Set the property's TIN on Settings → Taxes and invoices
/// first", for a period of the kind the property does not file (a month for a quarterly filer), or for a malformed
/// period; `NotFound` for a property not in the tenant.
pub async fn vat(tx: &mut Tx, property: Uuid, period: VatPeriod) -> Result<VatReport, ReportsError> {
    let (vat_registered, tin, return_period, business_date, generated_on): (bool, Option<String>, String, Date, Date) =
        sqlx::query_as(
            "select vat_registered, tin, vat_return_period, business_date, (now() at time zone timezone)::date
             from property where id = $1",
        )
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;
    if !vat_registered {
        return Err(ReportsError::Invalid("This property is not VAT registered".into()));
    }
    let Some(tin) = tin else {
        return Err(ReportsError::Invalid("Set the property's TIN on Settings → Taxes and invoices first".into()));
    };
    match (period, return_period.as_str()) {
        (VatPeriod::Month(..), "quarterly") | (VatPeriod::Quarter(..), "monthly") => {
            return Err(ReportsError::Invalid(format!("This property files its VAT return {return_period}")));
        }
        _ => {}
    }
    let (from, to) = period.range()?;
    let period_code = period.code();

    // The period's supplies by rate, in minor units: (value of supply, VAT).
    let mut supplies: BTreeMap<i32, (i64, i64)> = BTreeMap::new();
    for line in revenue_lines(tx, property, from, to).await? {
        let Some(vat) = line.taxes.iter().find(|tax| tax.code == VAT) else { continue };
        let others: i64 = line.taxes.iter().filter(|tax| tax.code != VAT).map(|tax| tax.amount).sum();
        let sum = supplies.entry(vat.rate_bp).or_default();
        sum.0 += line.net + others;
        sum.1 += vat.amount;
    }
    let totals: BTreeMap<i32, (i64, i64)> = supplies
        .into_iter()
        .map(|(rate_bp, (value, vat))| (rate_bp, (ratio(value, RUPEE), ratio(vat, RUPEE))))
        .collect();

    let vat_names: HashSet<String> =
        sqlx::query_scalar("select distinct name from tax_rule where property_id = $1 and code = $2")
            .bind(property)
            .bind(VAT)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .collect();
    let is_vat = |tax: &InvoiceTax| tax.code == VAT || (tax.code.is_empty() && vat_names.contains(&tax.name));

    let closed_to = business_date.previous_day().map(|closed| to.min(closed));
    let documents: Vec<Document> = match closed_to.filter(|closed_to| from <= *closed_to) {
        Some(closed_to) => {
            sqlx::query_as(VAT_DOCUMENTS_SQL).bind(property).bind(from).bind(closed_to).fetch_all(&mut **tx).await?
        }
        None => Vec::new(),
    };

    let (mut schedule01, mut schedule04) = (Vec::new(), Vec::new());
    for document in documents {
        let taxes: Vec<&InvoiceTax> = document.taxes.iter().filter(|tax| is_vat(tax)).collect();
        let tin = document.purchaser.tin.clone();
        for (rate_bp, value, vat_amount) in by_rate(&taxes, document.total_lkr) {
            match (document.kind, &tin) {
                (InvoiceKind::Invoice, Some(tin)) => schedule01.push((
                    rate_bp,
                    Schedule01Row {
                        serial_no: 0,
                        invoice_date: document.issued_on,
                        invoice_no: document.serial.clone(),
                        purchaser_tin: tin.clone(),
                        purchaser_name: document.purchaser.name.clone(),
                        description: format!("Tax invoice at {}", percent(rate_bp)),
                        value_of_supply: value,
                        vat_amount,
                    },
                )),
                // An invoice without a TIN is in its rate's combined row, through its lines.
                (InvoiceKind::Invoice, None) => {}
                (InvoiceKind::CreditNote, _) => schedule04.push((
                    rate_bp,
                    Schedule04Row {
                        serial_no: 0,
                        tin: tin.clone().unwrap_or_else(|| DUMMY_TIN.into()),
                        invoice_date: document.credited_on.ok_or(ReportsError::NotFound("credited invoice"))?,
                        invoice_no: document
                            .credited_serial
                            .clone()
                            .ok_or(ReportsError::NotFound("credited invoice"))?,
                        note_date: document.issued_on,
                        note_no: document.serial.clone(),
                        // A credit note's snapshot is its invoice's figures negated.
                        value: -value,
                        vat_amount: -vat_amount,
                    },
                )),
            }
        }
    }

    // Each rate's total, plus the credit notes Schedule 04 takes off it, less its TIN rows: the combined row, so that
    // Schedule 01 less Schedule 04 is the return's net. A row that would not be above 0 in both parts is the timing
    // difference instead.
    let mut rest = totals.clone();
    for (rate_bp, row) in &schedule04 {
        let sum = rest.entry(*rate_bp).or_default();
        sum.0 += row.value;
        sum.1 += row.vat_amount;
    }
    for (rate_bp, row) in &schedule01 {
        let sum = rest.entry(*rate_bp).or_default();
        sum.0 -= row.value_of_supply;
        sum.1 -= row.vat_amount;
    }
    let mut schedule01: Vec<Schedule01Row> = schedule01.into_iter().map(|(_, row)| row).collect();
    let mut schedule04: Vec<Schedule04Row> = schedule04.into_iter().map(|(_, row)| row).collect();
    let mut timing_difference = Vec::new();
    for (rate_bp, (value, vat_amount)) in rest {
        if value <= 0 || vat_amount <= 0 {
            if (value, vat_amount) != (0, 0) {
                timing_difference.push(VatTimingDifference { rate_bp, value: -value, vat: -vat_amount });
            }
            continue;
        }
        schedule01.push(Schedule01Row {
            serial_no: 0,
            invoice_date: to,
            invoice_no: format!("INCL01_{period_code}"),
            purchaser_tin: DUMMY_TIN.into(),
            purchaser_name: NO_TIN_PURCHASERS.into(),
            description: format!("Inclusive Invoices at {}", percent(rate_bp)),
            value_of_supply: value,
            vat_amount,
        });
    }
    for (serial_no, row) in (1..).zip(&mut schedule01) {
        row.serial_no = serial_no;
    }
    for (serial_no, row) in (1..).zip(&mut schedule04) {
        row.serial_no = serial_no;
    }

    let value_of_supply = totals.values().map(|sum| sum.0).sum();
    let output_vat = totals.values().map(|sum| sum.1).sum();
    Ok(VatReport {
        period_code,
        tin,
        generated_on,
        value_of_supply,
        output_vat,
        credit_notes_value: schedule04.iter().map(|row| row.value).sum(),
        credit_notes_vat: schedule04.iter().map(|row| row.vat_amount).sum(),
        net_value: value_of_supply,
        net_vat: output_vat,
        schedule01,
        schedule04,
        timing_difference,
        not_closed: to >= business_date,
    })
}

/// A document's VAT by rate, `(rate, value of supply, VAT)`. The value of supply at a rate is its VAT over the rate,
/// rounded to whole rupees, so value × rate gives the VAT, as IRD's e-filing checks each row: the document's total less
/// its VAT would also hold any part of it that bears no VAT. A 0% rate carries what is left of that total.
fn by_rate(taxes: &[&InvoiceTax], total_lkr: i64) -> Vec<(i32, i64, i64)> {
    let mut rates: BTreeMap<i32, i64> = BTreeMap::new();
    for tax in taxes {
        *rates.entry(tax.rate_bp).or_default() += tax.amount_lkr;
    }
    let mut left = total_lkr - rates.values().sum::<i64>();
    let mut rows: Vec<(i32, i64, i64)> = Vec::with_capacity(rates.len());
    for (&rate_bp, &amount) in rates.iter().filter(|(rate_bp, _)| **rate_bp != 0) {
        let value = ratio(amount.saturating_mul(10_000), i64::from(rate_bp));
        left -= value;
        rows.push((rate_bp, value, amount));
    }
    if let Some(&amount) = rates.get(&0) {
        rows.insert(0, (0, left, amount));
    }
    rows
}

/// A rate in basis points as a percentage: `18%`, or `12.5%`.
pub(crate) fn percent(rate_bp: i32) -> String {
    let (whole, fraction) = (rate_bp / 100, rate_bp % 100);
    if fraction == 0 {
        format!("{whole}%")
    } else {
        format!("{whole}.{}%", format!("{fraction:02}").trim_end_matches('0'))
    }
}

/// Schedule 01 as IRD's e-filing takes it: a header row, then each row; dates MM/DD/YYYY, amounts plain integers, a
/// field quoted only when it holds a comma, a quote or a line break, and lines ending CRLF.
pub fn schedule01_csv(report: &VatReport) -> String {
    let mut csv = csv_line(&[
        "Serial No",
        "Invoice Date",
        "Tax Invoice No",
        "Purchaser's TIN",
        "Name of the Purchaser",
        "Description",
        "Value of supply",
        "VAT Amount",
    ]);
    for row in &report.schedule01 {
        csv.push_str(&csv_line(&[
            &row.serial_no.to_string(),
            &us_date(row.invoice_date),
            &row.invoice_no,
            &row.purchaser_tin,
            csv_name(&row.purchaser_name),
            &row.description,
            &row.value_of_supply.to_string(),
            &row.vat_amount.to_string(),
        ]));
    }
    csv
}

/// Schedule 04, as [`schedule01_csv`] writes Schedule 01: every note a credit note issued by the hotel.
pub fn schedule04_csv(report: &VatReport) -> String {
    let mut csv = csv_line(&[
        "Serial No",
        "TIN No",
        "Invoice Date",
        "Invoice No",
        "Tax Credit / Tax Debit Note",
        "Date of Tax Credit / Tax Debit Note",
        "Tax Credit No. / Tax Debit Note No.",
        "Value of Tax Credit Note / Tax Debit Note",
        "VAT Amount",
        "Issued by Me",
    ]);
    for row in &report.schedule04 {
        csv.push_str(&csv_line(&[
            &row.serial_no.to_string(),
            &row.tin,
            &us_date(row.invoice_date),
            &row.invoice_no,
            "Credit",
            &us_date(row.note_date),
            &row.note_no,
            &row.value.to_string(),
            &row.vat_amount.to_string(),
            "Y",
        ]));
    }
    csv
}

/// The name IRD's e-filing expects of a schedule file: `<TIN>_VAT_SCHEDULE01_<period code>_<YYYYMMDD>_ORIGINAL_V1.csv`
/// for Schedule 01 made on `today`.
pub fn schedule_file_name(tin: &str, schedule: u8, period_code: &str, today: Date) -> String {
    format!(
        "{tin}_VAT_SCHEDULE{schedule:02}_{period_code}_{:04}{:02}{:02}_ORIGINAL_V1.csv",
        today.year(),
        u8::from(today.month()),
        today.day()
    )
}

/// `name` without the leading characters (and the spaces before them) a spreadsheet would read as a formula.
fn csv_name(name: &str) -> &str {
    name.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '=' | '+' | '-' | '@'))
}

pub(crate) fn us_date(date: Date) -> String {
    format!("{:02}/{:02}/{:04}", u8::from(date.month()), date.day(), date.year())
}

fn csv_line(fields: &[&str]) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|field| {
            if field.contains([',', '"', '\r', '\n']) {
                format!("\"{}\"", field.replace('"', "\"\""))
            } else {
                (*field).to_owned()
            }
        })
        .collect();
    fields.join(",") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tax(code: &str, rate_bp: i32, amount_lkr: i64) -> InvoiceTax {
        InvoiceTax { code: code.into(), name: code.into(), rate_bp, amount_lkr }
    }

    #[test]
    fn the_value_of_supply_is_the_vat_over_its_rate() {
        let (standard, reduced) = (tax(VAT, 1800, 1_800), tax(VAT, 800, 400));
        assert_eq!(by_rate(&[&standard, &reduced], 17_200), [(800, 5_000, 400), (1800, 10_000, 1_800)]);
        // Rs 200 of the total bears no VAT, so it is in no row.
        assert_eq!(by_rate(&[&standard], 12_000), [(1800, 10_000, 1_800)]);
        // 1,001 / 18% is 5,561.1.
        assert_eq!(by_rate(&[&tax(VAT, 1800, 1_001)], 7_000), [(1800, 5_561, 1_001)]);
        assert_eq!(by_rate(&[&tax(VAT, 1800, -1_800)], -11_800), [(1800, -10_000, -1_800)]);
        // A 0% rate carries what is left.
        assert_eq!(by_rate(&[&standard, &tax(VAT, 0, 0)], 12_000), [(0, 200, 0), (1800, 10_000, 1_800)]);
        assert!(by_rate(&[], 11_800).is_empty());
    }

    #[test]
    fn a_name_never_starts_as_a_formula() {
        assert_eq!(csv_name(" =HYPERLINK(\"x\")"), "HYPERLINK(\"x\")");
        assert_eq!(csv_name("+-@ Ann"), "Ann");
        assert_eq!(csv_name("Ann-Marie = Perera"), "Ann-Marie = Perera");
    }

    #[test]
    fn rates_print_as_percentages() {
        assert_eq!([percent(1800), percent(1250), percent(1205), percent(0)], ["18%", "12.5%", "12.05%", "0%"]);
    }

    #[test]
    fn fields_are_quoted_only_when_they_must_be() {
        assert_eq!(csv_line(&["a", "b,c", "say \"hi\"", "1"]), "a,\"b,c\",\"say \"\"hi\"\"\",1\r\n");
    }
}
