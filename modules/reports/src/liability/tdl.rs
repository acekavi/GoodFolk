//! The Tourism Development Levy of a quarter, in SLTDA's Form A layout.

use crate::ReportsError;
use crate::everyday::ratio;
use crate::liability::lines::revenue_lines;
use db::Tx;
use folio::ReportGroup;
use serde::Serialize;
use std::collections::HashMap;
use time::{Date, Month};
use uuid::Uuid;

/// One rupee in LKR minor units.
const RUPEE: i64 = 100;

/// The base at or under which the levy is 0.5%: Rs 3,000,000.
const SMALL_BASE: i64 = 3_000_000 * RUPEE;

/// A quarter of the April–March financial year: Q1 is April–June, Q2 July–September, Q3 October–December and Q4
/// January–March of the following year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiscalQuarter {
    /// The year the financial year's April falls in.
    pub fy_start_year: i32,
    /// 1 to 4.
    pub q: u8,
}

impl FiscalQuarter {
    /// The quarter's first and last days. `Invalid` for a quarter outside 1 to 4 or a year out of range.
    pub fn range(self) -> Result<(Date, Date), ReportsError> {
        let (year, first, last) = self.months()?;
        let invalid = || ReportsError::Invalid(format!("{} is not a year of the calendar", self.fy_start_year));
        let from = Date::from_calendar_date(year, first, 1).map_err(|_| invalid())?;
        let to = Date::from_calendar_date(year, last, last.length(year)).map_err(|_| invalid())?;
        Ok((from, to))
    }

    /// The period as SLTDA's form prints it, such as "April – June (2026)". `Invalid` for a quarter outside 1 to 4.
    pub fn label(self) -> Result<String, ReportsError> {
        let (year, first, last) = self.months()?;
        Ok(format!("{first} – {last} ({year})"))
    }

    /// The calendar year of the quarter's months, and its first and last month.
    fn months(self) -> Result<(i32, Month, Month), ReportsError> {
        match self.q {
            1 => Ok((self.fy_start_year, Month::April, Month::June)),
            2 => Ok((self.fy_start_year, Month::July, Month::September)),
            3 => Ok((self.fy_start_year, Month::October, Month::December)),
            4 => Ok((self.fy_start_year + 1, Month::January, Month::March)),
            q => Err(ReportsError::Invalid(format!("a quarter is 1 to 4, not {q}"))),
        }
    }
}

/// SLTDA's Form A for tourist hotels. Money is in base LKR minor units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TdlForm {
    /// The property's legal name, or its name when it has none.
    pub name: String,
    pub registration_no: Option<String>,
    pub period_label: String,
    pub room: i64,
    pub food: i64,
    pub bar: i64,
    pub other: i64,
    pub total_turnover: i64,
    /// The deductions: the service charge and VAT within the four sales lines.
    pub service_charge: i64,
    pub vat: i64,
    /// Total Turnover less the deductions.
    pub base: i64,
    /// 100 (1%), or 50 (0.5%) for a base not over Rs 3,000,000.
    pub rate_bp: i32,
    /// The levy on the base, rounded to whole rupees.
    pub tdl: i64,
    /// Some day of the quarter is not closed yet: the figures cover the closed days only.
    pub not_closed: bool,
}

/// `property`'s TDL for `quarter`, from its closed days ([`revenue_lines`]). Each sales line is its net plus its
/// service charge (`SC`) and VAT, passed on or absorbed; the TDL and SSCL passed on to guests are levies collected, not
/// sales. The lines fall under:
/// - **Room:** `room`;
/// - **Food:** `meal_plan`, and the outlets whose report group on the line's date is food;
/// - **Bar:** the outlets in the bar group;
/// - **Other:** `fee`, `laundry`, `misc`, the outlets in the other group, and a `pos:<OUTLET>` naming no outlet.
///
/// The rate follows the quarter's own base, not the property's billing setting. `Invalid` for a quarter outside 1 to
/// 4; `NotFound` for a property not in the tenant.
pub async fn tdl(tx: &mut Tx, property: Uuid, quarter: FiscalQuarter) -> Result<TdlForm, ReportsError> {
    let (from, to) = quarter.range()?;
    let (name, registration_no, business_date): (String, Option<String>, Date) = sqlx::query_as(
        "select coalesce(legal_name, name), tdl_registration_no, business_date from property where id = $1",
    )
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ReportsError::NotFound("property"))?;

    let (mut room, mut food, mut bar, mut other, mut service_charge, mut vat) = (0, 0, 0, 0, 0, 0);
    let mut groups: HashMap<(Uuid, Date), ReportGroup> = HashMap::new();
    for line in revenue_lines(tx, property, from, to).await? {
        let mut sales = line.net;
        for &(ref code, _, _, amount) in &line.taxes {
            match code.as_str() {
                "SC" => service_charge += amount,
                "VAT" => vat += amount,
                _ => continue,
            }
            sales += amount;
        }
        let group = match (line.category.as_str(), line.outlet) {
            ("room", _) => {
                room += sales;
                continue;
            }
            ("meal_plan", _) => ReportGroup::Food,
            (_, Some(outlet)) => match groups.get(&(outlet, line.business_date)) {
                Some(&group) => group,
                None => {
                    let group = folio::outlet_report_group(tx, outlet, line.business_date).await?;
                    groups.insert((outlet, line.business_date), group);
                    group
                }
            },
            _ => ReportGroup::Other,
        };
        match group {
            ReportGroup::Food => food += sales,
            ReportGroup::Bar => bar += sales,
            ReportGroup::Other => other += sales,
        }
    }

    let total_turnover = room + food + bar + other;
    let base = total_turnover - service_charge - vat;
    let rate_bp: i32 = if base <= SMALL_BASE { 50 } else { 100 };
    let tdl = ratio(base.saturating_mul(i64::from(rate_bp)), 10_000 * RUPEE) * RUPEE;
    Ok(TdlForm {
        name,
        registration_no,
        period_label: quarter.label()?,
        room,
        food,
        bar,
        other,
        total_turnover,
        service_charge,
        vat,
        base,
        rate_bp,
        tdl,
        not_closed: to >= business_date,
    })
}
