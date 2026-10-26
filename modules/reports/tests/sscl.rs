//! The SSCL by month, with the foreign-currency exemption measured when each folio or outlet bill is settled, over
//! lines and payments seeded on closed business dates.

mod common;

use common::{Hotel, date};
use folio::{Method, ReportGroup};
use reports::liability::settlement::{Segment, segments};
use reports::{CalendarQuarter, SsclReport};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Month;

/// July to September 2026.
const Q3: CalendarQuarter = CalendarQuarter { year: 2026, q: 3 };

async fn sscl(hotel: &Hotel, quarter: CalendarQuarter) -> SsclReport {
    reports::sscl(&mut hotel.tx().await, hotel.property, quarter).await.unwrap()
}

fn card() -> Method {
    Method::CardTerminal { approval: "A1".into(), last4: "4242".into(), brand: "Visa".into() }
}

/// A guest folio with USD 200.00 (LKR 60,000.00 at 300) and LKR 40,000.00 of turnover on 10 July, settled that day
/// by USD card and LKR cash.
async fn mixed_folio(hotel: &Hotel) -> uuid::Uuid {
    hotel.set_business_date(date(2026, 7, 10)).await;
    hotel.rate("USD", 300).await;
    let folio = hotel.folio().await;
    hotel.line_in(folio, date(2026, 7, 10), "room", 20_000, "USD", 6_000_000).await;
    hotel.line(folio, date(2026, 7, 10), "manual", "misc", 4_000_000, &[], None).await;
    hotel.pay(folio, card(), 20_000, "USD").await;
    hotel.pay(folio, Method::Cash, 4_000_000, "LKR").await;
    folio
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn usd_card_share_exempt_lkr_cash_not(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    let folio = mixed_folio(&hotel).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let found = segments(&mut hotel.tx().await, hotel.property, date(2026, 7, 1), date(2026, 9, 30)).await.unwrap();
    assert_eq!(
        found,
        vec![Segment {
            folio_id: folio,
            settled_on: date(2026, 7, 10),
            exempt_share_bp: 6_000,
            covers_from: date(2026, 7, 10)
        }]
    );
    let report = sscl(&hotel, Q3).await;
    let july = &report.months[0];
    assert_eq!((july.turnover, july.exempt_same_month, july.exempt_earlier_months), (10_000_000, 6_000_000, 0));
    assert_eq!((july.bad_debts, july.liable, july.foreign_cash), (0, 4_000_000, 0));
    // 2.5% of Rs 40,000.
    assert_eq!(july.levy, 100_000);
    assert!(report.sltda_registered);
    assert_eq!(report.exemption_note, None);
    assert_eq!(
        report.method_note,
        "Exempt: services paid in foreign currency by card or bank transfer (SSCL Act No. 25 of 2022, First Schedule \
         Part II item 10), measured when each bill is settled."
    );
}

/// Foreign cash is listed, never exempt; a bill settled at its outlet is its own settlement.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn foreign_cash_listed_not_exempt(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    hotel.outlet("BAR", ReportGroup::Bar).await;
    hotel.set_business_date(date(2026, 8, 3)).await;
    hotel.rate("USD", 300).await;
    let folio = hotel.folio().await;
    hotel.line_in(folio, date(2026, 8, 3), "room", 10_000, "USD", 3_000_000).await;
    hotel.pay(folio, Method::Cash, 10_000, "USD").await;
    // USD 50.00 in cash and USD 20.00 by card, at the bar, with the property's TDL (1%) and SSCL (2.5%) passed on:
    // USD 51.76 (LKR 15,528) and USD 20.71 (LKR 6,213).
    hotel.settled_bill_in("BAR", "B1", 5_000, "USD", Method::Cash).await;
    hotel.settled_bill_in("BAR", "B2", 2_000, "USD", card()).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let august = &sscl(&hotel, Q3).await.months[1];

    assert_eq!(august.turnover, 5_174_100);
    assert_eq!(august.foreign_cash, 4_552_800);
    assert_eq!((august.exempt_same_month, august.exempt_earlier_months), (621_300, 0));
    assert_eq!(august.liable, 4_552_800);
}

/// July's turnover settled in August is exempted in August, as turnover of earlier months; July never changes.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unsettled_folio_adjusts_month_it_settles(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    hotel.set_business_date(date(2026, 7, 20)).await;
    hotel.rate("USD", 300).await;
    let folio = hotel.folio().await;
    hotel.line_in(folio, date(2026, 7, 20), "room", 10_000, "USD", 3_000_000).await;
    hotel.set_business_date(date(2026, 8, 1)).await;
    let before = sscl(&hotel, Q3).await;
    let july = &before.months[0];
    assert_eq!((july.turnover, july.exempt_same_month, july.exempt_earlier_months), (3_000_000, 0, 0));
    assert_eq!((july.liable, july.levy), (3_000_000, 75_000));
    assert!(!july.not_closed);
    assert!(before.months[1].not_closed);

    hotel.set_business_date(date(2026, 8, 5)).await;
    // At a new rate: the USD balance clears though the LKR base does not net to zero.
    hotel.rate("USD", 310).await;
    hotel.pay(folio, Method::BankTransfer { reference: "TT-9".into() }, 10_000, "USD").await;
    hotel.set_business_date(date(2026, 10, 1)).await;
    let after = sscl(&hotel, Q3).await;

    assert_eq!(after.months[0], before.months[0]);
    let august = &after.months[1];
    assert_eq!((august.turnover, august.exempt_same_month, august.exempt_earlier_months), (0, 0, 3_000_000));
    // The adjustment takes August below 0; the levy never does.
    assert_eq!((august.liable, august.levy), (-3_000_000, 0));
}

/// A folio settled in July, then charged and settled again in August and September: each settlement exempts only
/// its own lines.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn resettled_folio_not_double_counted(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    hotel.set_business_date(date(2026, 7, 10)).await;
    hotel.rate("USD", 300).await;
    let folio = hotel.folio().await;
    hotel.line_in(folio, date(2026, 7, 10), "room", 10_000, "USD", 3_000_000).await;
    hotel.pay(folio, card(), 10_000, "USD").await;
    hotel.set_business_date(date(2026, 8, 10)).await;
    hotel.line(folio, date(2026, 8, 10), "manual", "misc", 1_000_000, &[], None).await;
    hotel.pay(folio, Method::Cash, 1_000_000, "LKR").await;
    hotel.set_business_date(date(2026, 9, 10)).await;
    hotel.line_in(folio, date(2026, 9, 10), "room", 5_000, "USD", 1_500_000).await;
    hotel.pay(folio, card(), 5_000, "USD").await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let found = segments(&mut hotel.tx().await, hotel.property, date(2026, 7, 1), date(2026, 9, 30)).await.unwrap();
    let shares: Vec<_> = found.iter().map(|segment| (segment.settled_on, segment.exempt_share_bp)).collect();
    assert_eq!(shares, vec![(date(2026, 7, 10), 10_000), (date(2026, 8, 10), 0), (date(2026, 9, 10), 10_000)]);
    let report = sscl(&hotel, Q3).await;
    let exempt: Vec<_> =
        report.months.iter().map(|month| (month.exempt_same_month, month.exempt_earlier_months)).collect();
    assert_eq!(exempt, vec![(3_000_000, 0), (0, 0), (1_500_000, 0)]);
    assert_eq!(report.quarter_totals.exempt_same_month, 4_500_000);
    assert_eq!(report.quarter_totals.liable, 1_000_000);
}

/// A deposit paid by USD card and moved to the stay at check-in carries its share: the stay, settled in USD cash for
/// the rest, is half exempt.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deposit_transfer_carries_its_share(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    hotel.set_business_date(date(2026, 7, 8)).await;
    hotel.rate("USD", 300).await;
    let stay = hotel.stay("checked_in", date(2026, 7, 10), date(2026, 7, 12), None, "Ann", "Perera").await;
    let mut tx = hotel.tx().await;
    let reservation: uuid::Uuid = sqlx::query_scalar("select reservation_id from reservation_room where id = $1")
        .bind(stay)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let deposit = folio::ensure_deposit_folio(&mut tx, hotel.tenant, hotel.property, reservation).await.unwrap();
    let guest = folio::ensure_guest_folio(&mut tx, hotel.tenant, hotel.property, stay).await.unwrap();
    tx.commit().await.unwrap();
    hotel.pay(deposit, card(), 10_000, "USD").await;

    hotel.set_business_date(date(2026, 7, 10)).await;
    let mut tx = hotel.tx().await;
    folio::move_deposit(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, reservation, stay).await.unwrap();
    tx.commit().await.unwrap();
    hotel.line_in(guest, date(2026, 7, 10), "room", 20_000, "USD", 6_000_000).await;
    hotel.set_business_date(date(2026, 7, 11)).await;
    hotel.pay(guest, Method::Cash, 10_000, "USD").await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let found = segments(&mut hotel.tx().await, hotel.property, date(2026, 7, 1), date(2026, 9, 30)).await.unwrap();
    let stay_segment = found.iter().find(|segment| segment.folio_id == guest).unwrap();
    assert_eq!((stay_segment.settled_on, stay_segment.exempt_share_bp), (date(2026, 7, 11), 5_000));
    let july = &sscl(&hotel, Q3).await.months[0];
    assert_eq!((july.turnover, july.exempt_same_month, july.foreign_cash), (6_000_000, 3_000_000, 3_000_000));
}

/// Turnover of June settled in July reads June's lines of that folio only: the folio's exemption is in July, as
/// turnover of earlier months, and another folio's June lines don't enter the quarter.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn earlier_quarter_lines_of_settled_folio(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.register_sltda().await;
    hotel.set_business_date(date(2026, 6, 28)).await;
    hotel.rate("USD", 300).await;
    let folio = hotel.folio().await;
    let other = hotel.folio().await;
    hotel.line_in(folio, date(2026, 6, 28), "room", 10_000, "USD", 3_000_000).await;
    hotel.line(other, date(2026, 6, 28), "room", "room", 500_000, &[], None).await;
    hotel.set_business_date(date(2026, 7, 2)).await;
    hotel.pay(folio, card(), 10_000, "USD").await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let july = &sscl(&hotel, Q3).await.months[0];

    assert_eq!((july.turnover, july.exempt_same_month, july.exempt_earlier_months), (0, 0, 3_000_000));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn not_sltda_registered_exempts_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    mixed_folio(&hotel).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let report = sscl(&hotel, Q3).await;

    assert!(!report.sltda_registered);
    assert_eq!(report.exemption_note, Some("Not SLTDA registered: no exemption"));
    let july = &report.months[0];
    assert_eq!((july.exempt_same_month, july.exempt_earlier_months, july.liable), (0, 0, 10_000_000));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn due_dates(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let report = sscl(&hotel, Q3).await;

    let months: Vec<_> = report.months.iter().map(|month| (month.month, month.due)).collect();
    assert_eq!(
        months,
        vec![
            (Month::July, date(2026, 8, 20)),
            (Month::August, date(2026, 9, 20)),
            (Month::September, date(2026, 10, 20))
        ]
    );
    assert_eq!(report.return_due, date(2026, 10, 20));
    let q4 = sscl(&hotel, CalendarQuarter { year: 2026, q: 4 }).await;
    assert_eq!((q4.months[2].due, q4.return_due), (date(2027, 1, 20), date(2027, 1, 20)));
    let q1 = sscl(&hotel, CalendarQuarter { year: 2027, q: 1 }).await;
    assert_eq!(q1.months[0].month, Month::January);
    assert!(reports::sscl(&mut hotel.tx().await, hotel.property, CalendarQuarter { year: 2026, q: 5 }).await.is_err());
}

/// Turnover is every tax but VAT on the net, passed-on levies included; each printed figure is whole rupees and the
/// levy 2.5% of the printed liable turnover.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn levy_is_two_and_half_percent_of_liable(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.folio().await;
    let taxes = [("TDL", "levy", "on_bill", 10_001), ("VAT", "tax", "on_bill", 180_025)];
    hotel.line(folio, date(2026, 7, 2), "room", "room", 1_000_140, &taxes, None).await;
    hotel.line(folio, date(2026, 8, 2), "room", "room", 1_000_000, &[], None).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let report = sscl(&hotel, Q3).await;

    let july = &report.months[0];
    // Rs 10,001.40 + 100.01 is Rs 10,101.41, so Rs 10,101; 2.5% is Rs 252.525, so Rs 253.
    assert_eq!((july.turnover, july.liable, july.levy), (1_010_100, 1_010_100, 25_300));
    let august = &report.months[1];
    assert_eq!((august.liable, august.levy), (1_000_000, 25_000));
    for month in &report.months {
        assert_eq!(
            month.liable,
            month.turnover - month.exempt_same_month - month.exempt_earlier_months - month.bad_debts
        );
        for figure in [month.turnover, month.liable, month.levy] {
            assert_eq!(figure % 100, 0, "{figure} is not whole rupees");
        }
    }
    let totals = &report.quarter_totals;
    assert_eq!((totals.turnover, totals.liable, totals.levy), (2_010_100, 2_010_100, 50_300));
}
