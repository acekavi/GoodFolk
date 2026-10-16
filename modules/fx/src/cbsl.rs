//! The Central Bank of Sri Lanka's telegraphic transfer (TT) rates, read from its results page.

use crate::{FxError, RateSource};
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;
use time::macros::format_description;
use time::{Date, Duration as Days};

const URL: &str = "https://www.cbsl.gov.lk/cbsl_custom/exratestt/exrates_resultstt.php";

/// The bank's name for each currency in the lookup form (`chk_cur[]=<code>~<name>`), which is also the heading above
/// each currency's table in the results.
const CURRENCIES: [(&str, &str); 9] = [
    ("AUD", "Australian Dollar"),
    ("CAD", "Canadian Dollar"),
    ("CHF", "Swiss Franc"),
    ("CNY", "Renminbi"),
    ("EUR", "Euro"),
    ("GBP", "British Pound"),
    ("JPY", "Yen"),
    ("SGD", "Singapore Dollar"),
    ("USD", "United States Dollar"),
];

/// How far back the request looks, so a weekend or a holiday still finds the last published rate.
const WINDOW_DAYS: i64 = 7;

pub struct Cbsl {
    client: reqwest::Client,
    url: String,
}

impl Cbsl {
    pub fn new() -> Result<Self, FxError> {
        Self::with_url(URL)
    }

    /// Reads from `url` instead of the bank's page (tests).
    pub fn with_url(url: &str) -> Result<Self, FxError> {
        crate::install_tls_provider();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("GoodFolk/", env!("CARGO_PKG_VERSION"), " (exchange-rate import)"))
            .build()
            .map_err(|error| FxError::Source(error.to_string()))?;
        Ok(Self { client, url: url.to_owned() })
    }
}

fn iso(date: Date) -> String {
    date.format(format_description!("[year]-[month]-[day]")).expect("a date formats")
}

#[async_trait]
impl RateSource for Cbsl {
    async fn tt_selling(&self, currencies: &[String], on: Date) -> Result<Vec<(String, Date, Decimal)>, FxError> {
        let body = request_body(currencies, on);
        let response = self
            .client
            .post(&self.url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|error| FxError::Source(error.to_string()))?;
        let html = response.text().await.map_err(|error| FxError::Source(error.to_string()))?;
        Ok(newest_on_or_before(parse_cbsl(&html)?, currencies, on))
    }
}

/// The form the bank's lookup page posts: the window from a week before `on` to `on`, and one checkbox per currency.
fn request_body(currencies: &[String], on: Date) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("lookupPage", "lookup_daily_exchange_rates.php")
        .append_pair("startRange", "2006-11-11")
        .append_pair("rangeType", "dates")
        .append_pair("txtStart", &iso(on - Days::days(WINDOW_DAYS)))
        .append_pair("txtEnd", &iso(on));
    // A currency the bank does not publish is simply not asked for; the import reports it as missing.
    for (code, name) in CURRENCIES.iter().filter(|(code, _)| currencies.iter().any(|c| c == code)) {
        form.append_pair("chk_cur[]", &format!("{code}~{name}"));
    }
    form.append_pair("submit_button", "Submit");
    form.finish()
}

/// For each of `currencies`, its newest row dated on or before `on`.
fn newest_on_or_before(
    rows: Vec<(String, Date, Decimal)>,
    currencies: &[String],
    on: Date,
) -> Vec<(String, Date, Decimal)> {
    currencies
        .iter()
        .filter_map(|code| {
            rows.iter().filter(|(c, date, _)| c == code && *date <= on).max_by_key(|(_, date, _)| *date).cloned()
        })
        .collect()
}

/// Every Sell Rate on the page, as `(currency, date, rate)`. Any deviation from the layout seen on 2026-10-16 (a
/// heading the bank has not told us about, renamed columns, a malformed date or rate) is [`FxError::Layout`] rather
/// than a guess, so a redesign raises the alert instead of storing wrong money.
pub fn parse_cbsl(html: &str) -> Result<Vec<(String, Date, Decimal)>, FxError> {
    let layout = |what: &str| FxError::Layout(what.to_owned());
    let mut rows = Vec::new();
    let sections = html.split("<h2 id=\"LOOKUPS_").skip(1);
    for section in sections {
        let heading = between(section, ">", "</h2>").ok_or_else(|| layout("a heading is not closed"))?.trim();
        let (code, _) = CURRENCIES
            .iter()
            .find(|(_, name)| *name == heading)
            .ok_or_else(|| layout("an unknown currency heading"))?;
        let table = between(section, "<table", "</table>").ok_or_else(|| layout("a currency has no table"))?;
        let head = between(table, "<thead>", "</thead>").ok_or_else(|| layout("the table has no header"))?;
        let columns: Vec<&str> = cells(head, "th");
        if columns != ["Date", "Buy Rate (LKR)", "Sell Rate (LKR)"] {
            return Err(layout("the columns are not Date, Buy Rate (LKR), Sell Rate (LKR)"));
        }
        let body = between(table, "<tbody>", "</tbody>").ok_or_else(|| layout("the table has no body"))?;
        for row in body.split("<tr").skip(1) {
            let values = cells(row, "td");
            let &[date, _buy, sell] = values.as_slice() else {
                return Err(layout("a row does not have three cells"));
            };
            let date = Date::parse(date, format_description!("[year]-[month]-[day]"))
                .map_err(|_| layout("a row has an unreadable date"))?;
            let sell = Decimal::from_str(sell).map_err(|_| layout("a row has an unreadable sell rate"))?;
            if sell <= Decimal::ZERO {
                return Err(layout("a row has a sell rate of zero or less"));
            }
            rows.push(((*code).to_owned(), date, sell));
        }
    }
    Ok(rows)
}

/// The text between the first `open` and the next `close` after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    Some(&text[start..end])
}

/// The trimmed text of each `<tag ...>...</tag>` cell in `text`.
fn cells<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    text.split(open.as_str()).skip(1).filter_map(|cell| between(cell, ">", &close)).map(str::trim).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn newest_row_on_or_before_the_date_wins() {
        let rows = vec![
            ("USD".to_owned(), date!(2026 - 10 - 09), Decimal::from(1)),
            ("USD".to_owned(), date!(2026 - 10 - 16), Decimal::from(3)),
            ("USD".to_owned(), date!(2026 - 10 - 12), Decimal::from(2)),
        ];
        let picked = newest_on_or_before(rows, &["USD".to_owned(), "EUR".to_owned()], date!(2026 - 10 - 15));
        assert_eq!(picked, vec![("USD".to_owned(), date!(2026 - 10 - 12), Decimal::from(2))]);
    }
}
