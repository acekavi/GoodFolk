//! The one reader of closed-date revenue the liability reports share.

use crate::ReportsError;
use db::Tx;
use std::collections::HashMap;
use time::Date;
use uuid::Uuid;

/// One folio line, or one item of a POS bill settled at its outlet, on a closed business date. Money is in base LKR
/// minor units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevenueLine {
    pub business_date: Date,
    /// `None` for an item of a bill settled at its outlet.
    pub folio_id: Option<Uuid>,
    /// The bill of an item settled at its outlet; `None` for a folio line.
    pub pos_bill_id: Option<Uuid>,
    /// `room`, `meal_plan`, `laundry`, `misc`, `fee` or `pos:<OUTLET>`.
    pub category: String,
    /// The outlet a `pos:<OUTLET>` category names; `None` for every other category.
    pub outlet: Option<Uuid>,
    /// Net of the on-bill taxes.
    pub net: i64,
    /// By code, on-bill and absorbed.
    pub taxes: Vec<audit::RevenueTax>,
}

/// The revenue of `property`'s closed business dates from `from` to `to`, both included: those before the property's
/// business date, by the night audit's own rules ([`audit::revenue_lines`]). The open days of the range are left out,
/// never estimated. `Invalid` when the range ends before it starts; `NotFound` for a property not in the tenant.
pub async fn revenue_lines(
    tx: &mut Tx,
    property: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<RevenueLine>, ReportsError> {
    let Some(to) = closed_to(tx, property, from, to).await? else {
        return Ok(Vec::new());
    };
    let lines = audit::revenue_lines(tx, property, from, to).await?;
    with_outlets(tx, property, lines).await
}

/// The revenue of the lines of `folios`, from `from` to `to` (both included), clamped to the closed days as
/// [`revenue_lines`] is: no POS bills settled at their outlet. `Invalid` when the range ends before it starts;
/// `NotFound` for a property not in the tenant.
pub async fn folio_revenue_lines(
    tx: &mut Tx,
    property: Uuid,
    folios: &[Uuid],
    from: Date,
    to: Date,
) -> Result<Vec<RevenueLine>, ReportsError> {
    let Some(to) = closed_to(tx, property, from, to).await? else {
        return Ok(Vec::new());
    };
    let lines = audit::folio_revenue_lines(tx, property, folios, from, to).await?;
    with_outlets(tx, property, lines).await
}

/// `to`, or the property's last closed date when that is earlier; `None` when no day from `from` is closed.
async fn closed_to(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Option<Date>, ReportsError> {
    if to < from {
        return Err(ReportsError::Invalid("the range ends before it starts".into()));
    }
    let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;
    Ok(business_date.previous_day().map(|closed| to.min(closed)).filter(|to| from <= *to))
}

/// The night audit's lines, with the outlet each `pos:<OUTLET>` category names.
async fn with_outlets(
    tx: &mut Tx,
    property: Uuid,
    lines: Vec<audit::RevenueLine>,
) -> Result<Vec<RevenueLine>, ReportsError> {
    let outlets: HashMap<String, Uuid> = sqlx::query_as("select code, id from outlet where property_id = $1")
        .bind(property)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect();
    Ok(lines
        .into_iter()
        .map(|line| RevenueLine {
            business_date: line.business_date,
            folio_id: line.folio_id,
            pos_bill_id: line.pos_bill_id,
            outlet: line.category.strip_prefix("pos:").and_then(|code| outlets.get(code).copied()),
            category: line.category,
            net: line.net,
            taxes: line.taxes,
        })
        .collect())
}
