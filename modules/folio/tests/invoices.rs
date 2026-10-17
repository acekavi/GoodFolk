mod common;

use common::Hotel;
use db::{Scope, begin};
use folio::{FolioError, FolioQuery, FolioStatus, InvoiceRow, Method, Source};
use media_types::{InvoiceData, InvoiceKind, InvoiceLine, InvoicePayment, InvoiceTax, Purchaser};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::Arc;
use uuid::Uuid;

fn conflict<T: std::fmt::Debug>(result: Result<T, FolioError>) -> String {
    match result {
        Err(FolioError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

fn invalid<T: std::fmt::Debug>(result: Result<T, FolioError>) -> String {
    match result {
        Err(FolioError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn not_found<T: std::fmt::Debug>(result: Result<T, FolioError>) {
    assert!(matches!(result, Err(FolioError::NotFound(_))), "expected NotFound, got {result:?}");
}

fn data(invoice: &InvoiceRow) -> InvoiceData {
    serde_json::from_value(invoice.data.clone()).unwrap()
}

impl Hotel {
    /// A cash payment of `amount` on `folio`, committed.
    async fn pay(&self, folio: Uuid, amount: i64, currency: &str) -> Uuid {
        let mut tx = self.tx().await;
        let payment = folio::take_payment(
            &mut tx,
            self.tenant,
            Some(self.user),
            self.property,
            folio,
            Method::Cash,
            amount,
            currency,
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        payment.id
    }

    /// A room charge of `unit_net` LKR on `folio`, paid in full: a settled folio.
    async fn settled(&self, folio: Uuid, unit_net: i64) {
        let line = self.post(folio, self.charge(Source::Manual, "room", unit_net, "LKR")).await.unwrap();
        self.pay(folio, line.total, "LKR").await;
    }

    async fn issue(&self, folio: Uuid) -> Result<InvoiceRow, FolioError> {
        let mut tx = self.tx().await;
        let invoice = folio::issue_invoice(&mut tx, self.tenant, Some(self.user), self.property, folio).await?;
        tx.commit().await.unwrap();
        Ok(invoice)
    }

    async fn credit(&self, invoice: Uuid, reason: &str) -> Result<InvoiceRow, FolioError> {
        let mut tx = self.tx().await;
        let note = folio::credit_note(&mut tx, self.tenant, Some(self.user), self.property, invoice, reason).await?;
        tx.commit().await.unwrap();
        Ok(note)
    }

    async fn link(&self, property: Uuid, invoice: Uuid) -> Result<String, FolioError> {
        let mut tx = self.tx().await;
        let key = folio::invoice_link_key(&mut tx, self.tenant, Some(self.user), property, invoice).await?;
        tx.commit().await.unwrap();
        Ok(key)
    }

    async fn set_pdf(&self, invoice: Uuid, key: &str) -> Result<(), FolioError> {
        let mut tx = self.tx().await;
        folio::set_invoice_pdf(&mut tx, self.tenant, invoice, key).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    async fn folio_state(&self, folio: Uuid) -> (FolioStatus, i32) {
        let query = FolioQuery::Folio { property: self.property, folio };
        let view = folio::folios_for(&mut self.tx().await, query).await.unwrap().pop().unwrap();
        (view.folio.status, view.folio.version)
    }

    /// The payloads queued on `invoice-render`, oldest first.
    async fn render_jobs(&self) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select payload from outbox where topic = 'invoice-render' order by id")
            .fetch_all(&self.owner)
            .await
            .unwrap()
    }

    fn serial(&self, number: u32) -> String {
        format!("GAL-{}-{number:06}", self.business_date.year())
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_settled_folio_with_lines_is_invoiced(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];

    assert_eq!(conflict(hotel.issue(folio).await), "Nothing to invoice");
    let line = hotel.post(folio, hotel.charge(Source::Manual, "room", 1_000_000, "LKR")).await.unwrap();
    assert_eq!(conflict(hotel.issue(folio).await), "The folio must be settled before it is invoiced");
    hotel.pay(folio, line.total - 1, "LKR").await;
    assert_eq!(conflict(hotel.issue(folio).await), "The folio must be settled before it is invoiced");
    hotel.pay(folio, 1, "LKR").await;
    hotel.issue(folio).await.unwrap();

    assert_eq!(conflict(hotel.issue(folio).await), "The folio is already invoiced");
    let other = hotel.add_property("COL").await;
    let mut tx = hotel.tx().await;
    not_found(folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), other, folio).await);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invoice_freezes_the_folio_and_a_credit_note_reopens_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    // A room night of 10,000.00 without VAT: TDL 100.00 and SSCL 252.50, so 10,352.50 in all.
    let line = hotel.post(folio, hotel.charge(Source::Manual, "room", 1_000_000, "LKR")).await.unwrap();
    assert_eq!(line.total, 1_035_250);
    let payment = hotel.pay(folio, 1_100_000, "LKR").await;
    let mut tx = hotel.tx().await;
    folio::refund(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, payment, 64_750, "Change", None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let invoice = hotel.issue(folio).await.unwrap();
    assert_eq!(
        (invoice.kind, invoice.serial.clone(), invoice.folio_id),
        (InvoiceKind::Invoice, hotel.serial(1), folio)
    );
    assert_eq!((invoice.total_lkr, invoice.issued_by, invoice.pdf_key.clone()), (10_353, Some(hotel.user.0), None));
    let issued = data(&invoice);
    assert_eq!((issued.title.as_str(), issued.vat_registered), ("INVOICE", false));
    assert_eq!(
        (issued.issued_on, issued.supply_from, issued.supply_to),
        (hotel.business_date, hotel.business_date, hotel.business_date)
    );
    assert_eq!(issued.purchaser, Purchaser { name: "Silva".into(), tin: None });
    assert_eq!(issued.supplier.legal_name, "Hotel GAL");
    assert_eq!(
        issued.lines,
        vec![InvoiceLine {
            description: "Charge".into(),
            quantity: 1,
            currency: "LKR".into(),
            total: 1_035_250,
            total_lkr: 10_353
        }]
    );
    assert_eq!(
        issued.taxes,
        vec![
            InvoiceTax { name: "Tourism development levy".into(), rate_bp: 100, amount_lkr: 100 },
            InvoiceTax { name: "Social security contribution levy".into(), rate_bp: 250, amount_lkr: 253 },
        ]
    );
    // The rounding difference lands on the net, so 10,000 + 100 + 253 is the printed 10,353.
    assert_eq!((issued.net_lkr, issued.total_lkr), (10_000, 10_353));
    assert_eq!(issued.total_words, "Ten Thousand Three Hundred Fifty-Three Rupees Only");
    let amounts: Vec<(i64, &str)> = issued.payments.iter().map(|p| (p.amount, p.currency.as_str())).collect();
    assert_eq!(amounts, [(1_100_000, "LKR"), (-64_750, "LKR")]);
    assert_eq!(issued.payments[0].method, "cash");
    assert_eq!((issued.credits_serial.clone(), issued.reason.clone()), (None, None));

    assert_eq!(hotel.folio_state(folio).await, (FolioStatus::Invoiced, 2));
    let refused = hotel.post(folio, hotel.charge(Source::Manual, "misc", 100, "LKR")).await;
    assert!(conflict(refused).contains("invoiced"));
    let audited = hotel.audited("invoice.issued").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].0, invoice.id);
    assert_eq!(audited[0].1["serial"], hotel.serial(1));
    let jobs = hotel.render_jobs().await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["tenant_id"], hotel.tenant.0.to_string());
    assert_eq!(jobs[0]["invoice_id"], invoice.id.to_string());
    assert_eq!(serde_json::from_value::<InvoiceData>(jobs[0]["data"].clone()).unwrap(), issued);

    invalid(hotel.credit(invoice.id, " ").await);
    let note = hotel.credit(invoice.id, " Billed to the wrong guest ").await.unwrap();
    let cn_serial = format!("CN-GAL-{}-000001", hotel.business_date.year());
    assert_eq!((note.kind, note.serial.clone(), note.credits), (InvoiceKind::CreditNote, cn_serial, Some(invoice.id)));
    assert_eq!((note.reason.as_deref(), note.total_lkr), (Some("Billed to the wrong guest"), -10_353));
    let credited = data(&note);
    assert_eq!((credited.kind, credited.title.as_str()), (InvoiceKind::CreditNote, "CREDIT NOTE"));
    assert_eq!(credited.credits_serial.as_deref(), Some(invoice.serial.as_str()));
    assert_eq!(credited.total_words, "Minus Ten Thousand Three Hundred Fifty-Three Rupees Only");
    // Every figure is the original's negated; the parties and the supply period are the original's.
    assert_eq!((credited.net_lkr, credited.total_lkr), (-issued.net_lkr, -issued.total_lkr));
    let negate_line = |l: &InvoiceLine| (-l.quantity, -l.total, -l.total_lkr);
    assert_eq!(
        credited.lines.iter().map(|l| (l.quantity, l.total, l.total_lkr)).collect::<Vec<_>>(),
        issued.lines.iter().map(negate_line).collect::<Vec<_>>()
    );
    assert_eq!(
        credited.taxes.iter().map(|t| (t.name.clone(), t.rate_bp, t.amount_lkr)).collect::<Vec<_>>(),
        issued.taxes.iter().map(|t| (t.name.clone(), t.rate_bp, -t.amount_lkr)).collect::<Vec<_>>()
    );
    assert_eq!(credited.payments, [], "the money stays on the folio");
    assert_eq!((&credited.supplier, &credited.purchaser), (&issued.supplier, &issued.purchaser));
    assert_eq!((credited.supply_from, credited.supply_to), (issued.supply_from, issued.supply_to));
    assert_eq!(hotel.folio_state(folio).await, (FolioStatus::Open, 3));
    let audited = hotel.audited("invoice.credited").await;
    assert_eq!((audited.len(), audited[0].0), (1, invoice.id));
    assert_eq!(audited[0].1["credit_note"], note.id.to_string());
    assert_eq!(hotel.render_jobs().await.len(), 2);

    assert_eq!(conflict(hotel.credit(invoice.id, "Again").await), "The invoice has already been credited");
    invalid(hotel.credit(note.id, "A credit note").await);
    let reissued = hotel.issue(folio).await.unwrap();
    assert_eq!(reissued.serial, hotel.serial(2));
    assert_eq!(hotel.folio_state(folio).await, (FolioStatus::Invoiced, 4));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_vat_registered_property_issues_a_tax_invoice(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    let stay = hotel.book(None).await;
    hotel.settled(stay.guests[0], 1_000_000).await;

    let refused = hotel.issue(stay.guests[0]).await;
    assert_eq!(conflict(refused), "Set the property's TIN before issuing a tax invoice");
    let mut tx = hotel.tx().await;
    sqlx::query("update property set tin = '134567890-7000' where id = $1")
        .bind(hotel.property)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let invoice = hotel.issue(stay.guests[0]).await.unwrap();
    assert_eq!(invoice.serial, hotel.serial(1), "the refusal took no number");
    let issued = data(&invoice);

    assert_eq!(issued.supplier.tin.as_deref(), Some("134567890-7000"));
    assert_eq!((issued.title.as_str(), issued.vat_registered), ("TAX INVOICE", true));
    // 1,035,250 + VAT 186,345 = 1,221,595 cents. Taxes print in their calculation order.
    let taxes: Vec<(i32, i64)> = issued.taxes.iter().map(|t| (t.rate_bp, t.amount_lkr)).collect();
    assert_eq!(taxes, [(100, 100), (250, 253), (1800, 1_863)]);
    assert_eq!((issued.net_lkr, issued.total_lkr), (10_000, 12_216));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_usd_line_is_invoiced_in_whole_rupees(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(3005, 1)).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    // USD 100.00: TDL 1.00 and SSCL 2.53 (2.525 half up), so USD 103.53, which is LKR 31,110.77 at 300.5.
    let line = hotel.post(folio, hotel.charge(Source::Manual, "room", 10_000, "USD")).await.unwrap();
    assert_eq!((line.total, line.base_total), (10_353, 3_111_077));
    hotel.pay(folio, 10_353, "USD").await;

    let issued = data(&hotel.issue(folio).await.unwrap());

    assert_eq!(
        issued.lines,
        vec![InvoiceLine {
            description: "Charge".into(),
            quantity: 1,
            currency: "USD".into(),
            total: 10_353,
            total_lkr: 31_111
        }]
    );
    // Each tax at the line's rate: USD 1.00 is LKR 300.50, so 301; USD 2.53 is LKR 760.265, so 760.
    let taxes: Vec<(i32, i64)> = issued.taxes.iter().map(|t| (t.rate_bp, t.amount_lkr)).collect();
    assert_eq!(taxes, [(100, 301), (250, 760)]);
    assert_eq!((issued.net_lkr, issued.total_lkr), (30_050, 31_111));
    assert_eq!(issued.payments, vec![InvoicePayment { method: "cash".into(), amount: 10_353, currency: "USD".into() }]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reversal_prints_as_a_negative_line_and_its_taxes_net(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    let wrong = hotel.post(folio, hotel.charge(Source::Manual, "room", 500_000, "LKR")).await.unwrap();
    let mut tx = hotel.tx().await;
    folio::reverse(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, folio, wrong.id, "Wrong rate")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.settled(folio, 1_000_000).await;

    let issued = data(&hotel.issue(folio).await.unwrap());

    let lines: Vec<(i32, i64)> = issued.lines.iter().map(|l| (l.quantity, l.total_lkr)).collect();
    assert_eq!(lines, [(1, 5_176), (-1, -5_176), (1, 10_353)]);
    let taxes: Vec<(i32, i64)> = issued.taxes.iter().map(|t| (t.rate_bp, t.amount_lkr)).collect();
    assert_eq!(taxes, [(100, 100), (250, 253)]);
    assert_eq!((issued.net_lkr, issued.total_lkr), (10_000, 10_353));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_purchaser_is_the_account_the_guest_or_the_booker(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let account = hotel.account().await;
    let stay = hotel.book(Some(account)).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update account set tin = '114567890' where id = $1").bind(account).execute(&mut *tx).await.unwrap();
    let account_folio = folio::ensure_account_folio(&mut tx, hotel.tenant, hotel.property, account).await.unwrap();
    // The stay's primary guest is not the booker.
    let guest = Uuid::now_v7();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, tin, residency)
         values ($1, $2, 'Nimal', 'Perera', '987654321V', 'resident')",
    )
    .bind(guest)
    .bind(hotel.tenant.0)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("update reservation_room set primary_guest_id = $2 where id = $1")
        .bind(stay.rooms[0])
        .bind(guest)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    for folio in [account_folio, stay.guests[0], stay.deposit] {
        hotel.settled(folio, 10_000).await;
    }

    let purchaser = |invoice: InvoiceRow| data(&invoice).purchaser;
    assert_eq!(
        purchaser(hotel.issue(account_folio).await.unwrap()),
        Purchaser { name: "Acme".into(), tin: Some("114567890".into()) }
    );
    assert_eq!(
        purchaser(hotel.issue(stay.guests[0]).await.unwrap()),
        Purchaser { name: "Nimal Perera".into(), tin: Some("987654321V".into()) }
    );
    assert_eq!(purchaser(hotel.issue(stay.deposit).await.unwrap()), Purchaser { name: "Silva".into(), tin: None });
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invoices_are_issued_only_in_lkr(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    let input = property::NewProperty {
        code: "USDH".into(),
        name: "Dollar hotel".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "USD".into(),
    };
    let dollar = property::create_property(&mut tx, hotel.tenant, hotel.user, input).await.unwrap().id;
    let folio = folio::ensure_account_folio(&mut tx, hotel.tenant, dollar, hotel.account().await).await.unwrap();

    let refused = folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), dollar, folio).await;

    assert_eq!(invalid(refused), "Invoices are issued in LKR");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn twenty_concurrent_issues_take_twenty_consecutive_serials(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = db::testing::app_pool(opts.clone(), 21).await;
    let hotel = Hotel::new(opts).await;
    let mut folios = Vec::new();
    for _ in 0..20 {
        let stay = hotel.book(None).await;
        hotel.settled(stay.guests[0], 10_000).await;
        folios.push(stay.guests[0]);
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(folios.len()));
    let tasks: Vec<_> = folios
        .into_iter()
        .map(|folio| {
            let (pool, barrier) = (pool.clone(), barrier.clone());
            let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
            tokio::spawn(async move {
                let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
                barrier.wait().await;
                let invoice = folio::issue_invoice(&mut tx, tenant, Some(user), property, folio).await.unwrap();
                tx.commit().await.unwrap();
                invoice.serial
            })
        })
        .collect();
    let mut serials = Vec::new();
    for task in tasks {
        serials.push(task.await.unwrap());
    }

    serials.sort();
    assert_eq!(serials, (1..=20).map(|n| hotel.serial(n)).collect::<Vec<_>>());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rolled_back_issue_gives_its_number_back(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    hotel.settled(stay.guests[0], 10_000).await;

    let mut tx = hotel.tx().await;
    let rolled_back =
        folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, stay.guests[0]).await.unwrap();
    assert_eq!(rolled_back.serial, hotel.serial(1));
    tx.rollback().await.unwrap();

    assert_eq!(hotel.folio_state(stay.guests[0]).await, (FolioStatus::Open, 1));
    assert_eq!(hotel.issue(stay.guests[0]).await.unwrap().serial, hotel.serial(1));
    assert_eq!(hotel.render_jobs().await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_concurrent_issues_of_one_folio_give_one_invoice(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Arc::new(Hotel::new(opts).await);
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    hotel.settled(folio, 10_000).await;

    let mut first = hotel.tx().await;
    folio::issue_invoice(&mut first, hotel.tenant, Some(hotel.user), hotel.property, folio).await.unwrap();
    let racer = hotel.clone();
    let second = tokio::spawn(async move { racer.issue(folio).await });
    hotel.until_blocked().await;
    first.commit().await.unwrap();

    assert_eq!(conflict(second.await.unwrap()), "The folio is already invoiced");
    let count: i64 =
        sqlx::query_scalar("select count(*) from invoice").fetch_one(&mut *hotel.tx().await).await.unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_link_waits_for_the_pdf_and_is_audited(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    hotel.settled(stay.guests[0], 10_000).await;
    let invoice = hotel.issue(stay.guests[0]).await.unwrap();

    assert_eq!(conflict(hotel.link(hotel.property, invoice.id).await), "The invoice PDF is still being prepared");
    let other = format!("t/{}/invoices/{}.pdf", hotel.tenant.0, Uuid::now_v7());
    invalid(hotel.set_pdf(invoice.id, &other).await);
    let key = format!("t/{}/invoices/{}.pdf", hotel.tenant.0, invoice.id);
    hotel.set_pdf(invoice.id, &key).await.unwrap();
    // A second key (a redelivery of another render) changes nothing.
    hotel.set_pdf(invoice.id, &format!("t/{}/invoices/{}/again.pdf", hotel.tenant.0, invoice.id)).await.unwrap();

    assert_eq!(hotel.link(hotel.property, invoice.id).await.unwrap(), key);
    let viewed = hotel.audited("invoice.viewed").await;
    assert_eq!(viewed, vec![(invoice.id, serde_json::json!({ "serial": invoice.serial }))]);

    let other_property = hotel.add_property("COL").await;
    not_found(hotel.link(other_property, invoice.id).await);
    not_found(hotel.credit(Uuid::now_v7(), "Missing").await);
    let mut tx = hotel.tx().await;
    not_found(folio::credit_note(&mut tx, hotel.tenant, Some(hotel.user), other_property, invoice.id, "Wrong").await);
    assert_eq!(hotel.audited("invoice.viewed").await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_line_totals_sum_to_the_money_settled(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    // 30 nights at 10,050.50 with the taxes included: 301,515.00 settled, where rounding each night would print
    // 301,530.
    for _ in 0..30 {
        let night = folio::NewCharge { inclusive: true, ..hotel.charge(Source::Manual, "room", 1_005_050, "LKR") };
        assert_eq!(hotel.post(folio, night).await.unwrap().total, 1_005_050);
    }
    hotel.pay(folio, 30_151_500, "LKR").await;

    let issued = data(&hotel.issue(folio).await.unwrap());

    assert_eq!(issued.total_lkr, 301_515);
    assert_eq!(issued.lines.iter().map(|l| l.total_lkr).sum::<i64>(), 301_515);
    let taxes: i64 = issued.taxes.iter().map(|t| t.amount_lkr).sum();
    assert_eq!(issued.net_lkr + taxes, issued.total_lkr);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_mixed_currency_folio_prints_lines_that_sum_to_its_total(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(3005, 1)).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    let mut base = 0;
    for (unit_net, currency) in [(10_000, "USD"), (333_333, "LKR"), (1_999, "USD"), (50_050, "LKR")] {
        let line = hotel.post(folio, hotel.charge(Source::Manual, "room", unit_net, currency)).await.unwrap();
        base += line.base_total;
        hotel.pay(folio, line.total, currency).await;
    }

    let issued = data(&hotel.issue(folio).await.unwrap());

    let rounded = (base + 50) / 100;
    assert_eq!(issued.total_lkr, rounded);
    assert_eq!(issued.lines.iter().map(|l| l.total_lkr).sum::<i64>(), rounded);
    let taxes: i64 = issued.taxes.iter().map(|t| t.amount_lkr).sum();
    assert_eq!(issued.net_lkr + taxes, issued.total_lkr);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_series_continues_past_six_digits(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    hotel.settled(stay.guests[0], 10_000).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into property_counter (tenant_id, property_id, name, value) values ($1, $2, 'invoice', 999999)",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let invoice = hotel.issue(stay.guests[0]).await.unwrap();

    assert_eq!(invoice.serial, format!("GAL-{}-1000000", hotel.business_date.year()));
}
