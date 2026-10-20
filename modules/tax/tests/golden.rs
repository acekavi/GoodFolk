use proptest::prelude::*;
use tax::{
    Breakdown, Condition, Context, Presentation, Rule, TaxError, back_calculate, calculate, sri_lanka_preset, validate,
};
use time::macros::date;

fn ctx(category: &str, vat: bool) -> Context {
    Context {
        date: date!(2026 - 06 - 01),
        category: category.to_owned(),
        vat_registered: vat,
        resident: None,
        currency: "LKR".to_owned(),
    }
}

fn amount(b: &Breakdown, code: &str) -> Option<i64> {
    b.components.iter().find(|c| c.code == code).map(|c| c.amount)
}

fn on_bill_sum(b: &Breakdown) -> i64 {
    b.components.iter().filter(|c| c.presentation == Presentation::OnBill).map(|c| c.amount).sum()
}

#[test]
fn order_a_matches_the_golden_figures() {
    let b = calculate(1_000_000, &sri_lanka_preset(false), &ctx("pos:REST", true)).unwrap();
    assert_eq!(amount(&b, "SC"), Some(100_000));
    assert_eq!(amount(&b, "TDL"), Some(10_000));
    assert_eq!(amount(&b, "SSCL"), Some(27_750));
    assert_eq!(amount(&b, "VAT"), Some(204_795));
    assert_eq!(b.total, 1_342_545);
}

#[test]
fn order_b_matches_the_golden_total() {
    let mut rules = sri_lanka_preset(false);
    for r in &mut rules {
        if r.code == "SSCL" || r.code == "VAT" {
            r.base = vec!["net".into(), "SC".into()];
        }
    }
    let b = calculate(1_000_000, &rules, &ctx("pos:REST", true)).unwrap();
    assert_eq!(b.total, 1_335_500);
}

#[test]
fn a_room_has_no_service_charge() {
    let b = calculate(1_000_000, &sri_lanka_preset(false), &ctx("room", true)).unwrap();
    assert_eq!(amount(&b, "SC"), None);
    assert_eq!(amount(&b, "TDL"), Some(10_000));
    assert_eq!(amount(&b, "SSCL"), Some(25_250));
    assert_eq!(amount(&b, "VAT"), Some(186_345));
}

#[test]
fn no_vat_unless_registered() {
    let b = calculate(1_000_000, &sri_lanka_preset(false), &ctx("room", false)).unwrap();
    assert_eq!(amount(&b, "VAT"), None);
    assert_eq!(b.total, 1_000_000 + 10_000 + 25_250);
}

#[test]
fn small_turnover_tdl_is_50_bp() {
    let b = calculate(1_000_000, &sri_lanka_preset(true), &ctx("room", true)).unwrap();
    assert_eq!(amount(&b, "TDL"), Some(5_000));
}

#[test]
fn effective_dates_switch_rules() {
    let today = date!(2026 - 06 - 01);
    let mut ended = sri_lanka_preset(false);
    ended[1].effective_to = Some(today);
    let mut replacement = sri_lanka_preset(false)[1].clone();
    replacement.rate_bp = 200;
    replacement.effective_from = today;
    replacement.effective_to = None;
    let mut rules = ended;
    rules.push(replacement);
    let c = ctx("room", false);
    let b = calculate(1_000_000, &rules, &c).unwrap();
    assert_eq!(amount(&b, "TDL"), Some(20_000));

    let yesterday = Context { date: date!(2026 - 05 - 31), ..c };
    let b = calculate(1_000_000, &rules, &yesterday).unwrap();
    assert_eq!(amount(&b, "TDL"), Some(10_000));
}

#[test]
fn an_absorbed_component_is_not_in_the_total() {
    let mut rules = sri_lanka_preset(false);
    rules[1].presentation = Presentation::Absorbed;
    let b = calculate(1_000_000, &rules, &ctx("room", false)).unwrap();
    assert_eq!(amount(&b, "TDL"), Some(10_000));
    assert_eq!(b.total, 1_000_000 + on_bill_sum(&b));
    assert_eq!(b.total, 1_000_000 + 25_250);
}

#[test]
fn inclusive_back_calculation_finds_the_net() {
    let b = back_calculate(1_342_545, &sri_lanka_preset(false), &ctx("pos:REST", true)).unwrap();
    assert_eq!(b.net, 1_000_000);
    assert_eq!(b.total, 1_342_545);
}

#[test]
fn negative_net_mirrors_positive() {
    let rules = sri_lanka_preset(false);
    for net in [1, 3, 17, 999, 123_457, 1_000_000] {
        let c = ctx("pos:BAR", true);
        let p = calculate(net, &rules, &c).unwrap();
        let n = calculate(-net, &rules, &c).unwrap();
        assert_eq!(n.total, -p.total);
        for (a, b) in p.components.iter().zip(&n.components) {
            assert_eq!(b.amount, -a.amount);
            assert_eq!(b.base, -a.base);
        }
        let pb = back_calculate(p.total, &rules, &c).unwrap();
        let nb = back_calculate(-p.total, &rules, &c).unwrap();
        assert_eq!(nb.net, -pb.net);
    }
}

#[test]
fn bad_bases_are_rejected() {
    let mut rules = sri_lanka_preset(false);
    rules[0].base = vec!["VAT".into()];
    assert_eq!(calculate(100, &rules, &ctx("pos:BAR", true)).unwrap_err(), TaxError::UnknownBase("VAT".into()));
    let mut rules = sri_lanka_preset(false);
    rules[1].base = vec!["TDL".into()];
    assert_eq!(calculate(100, &rules, &ctx("room", true)).unwrap_err(), TaxError::Cycle("TDL".into()));
    assert_eq!(calculate(i64::MAX, &sri_lanka_preset(false), &ctx("room", true)).unwrap_err(), TaxError::Overflow);
}

#[test]
fn a_same_sequence_base_is_unknown_in_both_orders() {
    let mut a = sri_lanka_preset(false)[1].clone();
    a.code = "A".into();
    let mut b = a.clone();
    b.code = "B".into();
    a.base = vec!["net".into(), "B".into()];
    for rules in [vec![a.clone(), b.clone()], vec![b, a]] {
        assert_eq!(calculate(100, &rules, &ctx("room", true)).unwrap_err(), TaxError::UnknownBase("B".into()));
        assert_eq!(validate(&rules).unwrap_err(), TaxError::UnknownBase("B".into()));
    }
}

#[test]
fn validate_rejects_duplicates_and_high_rates() {
    assert_eq!(validate(&sri_lanka_preset(false)), Ok(()));
    let mut rules = sri_lanka_preset(false);
    rules.push(rules[1].clone());
    assert_eq!(validate(&rules).unwrap_err(), TaxError::DuplicateCode("TDL".into()));
    rules[1].effective_to = Some(date!(2026 - 06 - 01));
    rules[4].effective_from = date!(2026 - 06 - 01);
    assert_eq!(validate(&rules), Ok(()));
    let mut rules = sri_lanka_preset(false);
    rules[0].rate_bp = 10_001;
    assert_eq!(validate(&rules).unwrap_err(), TaxError::RateTooHigh("SC".into()));
    assert_eq!(calculate(1, &rules, &ctx("room", true)).unwrap_err(), TaxError::RateTooHigh("SC".into()));
}

#[test]
fn extreme_grosses_never_panic_or_hang() {
    for rules in [sri_lanka_preset(false), Vec::new()] {
        for gross in [i64::MAX, i64::MAX - 1, i64::MIN + 1] {
            for pos in ["room", "pos:BAR"] {
                let r = back_calculate(gross, &rules, &ctx(pos, true));
                assert!(r.is_ok() || r == Err(TaxError::Overflow), "{gross} {r:?}");
                if let Ok(b) = r {
                    assert_eq!(b.total, gross);
                }
            }
        }
    }
}

#[test]
fn conditions_and_category_patterns() {
    let mut rule = sri_lanka_preset(false)[1].clone();
    rule.condition = Condition::Residency { resident: false };
    let mut c = ctx("room", false);
    assert!(calculate(1000, std::slice::from_ref(&rule), &c).unwrap().components.is_empty());
    c.resident = Some(false);
    assert_eq!(calculate(1000, std::slice::from_ref(&rule), &c).unwrap().components.len(), 1);
    rule.condition = Condition::PaymentCurrency { currencies: vec!["USD".into()] };
    assert!(calculate(1000, std::slice::from_ref(&rule), &c).unwrap().components.is_empty());
    c.currency = "USD".into();
    assert_eq!(calculate(1000, std::slice::from_ref(&rule), &c).unwrap().components.len(), 1);
    rule.applies_to = vec!["pos:BAR".into()];
    assert!(calculate(1000, std::slice::from_ref(&rule), &c).unwrap().components.is_empty());
}

fn rules_for(variant: u8) -> Vec<Rule> {
    let mut rules = sri_lanka_preset(variant & 1 == 1);
    if variant & 2 == 2 {
        rules[1].presentation = Presentation::Absorbed;
    }
    rules
}

proptest! {
    #[test]
    fn back_calculate_is_exact(
        gross in -10_000_000_000_i64..10_000_000_000,
        pos in any::<bool>(),
        vat in any::<bool>(),
        variant in 0_u8..4,
    ) {
        let rules = rules_for(variant);
        let c = ctx(if pos { "pos:REST" } else { "room" }, vat);
        let b = back_calculate(gross, &rules, &c).unwrap();
        prop_assert_eq!(b.total, gross);
        prop_assert_eq!(b.net + on_bill_sum(&b), b.total);
        let sign_ok = if gross >= 0 { b.net >= 0 } else { b.net <= 0 };
        prop_assert!(sign_ok);
        let mirrored = back_calculate(-gross, &rules, &c).unwrap();
        prop_assert_eq!(mirrored.net, -b.net);
        let forward = calculate(b.net, &rules, &c).unwrap();
        prop_assert!((forward.total - gross).abs() <= 3);
    }

    #[test]
    fn any_gross_is_ok_or_overflow(gross in any::<i64>(), pos in any::<bool>()) {
        let c = ctx(if pos { "pos:REST" } else { "room" }, true);
        let r = back_calculate(gross, &sri_lanka_preset(false), &c);
        prop_assert!(matches!(&r, Ok(b) if b.total == gross) || r == Err(TaxError::Overflow));
        let n = calculate(gross, &sri_lanka_preset(false), &c);
        prop_assert!(n.is_ok() || n == Err(TaxError::Overflow));
    }
}
