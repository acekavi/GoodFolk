//! The service charge of a month: what was collected on the bill and what was absorbed, by outlet, on rooms and on
//! everything else. A statement for the hotel's own staff distribution: it does not compute who gets what.

use crate::ReportsError;
use crate::everyday::ratio;
use crate::liability::lines::revenue_lines;
use db::Tx;
use serde::Serialize;
use std::collections::BTreeMap;
use time::{Date, Month};
use uuid::Uuid;

/// One rupee in LKR minor units.
const RUPEE: i64 = 100;

/// Service charge in base LKR minor units, each figure whole rupees.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ServiceCharge {
    /// On the bill, paid by the guest.
    pub collected: i64,
    /// Inside the price, borne by the hotel.
    pub absorbed: i64,
}

/// The service charge of one outlet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutletServiceCharge {
    /// The outlet's code.
    pub outlet: String,
    pub collected: i64,
    pub absorbed: i64,
}

/// A month's service charge, each figure its sum rounded once to whole rupees, and the total the sum of the printed
/// figures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceChargeReport {
    /// By outlet code, the outlets with service charge in the month only.
    pub by_outlet: Vec<OutletServiceCharge>,
    /// On the `room` lines.
    pub rooms: ServiceCharge,
    /// On every other line: meal plans, laundry, fees and the rest.
    pub other: ServiceCharge,
    pub total: ServiceCharge,
    /// Some day of the month is not closed yet: its figures cover the closed days only.
    pub not_closed: bool,
}

/// `property`'s service charge (tax code `SC`) in `month` of `year`, from its closed days ([`revenue_lines`]): on-bill
/// as collected, absorbed apart; a `pos:<OUTLET>` line under its outlet, a `room` line under rooms, and every other
/// under other. `Invalid` for a year out of range; `NotFound` for a property not in the tenant.
pub async fn service_charge(
    tx: &mut Tx,
    property: Uuid,
    year: i32,
    month: Month,
) -> Result<ServiceChargeReport, ReportsError> {
    let invalid = || ReportsError::Invalid(format!("{year} is not a year of the calendar"));
    let from = Date::from_calendar_date(year, month, 1).map_err(|_| invalid())?;
    let to = Date::from_calendar_date(year, month, month.length(year)).map_err(|_| invalid())?;
    let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;

    let (mut by_outlet, mut rooms, mut other) =
        (BTreeMap::<String, ServiceCharge>::new(), ServiceCharge::default(), ServiceCharge::default());
    for line in revenue_lines(tx, property, from, to).await? {
        for &(ref code, _, ref presentation, amount) in &line.taxes {
            if code != "SC" {
                continue;
            }
            let sum = match (line.category.as_str(), line.category.strip_prefix("pos:")) {
                ("room", _) => &mut rooms,
                (_, Some(outlet)) => by_outlet.entry(outlet.to_owned()).or_default(),
                _ => &mut other,
            };
            if presentation == "absorbed" {
                sum.absorbed += amount;
            } else {
                sum.collected += amount;
            }
        }
    }

    let rupees = |sum: ServiceCharge| ServiceCharge {
        collected: ratio(sum.collected, RUPEE) * RUPEE,
        absorbed: ratio(sum.absorbed, RUPEE) * RUPEE,
    };
    let (rooms, other) = (rupees(rooms), rupees(other));
    let by_outlet: Vec<OutletServiceCharge> = by_outlet
        .into_iter()
        .map(|(outlet, sum)| {
            let sum = rupees(sum);
            OutletServiceCharge { outlet, collected: sum.collected, absorbed: sum.absorbed }
        })
        .collect();
    let mut total =
        ServiceCharge { collected: rooms.collected + other.collected, absorbed: rooms.absorbed + other.absorbed };
    for outlet in &by_outlet {
        total.collected += outlet.collected;
        total.absorbed += outlet.absorbed;
    }
    Ok(ServiceChargeReport { by_outlet, rooms, other, total, not_closed: to >= business_date })
}
