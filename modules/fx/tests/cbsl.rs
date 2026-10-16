use fx::{FxError, parse_cbsl};
use rust_decimal::Decimal;
use std::str::FromStr;
use time::macros::date;

const FIXTURE: &str = include_str!("fixtures/cbsl_usd.html");

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn the_fixture_gives_every_currencys_sell_rates() {
    let rows = parse_cbsl(FIXTURE).unwrap();
    assert_eq!(rows.len(), 9 * 6);
    let usd: Vec<_> = rows.iter().filter(|(c, _, _)| c == "USD").collect();
    assert_eq!(usd.len(), 6);
    assert_eq!(usd[0], &("USD".to_owned(), date!(2026 - 10 - 16), dec("335.6485")));
    assert_eq!(usd[1], &("USD".to_owned(), date!(2026 - 10 - 15), dec("335.7328")));
    assert!(rows.contains(&("EUR".to_owned(), date!(2026 - 10 - 16), dec("377.9703"))));
    assert!(rows.contains(&("GBP".to_owned(), date!(2026 - 10 - 09), dec("445.4237"))));
    assert!(rows.contains(&("JPY".to_owned(), date!(2026 - 10 - 12), dec("2.1305"))));
}

#[test]
fn a_renamed_column_is_a_layout_error() {
    let changed = FIXTURE.replace("Sell Rate (LKR)", "Selling (LKR)");
    assert!(matches!(parse_cbsl(&changed), Err(FxError::Layout(_))));
}

#[test]
fn an_unknown_currency_heading_is_a_layout_error() {
    let changed = FIXTURE.replace("> Yen <", "> Japanese Yen <");
    assert!(matches!(parse_cbsl(&changed), Err(FxError::Layout(_))));
}

#[test]
fn an_unreadable_rate_is_a_layout_error() {
    let changed = FIXTURE.replace("335.6485", "n/a");
    assert!(matches!(parse_cbsl(&changed), Err(FxError::Layout(_))));
}

#[test]
fn a_page_without_currencies_has_no_rows() {
    assert!(parse_cbsl("<html><body>No data</body></html>").unwrap().is_empty());
}
