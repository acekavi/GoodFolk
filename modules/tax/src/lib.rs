//! A pure tax and service charge engine. No I/O: the folio module feeds it rules and a context for every line.
//!
//! All arithmetic is integer, in minor units. Rates are basis points; each component is rounded half-up (symmetric
//! away from zero for negatives) on its own, so a reversal mirrors the original posting exactly.

mod preset;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use preset::sri_lanka_preset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Tax,
    Levy,
    ServiceCharge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    OnBill,
    Absorbed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    Always,
    VatRegistered,
    Residency { resident: bool },
    PaymentCurrency { currencies: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub code: String,
    pub name: String,
    pub kind: Kind,
    pub rate_bp: u32,
    /// `"*"`, a category (`room`, `meal_plan`, `laundry`, `misc`, `fee`), `"pos:*"` or one outlet (`pos:BAR`).
    pub applies_to: Vec<String>,
    /// `"net"` and/or the codes of rules with a lower `sequence`.
    pub base: Vec<String>,
    pub sequence: i32,
    pub presentation: Presentation,
    pub condition: Condition,
    pub effective_from: time::Date,
    pub effective_to: Option<time::Date>,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub date: time::Date,
    pub category: String,
    pub vat_registered: bool,
    pub resident: Option<bool>,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub code: String,
    pub name: String,
    pub kind: Kind,
    pub rate_bp: u32,
    pub base: i64,
    pub amount: i64,
    pub presentation: Presentation,
}

/// `total = net + Σ on_bill amounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown {
    pub net: i64,
    pub components: Vec<Component>,
    pub total: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaxError {
    #[error("rule base names an unknown or later rule: {0}")]
    UnknownBase(String),
    #[error("rule base refers back to the rule itself: {0}")]
    Cycle(String),
    #[error("amount does not fit in 64 bits")]
    Overflow,
}

fn category_matches(pattern: &str, category: &str) -> bool {
    match pattern {
        "*" => true,
        "pos:*" => category.starts_with("pos:"),
        exact => exact == category,
    }
}

fn applies(rule: &Rule, ctx: &Context) -> bool {
    rule.effective_from <= ctx.date
        && rule.effective_to.is_none_or(|to| ctx.date < to)
        && rule.applies_to.iter().any(|p| category_matches(p, &ctx.category))
        && match &rule.condition {
            Condition::Always => true,
            Condition::VatRegistered => ctx.vat_registered,
            Condition::Residency { resident } => ctx.resident == Some(*resident),
            Condition::PaymentCurrency { currencies } => currencies.contains(&ctx.currency),
        }
}

/// `round_half_up(base * rate_bp / 10_000)`, symmetric away from zero.
fn round_amount(base: i64, rate_bp: u32) -> Result<i64, TaxError> {
    let product = i128::from(base) * i128::from(rate_bp);
    let magnitude = product.unsigned_abs();
    let mut quotient = magnitude / 10_000;
    if magnitude % 10_000 >= 5_000 {
        quotient += 1;
    }
    let signed = i128::try_from(quotient).map_err(|_| TaxError::Overflow)?;
    i64::try_from(if product < 0 { -signed } else { signed }).map_err(|_| TaxError::Overflow)
}

/// Applicable rules (category match, condition, effective on `ctx.date`) in `sequence` order; each component's base
/// is the sum of the named parts; `amount = round_half_up(base × rate_bp / 10_000)`.
///
/// A base may name `net` and rules with a lower `sequence`. A named rule that does not apply to this line (for
/// example the service charge on a room) counts as zero.
///
/// # Errors
/// `UnknownBase` for a base naming anything else, `Cycle` for a rule naming itself, `Overflow` when a figure
/// leaves the `i64` range.
pub fn calculate(net: i64, rules: &[Rule], ctx: &Context) -> Result<Breakdown, TaxError> {
    let mut ordered: Vec<&Rule> = rules.iter().filter(|r| applies(r, ctx)).collect();
    ordered.sort_by_key(|r| r.sequence);

    let mut amounts: HashMap<&str, i64> = HashMap::new();
    let mut components = Vec::with_capacity(ordered.len());
    let mut total = i128::from(net);
    for rule in ordered {
        let mut base = 0_i128;
        for part in &rule.base {
            if part == "net" {
                base += i128::from(net);
            } else if *part == rule.code {
                return Err(TaxError::Cycle(part.clone()));
            } else if let Some(amount) = amounts.get(part.as_str()) {
                base += i128::from(*amount);
            } else if rules.iter().any(|r| r.code == *part && r.sequence < rule.sequence) {
                // Defined earlier but not applicable to this line.
            } else {
                return Err(TaxError::UnknownBase(part.clone()));
            }
        }
        let base = i64::try_from(base).map_err(|_| TaxError::Overflow)?;
        let amount = round_amount(base, rule.rate_bp)?;
        amounts.insert(&rule.code, amount);
        if rule.presentation == Presentation::OnBill {
            total += i128::from(amount);
        }
        components.push(Component {
            code: rule.code.clone(),
            name: rule.name.clone(),
            kind: rule.kind,
            rate_bp: rule.rate_bp,
            base,
            amount,
            presentation: rule.presentation,
        });
    }
    let total = i64::try_from(total).map_err(|_| TaxError::Overflow)?;
    Ok(Breakdown { net, components, total })
}

/// The breakdown whose `total` equals `gross` exactly, for a tax-inclusive price.
///
/// Rounding makes `total(net)` a non-decreasing step function that can skip values, so no net maps to some
/// grosses. The method: binary search the largest net whose `calculate(net).total <= gross`, keep that search's
/// components, and set `net = gross - Σ on_bill amounts` so the remainder lands in net. The result therefore always
/// satisfies `net + Σ on_bill == total == gross`, and a gross that some net reaches exactly returns that net. A
/// negative gross is the mirror of its absolute value.
///
/// # Errors
/// As [`calculate`]; `Overflow` also when the result does not fit in `i64`.
pub fn back_calculate(gross: i64, rules: &[Rule], ctx: &Context) -> Result<Breakdown, TaxError> {
    if gross < 0 {
        let positive = back_calculate(gross.checked_neg().ok_or(TaxError::Overflow)?, rules, ctx)?;
        let neg = |v: i64| v.checked_neg().ok_or(TaxError::Overflow);
        let components = positive
            .components
            .into_iter()
            .map(|c| Ok(Component { base: neg(c.base)?, amount: neg(c.amount)?, ..c }))
            .collect::<Result<Vec<_>, TaxError>>()?;
        return Ok(Breakdown { net: neg(positive.net)?, components, total: neg(positive.total)? });
    }

    // Surfaces rule errors up front; afterwards an error inside the search can only be Overflow (too large).
    calculate(0, rules, ctx)?;
    let (mut low, mut high) = (0_i64, gross);
    while low < high {
        let mid = low + (high - low + 1) / 2;
        match calculate(mid, rules, ctx) {
            Ok(b) if b.total <= gross => low = mid,
            Ok(_) | Err(TaxError::Overflow) => high = mid - 1,
            Err(e) => return Err(e),
        }
    }
    let found = calculate(low, rules, ctx)?;
    let on_bill = found
        .components
        .iter()
        .filter(|c| c.presentation == Presentation::OnBill)
        .try_fold(0_i128, |acc, c| acc.checked_add(i128::from(c.amount)))
        .ok_or(TaxError::Overflow)?;
    let net = i64::try_from(i128::from(gross) - on_bill).map_err(|_| TaxError::Overflow)?;
    Ok(Breakdown { net, components: found.components, total: gross })
}
