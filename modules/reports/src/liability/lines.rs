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
    /// `room`, `meal_plan`, `laundry`, `misc`, `fee` or `pos:<OUTLET>`.
    pub category: String,
    /// The outlet a `pos:<OUTLET>` category names; `None` for every other category.
    pub outlet: Option<Uuid>,
    /// Net of the on-bill taxes.
    pub net: i64,
    /// `(code, kind, presentation, amount)` by code, on-bill and absorbed.
    pub taxes: Vec<(String, String, String, i64)>,
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
    if to < from {
        return Err(ReportsError::Invalid("the range ends before it starts".into()));
    }
    let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReportsError::NotFound("property"))?;
    let Some(to) = business_date.previous_day().map(|closed| to.min(closed)).filter(|to| from <= *to) else {
        return Ok(Vec::new());
    };
    let outlets: HashMap<String, Uuid> = sqlx::query_as("select code, id from outlet where property_id = $1")
        .bind(property)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect();
    Ok(audit::revenue_lines(tx, property, from, to)
        .await?
        .into_iter()
        .map(|line| RevenueLine {
            business_date: line.business_date,
            folio_id: line.folio_id,
            outlet: line.category.strip_prefix("pos:").and_then(|code| outlets.get(code).copied()),
            category: line.category,
            net: line.net,
            taxes: line.taxes.into_iter().map(|tax| (tax.code, tax.kind, tax.presentation, tax.amount)).collect(),
        })
        .collect())
}
