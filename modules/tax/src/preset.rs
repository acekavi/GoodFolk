//! The Sri Lanka tax and levy preset.

use time::macros::date;

use crate::{Condition, Kind, Presentation, Rule};

/// SC, TDL, SSCL and VAT in calculation order. The tourism development levy drops to 0.5% for a small-turnover
/// property.
#[must_use]
pub fn sri_lanka_preset(tdl_small_turnover: bool) -> Vec<Rule> {
    let rule = |code: &str,
                name: &str,
                kind: Kind,
                rate_bp: u32,
                applies_to: &[&str],
                base: &[&str],
                sequence: i32,
                condition: Condition| Rule {
        code: code.to_owned(),
        name: name.to_owned(),
        kind,
        rate_bp,
        applies_to: applies_to.iter().map(|s| (*s).to_owned()).collect(),
        base: base.iter().map(|s| (*s).to_owned()).collect(),
        sequence,
        presentation: Presentation::OnBill,
        condition,
        effective_from: date!(2026 - 01 - 01),
        effective_to: None,
    };
    vec![
        rule("SC", "Service charge", Kind::ServiceCharge, 1000, &["pos:*"], &["net"], 10, Condition::Always),
        rule(
            "TDL",
            "Tourism development levy",
            Kind::Levy,
            if tdl_small_turnover { 50 } else { 100 },
            &["*"],
            &["net"],
            20,
            Condition::Always,
        ),
        rule(
            "SSCL",
            "Social security contribution levy",
            Kind::Levy,
            250,
            &["*"],
            &["net", "SC", "TDL"],
            30,
            Condition::Always,
        ),
        rule(
            "VAT",
            "Value added tax",
            Kind::Tax,
            1800,
            &["*"],
            &["net", "SC", "TDL", "SSCL"],
            40,
            Condition::VatRegistered,
        ),
    ]
}
