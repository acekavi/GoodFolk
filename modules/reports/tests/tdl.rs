//! The TDL in SLTDA's Form A, over folio lines and settled POS bills seeded on closed business dates.

mod common;

use common::{Hotel, date};
use folio::{OutletChanges, ReportGroup};
use reports::{FiscalQuarter, TdlForm};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Q1 of the financial year starting April 2026.
const Q1: FiscalQuarter = FiscalQuarter { fy_start_year: 2026, q: 1 };

async fn tdl(hotel: &Hotel, quarter: FiscalQuarter) -> TdlForm {
    reports::tdl(&mut hotel.tx().await, hotel.property, quarter).await.unwrap()
}

/// SLTDA's own worked example: Room 55,800, Food 237,973 (SC 23,797 on it), Bar 3,183,537 settled at the outlet.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sltda_worked_example(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update property set legal_name = 'Galle Fort Hotels (Pvt) Ltd', tdl_registration_no = 'TDL/G/0042'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.outlet("BAR", ReportGroup::Bar).await;
    hotel.set_business_date(date(2026, 5, 5)).await;
    hotel.settled_bill("BAR", "B1", 318_353_700).await;
    let folio = hotel.folio().await;
    hotel.line(folio, date(2026, 4, 10), "room", "room", 5_580_000, &[], None).await;
    let sc = ("SC", "service_charge", "on_bill", 2_379_700);
    hotel.line(folio, date(2026, 6, 30), "meal_plan", "meal_plan", 21_417_600, &[sc], None).await;
    hotel.set_business_date(date(2026, 7, 1)).await;

    assert_eq!(
        tdl(&hotel, Q1).await,
        TdlForm {
            name: "Galle Fort Hotels (Pvt) Ltd".into(),
            registration_no: Some("TDL/G/0042".into()),
            period_label: "April – June (2026)".into(),
            room: 5_580_000,
            food: 23_797_300,
            bar: 318_353_700,
            other: 0,
            total_turnover: 347_731_000,
            service_charge: 2_379_700,
            vat: 0,
            base: 345_351_300,
            rate_bp: 100,
            // 34,535.13 rounded to whole rupees.
            tdl: 3_453_500,
            not_closed: false,
        }
    );
}

/// SLTDA's 0.5% example: a base not over Rs 3,000,000.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn half_percent_under_three_million(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", ReportGroup::Bar).await;
    hotel.set_business_date(date(2026, 7, 1)).await;
    let folio = hotel.folio().await;
    let sc = ("SC", "service_charge", "on_bill", 229_000);
    hotel.line(folio, date(2026, 4, 1), "room", "room", 290_000, &[], None).await;
    hotel.line(folio, date(2026, 5, 1), "meal_plan", "meal_plan", 2_061_000, &[sc], None).await;
    hotel.line(folio, date(2026, 6, 1), "pos", "pos:BAR", 232_352_000, &[], None).await;

    let form = tdl(&hotel, Q1).await;

    assert_eq!((form.room, form.food, form.bar, form.service_charge), (290_000, 2_290_000, 232_352_000, 229_000));
    assert_eq!((form.base, form.rate_bp), (234_703_000, 50));
    // 11,735.15 rounded to whole rupees.
    assert_eq!(form.tdl, 1_173_500);
}

/// A report group changed in July applies from July: April to June keep the outlet's sales under Food.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn outlet_group_change_does_not_restate_a_closed_quarter(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let cafe = hotel.outlet("CAFE", ReportGroup::Food).await;
    let folio = hotel.folio().await;
    hotel.line(folio, date(2026, 6, 15), "pos", "pos:CAFE", 1_000_000, &[], None).await;
    hotel.set_business_date(date(2026, 7, 1)).await;
    let before = tdl(&hotel, Q1).await;

    let mut tx = hotel.tx().await;
    let changes = OutletChanges { report_group: Some(ReportGroup::Bar), ..OutletChanges::default() };
    folio::update_outlet(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, cafe, 1, changes).await.unwrap();
    tx.commit().await.unwrap();
    hotel.line(folio, date(2026, 7, 1), "pos", "pos:CAFE", 400_000, &[], None).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let after = tdl(&hotel, Q1).await;
    assert_eq!((after.food, after.bar), (1_000_000, 0));
    assert_eq!(after, before);
    let q2 = tdl(&hotel, FiscalQuarter { fy_start_year: 2026, q: 2 }).await;
    assert_eq!((q2.food, q2.bar), (0, 400_000));
    assert_eq!(q2.period_label, "July – September (2026)");
}

/// A reversal posted in July reduces Q2, never the Q1 its original was reported in; an adjustment is not revenue.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn credit_reversal_counts_in_its_own_quarter(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.folio().await;
    let sc = ("SC", "service_charge", "on_bill", 100_000);
    let original = hotel.line(folio, date(2026, 6, 20), "room", "room", 1_000_000, &[sc], None).await;
    let reversal = ("SC", "service_charge", "on_bill", -100_000);
    hotel.line(folio, date(2026, 7, 5), "room", "room", -1_000_000, &[reversal], Some(original)).await;
    hotel.line(folio, date(2026, 7, 6), "adjustment", "misc", 50_000, &[], None).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let q1 = tdl(&hotel, Q1).await;
    assert_eq!((q1.room, q1.service_charge, q1.base), (1_100_000, 100_000, 1_000_000));
    let q2 = tdl(&hotel, FiscalQuarter { fy_start_year: 2026, q: 2 }).await;
    assert_eq!((q2.room, q2.other, q2.service_charge, q2.base), (-1_100_000, 0, -100_000, -1_000_000));
}

/// TDL and SSCL passed on to the guest are levies collected, not sales; SC and VAT count whether passed on or
/// absorbed.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn passed_on_levies_are_not_sales(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.folio().await;
    let taxes = [
        ("SC", "service_charge", "absorbed", 100_000),
        ("TDL", "levy", "on_bill", 11_000),
        ("SSCL", "levy", "on_bill", 27_825),
        ("VAT", "tax", "on_bill", 205_009),
    ];
    hotel.line(folio, date(2026, 5, 1), "room", "room", 1_000_000, &taxes, None).await;
    hotel.line(folio, date(2026, 5, 2), "manual", "laundry", 20_000, &[], None).await;
    hotel.set_business_date(date(2026, 7, 1)).await;

    let form = tdl(&hotel, Q1).await;

    assert_eq!((form.room, form.other, form.total_turnover), (1_305_000, 20_000, 1_325_000));
    assert_eq!((form.service_charge, form.vat, form.base), (100_000, 205_000, 1_020_000));
}

/// Each printed figure is whole rupees, rounded once, and the form adds up as printed: Total Turnover is the sum of
/// the rounded sales lines and the base that total less the rounded deductions.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn printed_figures_add_up(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", ReportGroup::Bar).await;
    let folio = hotel.folio().await;
    let sc = ("SC", "service_charge", "on_bill", 10_040);
    let vat = ("VAT", "tax", "on_bill", 18_070);
    // Room 1,001.40 + 100.40 + 180.70 = 1,282.50; Food 500.60 + 50.06 = 550.66; Bar 300.45 + 30.04 = 330.49.
    hotel.line(folio, date(2026, 4, 2), "room", "room", 100_140, &[sc, vat], None).await;
    hotel
        .line(
            folio,
            date(2026, 4, 3),
            "meal_plan",
            "meal_plan",
            50_060,
            &[("SC", "service_charge", "on_bill", 5_006)],
            None,
        )
        .await;
    hotel
        .line(folio, date(2026, 4, 4), "pos", "pos:BAR", 30_045, &[("SC", "service_charge", "on_bill", 3_004)], None)
        .await;
    hotel.set_business_date(date(2026, 7, 1)).await;

    let form = tdl(&hotel, Q1).await;

    assert_eq!((form.room, form.food, form.bar, form.other), (128_300, 55_100, 33_000, 0));
    // SC 100.40 + 50.06 + 30.04 = 180.50; VAT 180.70.
    assert_eq!((form.service_charge, form.vat), (18_100, 18_100));
    for figure in
        [form.room, form.food, form.bar, form.total_turnover, form.service_charge, form.vat, form.base, form.tdl]
    {
        assert_eq!(figure % 100, 0, "{figure} is not whole rupees");
    }
    assert_eq!(form.total_turnover, form.room + form.food + form.bar + form.other);
    assert_eq!(form.total_turnover, 216_400);
    assert_eq!(form.base, form.total_turnover - form.service_charge - form.vat);
    assert_eq!(form.base, 180_200);
    // 0.5% of Rs 1,802 is Rs 9.01, so Rs 9.
    assert_eq!((form.rate_bp, form.tdl), (50, 900));
}

/// A quarter not yet closed is flagged, its figures covering only the closed days.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn quarter_with_open_days_is_flagged(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.folio().await;
    hotel.line(folio, date(2026, 5, 14), "room", "room", 1_000_000, &[], None).await;
    hotel.line(folio, date(2026, 5, 15), "room", "room", 2_000_000, &[], None).await;
    hotel.set_business_date(date(2026, 5, 15)).await;

    let form = tdl(&hotel, Q1).await;

    assert!(form.not_closed);
    assert_eq!(form.room, 1_000_000);
    assert!(!tdl(&hotel, FiscalQuarter { fy_start_year: 2025, q: 4 }).await.not_closed);
}
