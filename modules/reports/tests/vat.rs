//! The VAT report and IRD's Schedule 01 and 04 files, over invoices and credit notes issued on closed business dates,
//! and the service charge report.

mod common;

use common::{Hotel, date};
use folio::ReportGroup;
use reports::{OutletServiceCharge, ServiceCharge, VatPeriod, VatReport, VatTimingDifference};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Month;
use uuid::Uuid;

/// July to September 2026.
const Q3: VatPeriod = VatPeriod::Quarter(2026, 3);

const SCHEDULE01_HEADER: &str = "Serial No,Invoice Date,Tax Invoice No,Purchaser's TIN,Name of the Purchaser,\
                                 Description,Value of supply,VAT Amount\r\n";

const SCHEDULE04_HEADER: &str = "Serial No,TIN No,Invoice Date,Invoice No,Tax Credit / Tax Debit Note,Date of Tax \
                                 Credit / Tax Debit Note,Tax Credit No. / Tax Debit Note No.,Value of Tax Credit Note \
                                 / Tax Debit Note,VAT Amount,Issued by Me\r\n";

/// A VAT-registered hotel with its TIN set.
async fn registered(opts: PgConnectOptions) -> Hotel {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update property set vat_registered = true, tin = '300767028' where id = $1")
        .bind(hotel.property)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel
}

/// A Rs 10,000 line on `folio` dated `on`, with Rs 1,000 service charge on the bill and Rs 1,980 VAT at 18% on both,
/// on the bill or `absorbed`. Its VAT is named unlike the property's VAT rule, so only its code makes it VAT.
async fn vat_line(hotel: &Hotel, folio: Uuid, on: time::Date, vat_presentation: &str) {
    let total: i64 = if vat_presentation == "on_bill" { 1_298_000 } else { 1_100_000 };
    let line = Uuid::now_v7();
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency,
                                 base_total, rate, rate_date, business_date)
         values ($1, $2, $3, 'manual', 'misc', 'Line', 1000000, $4, 'LKR', $4, 1, $5, $5)",
    )
    .bind(line)
    .bind(hotel.tenant.0)
    .bind(folio)
    .bind(total)
    .bind(on)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (code, name, kind, rate_bp, base, amount, presentation) in [
        ("SC", "Service charge", "service_charge", 1000, 1_000_000, 100_000, "on_bill"),
        ("VAT", "VAT", "tax", 1800, 1_100_000, 198_000, vat_presentation),
    ] {
        sqlx::query(
            "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(hotel.tenant.0)
        .bind(line)
        .bind(code)
        .bind(name)
        .bind(kind)
        .bind(rate_bp)
        .bind(base)
        .bind(amount)
        .bind(presentation)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
}

/// A tax invoice issued on `issued_on` to a guest named `first last`, with `tin` when given, for one [`vat_line`]
/// supplied the same day, paid in cash. Its id.
async fn invoice(hotel: &Hotel, issued_on: time::Date, first: &str, last: &str, tin: Option<&str>) -> Uuid {
    invoice_for(hotel, issued_on, issued_on, first, last, tin).await
}

/// [`invoice`], for a supply of `supplied_on`.
async fn invoice_for(
    hotel: &Hotel,
    supplied_on: time::Date,
    issued_on: time::Date,
    first: &str,
    last: &str,
    tin: Option<&str>,
) -> Uuid {
    hotel.set_business_date(issued_on).await;
    let folio = hotel.folio().await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "update guest set first_name = $2, last_name = $3, tin = $4
         where id = (select rr.primary_guest_id from folio f join reservation_room rr on rr.id = f.reservation_room_id
                     where f.id = $1)",
    )
    .bind(folio)
    .bind(first)
    .bind(last)
    .bind(tin)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    vat_line(hotel, folio, supplied_on, "on_bill").await;
    hotel.pay(folio, folio::Method::Cash, 1_298_000, "LKR").await;
    let mut tx = hotel.tx().await;
    let issued = folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, folio).await.unwrap();
    tx.commit().await.unwrap();
    issued.id
}

/// Credits `invoice` on `on`.
async fn credit(hotel: &Hotel, invoice: Uuid, on: time::Date) {
    hotel.set_business_date(on).await;
    let mut tx = hotel.tx().await;
    folio::credit_note(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, invoice, "Billed twice").await.unwrap();
    tx.commit().await.unwrap();
}

async fn vat(hotel: &Hotel, period: VatPeriod) -> Result<VatReport, reports::ReportsError> {
    reports::vat(&mut hotel.tx().await, hotel.property, period).await
}

/// Schedule 01 less Schedule 04 is the return's net.
fn assert_reconciles(report: &VatReport) {
    let issued = report.schedule01.iter().fold((0, 0), |(v, t), row| (v + row.value_of_supply, t + row.vat_amount));
    let credited = report.schedule04.iter().fold((0, 0), |(v, t), row| (v + row.value, t + row.vat_amount));
    assert_eq!((issued.0 - credited.0, issued.1 - credited.1), (report.net_value, report.net_vat), "{report:?}");
}

fn refusal(result: Result<VatReport, reports::ReportsError>) -> String {
    match result {
        Err(reports::ReportsError::Invalid(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn period_codes() {
    assert_eq!(Q3.code(), "2630");
    assert_eq!(VatPeriod::Month(2026, Month::August).code(), "2632");
    assert_eq!(VatPeriod::Month(2027, Month::January).code(), "2711");
    assert_eq!(VatPeriod::from_code("2630").unwrap(), Q3);
    assert_eq!(VatPeriod::from_code("2632").unwrap(), VatPeriod::Month(2026, Month::August));
    for invalid in ["2650", "2634", "263", "26300", "ab30", "+630"] {
        assert!(VatPeriod::from_code(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn file_name() {
    assert_eq!(
        reports::schedule_file_name("300767028", 1, "2630", date(2026, 10, 20)),
        "300767028_VAT_SCHEDULE01_2630_20261020_ORIGINAL_V1.csv"
    );
    assert_eq!(
        reports::schedule_file_name("300767028", 4, "2632", date(2026, 9, 2)),
        "300767028_VAT_SCHEDULE04_2632_20260902_ORIGINAL_V1.csv"
    );
}

/// Two invoices to purchasers with a TIN are a row each; three walk-ins are one row under the dummy TIN, dated the
/// quarter's last day. An invoice of another quarter, or of a day not closed yet, is left out.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn schedule01_exact_columns_and_dates(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    invoice(&hotel, date(2026, 6, 30), "Early", "Guest", Some("999888777")).await;
    invoice(&hotel, date(2026, 7, 15), "Ceylon", "Traders", Some("100200300")).await;
    invoice(&hotel, date(2026, 8, 20), "Lanka Tours,", "Ltd", Some("400500600")).await;
    for day in [1, 2, 3] {
        invoice(&hotel, date(2026, 9, day), "Walk", "In", None).await;
    }
    invoice(&hotel, date(2026, 9, 30), "Not", "Closed", None).await;

    let open = vat(&hotel, Q3).await.unwrap();
    assert!(open.not_closed);
    assert_eq!(open.value_of_supply, 55_000);

    hotel.set_business_date(date(2026, 10, 1)).await;
    let report = vat(&hotel, Q3).await.unwrap();
    assert_eq!(report.period_code, "2630");
    assert!(!report.not_closed);
    // Value of supply: the invoice's total less its VAT, the service charge inside.
    assert_eq!((report.value_of_supply, report.output_vat), (66_000, 11_880));
    assert_eq!((report.credit_notes_value, report.credit_notes_vat), (0, 0));
    assert_eq!((report.net_value, report.net_vat), (66_000, 11_880));
    // With no timing difference, the totals are the rows'.
    assert!(report.timing_difference.is_empty());
    assert_reconciles(&report);
    assert_eq!(
        reports::schedule01_csv(&report),
        format!(
            "{SCHEDULE01_HEADER}\
             1,07/15/2026,GAL-2026-000002,100200300,Ceylon Traders,Tax invoice at 18%,11000,1980\r\n\
             2,08/20/2026,GAL-2026-000003,400500600,\"Lanka Tours, Ltd\",Tax invoice at 18%,11000,1980\r\n\
             3,09/30/2026,INCL01_2630,100902885,Purchasers without a TIN,Inclusive Invoices at 18%,44000,7920\r\n"
        )
    );
    assert_eq!(reports::schedule04_csv(&report), SCHEDULE04_HEADER);
}

/// A credit note issued in October goes to Schedule 04 of Q4 with the original's number and date and its purchaser's
/// TIN, or the dummy TIN; Q3 never changes. The totals are the period's supplies, which a credit note alone does not
/// reverse, so Q4's combined row carries the notes and Schedule 01 less Schedule 04 is the net.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn credit_note_reduces_its_own_period_and_is_schedule04(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    let traders = invoice(&hotel, date(2026, 8, 20), "Ceylon", "Traders", Some("100200300")).await;
    let walk_in = invoice(&hotel, date(2026, 9, 1), "Walk", "In", None).await;
    hotel.set_business_date(date(2026, 10, 1)).await;
    let before = vat(&hotel, Q3).await.unwrap();

    credit(&hotel, traders, date(2026, 10, 5)).await;
    credit(&hotel, walk_in, date(2026, 10, 6)).await;
    invoice(&hotel, date(2026, 10, 7), "Walk", "In", None).await;
    hotel.set_business_date(date(2027, 1, 1)).await;

    let q3 = vat(&hotel, Q3).await.unwrap();
    assert_eq!(q3, before);
    assert!(q3.schedule04.is_empty());
    let q4 = vat(&hotel, VatPeriod::Quarter(2026, 4)).await.unwrap();
    assert_eq!((q4.value_of_supply, q4.output_vat), (11_000, 1_980));
    assert_eq!((q4.credit_notes_value, q4.credit_notes_vat), (22_000, 3_960));
    assert_eq!((q4.net_value, q4.net_vat), (11_000, 1_980));
    let rows: Vec<(&str, i64, i64)> =
        q4.schedule01.iter().map(|row| (row.invoice_no.as_str(), row.value_of_supply, row.vat_amount)).collect();
    assert_eq!(rows, [("INCL01_2640", 33_000, 5_940)]);
    assert!(q4.timing_difference.is_empty());
    assert_reconciles(&q4);
    assert_eq!(
        reports::schedule04_csv(&q4),
        format!(
            "{SCHEDULE04_HEADER}\
             1,100200300,08/20/2026,GAL-2026-000001,Credit,10/05/2026,CN-GAL-2026-000001,11000,1980,Y\r\n\
             2,100902885,09/01/2026,GAL-2026-000002,Credit,10/06/2026,CN-GAL-2026-000002,11000,1980,Y\r\n"
        )
    );
}

/// An invoice credited and issued again in the same period: both invoices are TIN rows and the note a Schedule 04
/// row, which together are the one supply, so there is no combined row and no false timing difference.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_credit_and_reissue_in_one_period_reconciles(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    let first = invoice(&hotel, date(2026, 8, 1), "Ceylon", "Traders", Some("100200300")).await;
    credit(&hotel, first, date(2026, 8, 5)).await;
    let folio: Uuid = sqlx::query_scalar("select folio_id from invoice where id = $1")
        .bind(first)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    let mut tx = hotel.tx().await;
    folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, folio).await.unwrap();
    tx.commit().await.unwrap();
    hotel.set_business_date(date(2026, 10, 1)).await;

    let report = vat(&hotel, Q3).await.unwrap();
    assert_eq!((report.net_value, report.net_vat), (11_000, 1_980));
    let rows: Vec<&str> = report.schedule01.iter().map(|row| row.invoice_no.as_str()).collect();
    assert_eq!(rows, ["GAL-2026-000001", "GAL-2026-000002"]);
    assert_eq!((report.credit_notes_value, report.credit_notes_vat), (11_000, 1_980));
    assert!(report.timing_difference.is_empty());
    assert_reconciles(&report);
}

/// A period's invoices are found through `invoice_issued_on_idx`, as the application role under row-level security,
/// among two years of the property's invoices (analyzed).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_periods_invoices_are_read_through_the_issued_on_index(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let hotel = registered(opts).await;
    let folio = hotel.folio().await;
    sqlx::query(
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, data, total_lkr)
         select gen_random_uuid(), $1, $2, $3, 'invoice', 'GAL-2026-' || lpad(g::text, 6, '0'),
                jsonb_build_object('issued_on', to_char(date '2025-01-01' + g % 730, 'YYYY-MM-DD'),
                                   'purchaser', jsonb_build_object('name', 'Walk In', 'tin', null), 'taxes', '[]'),
                0
         from generate_series(1, 5000) as g",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(folio)
    .execute(&owner)
    .await
    .unwrap();
    sqlx::query("analyze invoice").execute(&owner).await.unwrap();

    let mut tx = hotel.tx().await;
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("explain {}", reports::VAT_DOCUMENTS_SQL)))
        .bind(hotel.property)
        .bind(date(2026, 7, 1))
        .bind(date(2026, 9, 30))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("invoice_issued_on_idx"), "{plan}");
}

/// An invoice issued before taxes carried their code is read by the property's VAT rule names: its VAT is named as the
/// rule is, and its service charge is not VAT.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_older_snapshot_is_read_by_tax_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    let issued = invoice(&hotel, date(2026, 8, 20), "Ceylon", "Traders", Some("100200300")).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, data, total_lkr)
         select $2, tenant_id, property_id, folio_id, kind, 'GAL-2026-000099',
                jsonb_set(data, '{taxes}', (
                  select jsonb_agg(case when t->>'code' = 'VAT'
                                        then t - 'code' || jsonb_build_object('name', 'Value added tax')
                                        else t - 'code' end)
                  from jsonb_array_elements(data->'taxes') t)),
                total_lkr
         from invoice where id = $1",
    )
    .bind(issued)
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    hotel.set_business_date(date(2026, 10, 1)).await;

    let report = vat(&hotel, Q3).await.unwrap();
    let rows: Vec<(&str, i64, i64)> =
        report.schedule01.iter().map(|row| (row.invoice_no.as_str(), row.value_of_supply, row.vat_amount)).collect();
    assert_eq!(rows, [("GAL-2026-000001", 11_000, 1_980), ("GAL-2026-000099", 11_000, 1_980)]);
}

/// A folio never invoiced is a supply all the same, in the combined row; absorbed VAT is output VAT too.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn uninvoiced_and_absorbed_vat_are_in_the_combined_row(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    let folio = hotel.folio().await;
    vat_line(&hotel, folio, date(2026, 7, 10), "on_bill").await;
    vat_line(&hotel, folio, date(2026, 8, 10), "absorbed").await;
    invoice(&hotel, date(2026, 9, 1), "Ceylon", "Traders", Some("100200300")).await;
    hotel.set_business_date(date(2026, 10, 1)).await;

    let report = vat(&hotel, Q3).await.unwrap();
    assert_eq!((report.value_of_supply, report.output_vat), (33_000, 5_940));
    let rows: Vec<(&str, i64, i64)> =
        report.schedule01.iter().map(|row| (row.invoice_no.as_str(), row.value_of_supply, row.vat_amount)).collect();
    assert_eq!(rows, [("GAL-2026-000001", 11_000, 1_980), ("INCL01_2630", 22_000, 3_960)]);
    assert!(report.timing_difference.is_empty());
    assert_reconciles(&report);
}

/// A TIN invoice issued in October for a September supply: Q3 has the supply in its combined row, and Q4's TIN row
/// exceeds Q4's own supplies, so its combined row stops at 0 and the excess is a timing difference.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invoice_for_an_earlier_supply_is_a_timing_difference(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    invoice_for(&hotel, date(2026, 9, 30), date(2026, 10, 2), "Ceylon", "Traders", Some("100200300")).await;
    let walk_in = hotel.folio().await;
    vat_line(&hotel, walk_in, date(2026, 10, 3), "on_bill").await;
    let other = hotel.folio().await;
    vat_line(&hotel, other, date(2026, 10, 4), "on_bill").await;
    hotel.set_business_date(date(2027, 1, 1)).await;
    let rows = |report: &VatReport| -> Vec<(String, i64, i64)> {
        report.schedule01.iter().map(|row| (row.invoice_no.clone(), row.value_of_supply, row.vat_amount)).collect()
    };

    let q3 = vat(&hotel, Q3).await.unwrap();
    assert_eq!(rows(&q3), [("INCL01_2630".to_owned(), 11_000, 1_980)]);
    // Q4: Rs 22,000 of supplies less the Rs 11,000 TIN invoice.
    let q4 = vat(&hotel, VatPeriod::Quarter(2026, 4)).await.unwrap();
    assert_eq!(rows(&q4), [("GAL-2026-000001".to_owned(), 11_000, 1_980), ("INCL01_2640".to_owned(), 11_000, 1_980)]);
    assert!(q4.timing_difference.is_empty());
    assert_reconciles(&q4);

    // Q1 2027 has the TIN invoice of a December supply and no supply of its own.
    invoice_for(&hotel, date(2026, 12, 31), date(2027, 1, 1), "Lanka", "Tours", Some("400500600")).await;
    hotel.set_business_date(date(2027, 4, 1)).await;
    let q1 = vat(&hotel, VatPeriod::Quarter(2027, 1)).await.unwrap();
    assert_eq!((q1.value_of_supply, q1.output_vat), (0, 0));
    assert_eq!(rows(&q1), [("GAL-2027-000002".to_owned(), 11_000, 1_980)]);
    assert_eq!(q1.timing_difference, [VatTimingDifference { rate_bp: 1800, value: 11_000, vat: 1_980 }]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn not_vat_registered_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    assert_eq!(refusal(vat(&hotel, Q3).await), "This property is not VAT registered");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_tin_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update property set vat_registered = true where id = $1")
        .bind(hotel.property)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(refusal(vat(&hotel, Q3).await), "Set the property's TIN on Settings → Taxes and invoices first");
}

/// A quarterly filer is refused a month, and a monthly filer a quarter.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_period_of_the_other_kind_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = registered(opts).await;
    let august = VatPeriod::Month(2026, Month::August);
    assert_eq!(refusal(vat(&hotel, august).await), "This property files its VAT return quarterly");
    let mut tx = hotel.tx().await;
    sqlx::query("update property set vat_return_period = 'monthly' where id = $1")
        .bind(hotel.property)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(refusal(vat(&hotel, Q3).await), "This property files its VAT return monthly");
    assert_eq!(vat(&hotel, august).await.unwrap().period_code, "2632");
}

/// The service charge of a month: on the bill as collected, absorbed apart, by outlet, on rooms, and on everything
/// else; other months and other taxes are left out.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn service_charge_by_outlet_and_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("REST", ReportGroup::Food).await;
    hotel.outlet("BAR", ReportGroup::Bar).await;
    let folio = hotel.folio().await;
    let on_bill = |amount| ("SC", "service_charge", "on_bill", amount);
    let absorbed = |amount| ("SC", "service_charge", "absorbed", amount);
    let vat = ("VAT", "tax", "on_bill", 99_900);
    hotel.line(folio, date(2026, 8, 1), "pos", "pos:REST", 1_000_000, &[on_bill(100_000), vat], None).await;
    hotel.line(folio, date(2026, 8, 2), "pos", "pos:REST", 400_000, &[on_bill(40_050)], None).await;
    hotel.line(folio, date(2026, 8, 3), "pos", "pos:BAR", 500_000, &[absorbed(50_000)], None).await;
    hotel.line(folio, date(2026, 8, 4), "room", "room", 2_000_000, &[on_bill(200_000)], None).await;
    hotel.line(folio, date(2026, 8, 5), "meal_plan", "meal_plan", 300_000, &[on_bill(30_000)], None).await;
    hotel.line(folio, date(2026, 9, 1), "pos", "pos:REST", 1_000_000, &[on_bill(100_000)], None).await;
    hotel.set_business_date(date(2026, 9, 2)).await;

    let report = reports::service_charge(&mut hotel.tx().await, hotel.property, 2026, Month::August).await.unwrap();
    let sc = |collected, absorbed| ServiceCharge { collected, absorbed };
    assert_eq!(
        report.by_outlet,
        [
            OutletServiceCharge { outlet: "BAR".into(), collected: 0, absorbed: 50_000 },
            // Rs 1,400.50 rounds once, to Rs 1,401.
            OutletServiceCharge { outlet: "REST".into(), collected: 140_100, absorbed: 0 },
        ]
    );
    assert_eq!(report.rooms, sc(200_000, 0));
    assert_eq!(report.other, sc(30_000, 0));
    assert_eq!(report.total, sc(370_100, 50_000));
    assert!(!report.not_closed);
}
