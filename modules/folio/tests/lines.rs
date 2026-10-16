mod common;

use common::Hotel;
use folio::{Balance, FolioError, FolioKind, FolioQuery, FolioStatus, FolioView, LineRow, NewCharge, Source};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
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

async fn view(hotel: &Hotel, folio: Uuid) -> FolioView {
    let query = FolioQuery::Folio { property: hotel.property, folio };
    folio::folios_for(&mut hotel.tx().await, query).await.unwrap().pop().expect("the folio")
}

async fn reverse(hotel: &Hotel, folio: Uuid, line: Uuid) -> Result<LineRow, FolioError> {
    let mut tx = hotel.tx().await;
    let reversal =
        folio::reverse(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, folio, line, " Posted twice ").await?;
    tx.commit().await.unwrap();
    Ok(reversal)
}

async fn transfer(hotel: &Hotel, line: Uuid, to: Uuid) -> Result<(LineRow, LineRow), FolioError> {
    let mut tx = hotel.tx().await;
    let moved =
        folio::transfer(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, line, to, "Wrong room").await?;
    tx.commit().await.unwrap();
    Ok(moved)
}

/// `(code, kind, rate_bp, base, amount, presentation)` of a line's taxes, by code.
fn taxes(view: &FolioView, line: Uuid) -> Vec<(String, String, i32, i64, i64, String)> {
    let line = view.lines.iter().find(|entry| entry.line.id == line).expect("the line");
    line.taxes
        .iter()
        .map(|t| (t.code.clone(), t.kind.clone(), t.rate_bp, t.base, t.amount, t.presentation.clone()))
        .collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_restaurant_charge_stores_the_golden_components(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    let stay = hotel.book(None).await;

    let line = hotel.post(stay.guests[0], hotel.charge(Source::Manual, "pos:REST", 1_000_000, "LKR")).await.unwrap();

    assert_eq!((line.net, line.total, line.base_total), (1_000_000, 1_342_545, 1_342_545));
    assert_eq!((line.rate, line.rate_date, line.currency.as_str()), (Decimal::ONE, hotel.business_date, "LKR"));
    assert_eq!((line.source, line.category.as_str(), line.posted_by), (Source::Manual, "pos:REST", Some(hotel.user.0)));
    let view = view(&hotel, stay.guests[0]).await;
    let on_bill = |code: &str, kind: &str, rate: i32, base: i64, amount: i64| {
        (code.to_owned(), kind.to_owned(), rate, base, amount, "on_bill".to_owned())
    };
    assert_eq!(
        taxes(&view, line.id),
        vec![
            on_bill("SC", "service_charge", 1000, 1_000_000, 100_000),
            on_bill("SSCL", "levy", 250, 1_110_000, 27_750),
            on_bill("TDL", "levy", 100, 1_000_000, 10_000),
            on_bill("VAT", "tax", 1800, 1_137_750, 204_795),
        ]
    );
    assert_eq!(view.balances, vec![Balance { currency: "LKR".into(), amount: 1_342_545, base_amount: 1_342_545 }]);
    let audited = hotel.audited("folio.charge_posted").await;
    assert_eq!(audited.len(), 1, "a manual charge is audited");
    assert_eq!((audited[0].0, &audited[0].1["line"]), (stay.guests[0], &serde_json::json!(line.id)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_system_charge_is_not_audited_and_an_inclusive_price_is_the_total(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    let stay = hotel.book(None).await;

    let charge = NewCharge { inclusive: true, quantity: 2, ..hotel.charge(Source::Room, "room", 500_001, "LKR") };
    let line = hotel.post(stay.guests[0], charge).await.unwrap();

    assert_eq!(line.total, 1_000_002, "net plus taxes equal the price exactly");
    let view = view(&hotel, stay.guests[0]).await;
    let taxed: i64 = view.lines[0].taxes.iter().map(|tax| tax.amount).sum();
    assert_eq!(line.net + taxed, line.total);
    assert!(view.lines[0].taxes.iter().all(|tax| tax.code != "SC"), "no service charge on rooms");
    assert!(hotel.audited("folio.charge_posted").await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_foreign_charge_carries_its_base_amount_and_the_rate_s_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(3005, 1)).await;
    let stay = hotel.book(None).await;

    let line = hotel.post(stay.guests[0], hotel.charge(Source::Manual, "room", 10_000, "USD")).await.unwrap();

    // Not VAT-registered: TDL 100 and SSCL round_half_up(10_100 × 2.5%) = 253, so 10_353 cents.
    assert_eq!((line.net, line.total), (10_000, 10_353));
    // 103.53 USD × 300.5 = 31,110.765 LKR, half-up in cents.
    assert_eq!((line.base_total, line.rate, line.rate_date), (3_111_077, Decimal::new(3005, 1), hotel.business_date));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_currency_outside_the_accepted_list_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;

    let message = invalid(hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "JPY")).await);

    assert_eq!(message, "JPY is not a currency this property accepts");
    assert!(hotel.lines_on(&[stay.guests[0]]).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_missing_rate_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;

    let message = conflict(hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "EUR")).await);

    assert_eq!(message, format!("No exchange rate for EUR on {}", hotel.business_date));
    assert_eq!(message.len(), "No exchange rate for EUR on YYYY-MM-DD".len());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_charges_are_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let charge = hotel.charge(Source::Manual, "misc", 100, "LKR");

    for bad in [
        NewCharge { category: "spa".into(), ..charge.clone() },
        NewCharge { category: "pos:bar".into(), ..charge.clone() },
        NewCharge { description: "  ".into(), ..charge.clone() },
        NewCharge { quantity: 0, ..charge.clone() },
        NewCharge { quantity: 10_000, ..charge.clone() },
        NewCharge { quantity: -10_000, ..charge.clone() },
        NewCharge { unit_net: 10_000_000_000_001, ..charge.clone() },
        NewCharge { unit_net: -10_000_000_000_001, ..charge.clone() },
    ] {
        invalid(hotel.post(stay.guests[0], bad).await);
    }
    let largest = NewCharge { quantity: -9_999, unit_net: 10_000_000_000_000, ..charge.clone() };
    hotel.post(stay.guests[0], largest).await.expect("the bounds themselves are allowed");
    assert!(matches!(hotel.post(Uuid::now_v7(), charge).await, Err(FolioError::NotFound("folio"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reversal_nets_to_zero_including_taxes(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    hotel.rate("USD", Decimal::new(3005, 1)).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    let line = hotel.post(folio, hotel.charge(Source::Manual, "pos:BAR", 12_345, "USD")).await.unwrap();

    let reversal = reverse(&hotel, folio, line.id).await.unwrap();

    assert_eq!(reversal.reverses, Some(line.id));
    assert_eq!(reversal.reason.as_deref(), Some("Posted twice"));
    assert_eq!(
        (reversal.quantity, reversal.net, reversal.total, reversal.base_total),
        (-line.quantity, -line.net, -line.total, -line.base_total)
    );
    assert_eq!((reversal.rate, reversal.rate_date), (line.rate, line.rate_date), "at the original's rate");
    let view = view(&hotel, folio).await;
    let negated: Vec<_> = taxes(&view, line.id).into_iter().map(|(c, k, r, b, a, p)| (c, k, r, -b, -a, p)).collect();
    assert_eq!(taxes(&view, reversal.id), negated);
    assert_eq!(view.balances, vec![Balance { currency: "USD".into(), amount: 0, base_amount: 0 }]);
    let audited = hotel.audited("folio.line_reversed").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].1["reversal"], serde_json::json!(reversal.id));
    assert_eq!(
        (&audited[0].1["total"], &audited[0].1["base_total"], &audited[0].1["currency"]),
        (&serde_json::json!(line.total), &serde_json::json!(line.base_total), &serde_json::json!("USD"))
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reversing_twice_is_a_conflict_and_reversing_a_reversal_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    let line = hotel.post(folio, hotel.charge(Source::Manual, "misc", 1_000, "LKR")).await.unwrap();
    let reversal = reverse(&hotel, folio, line.id).await.unwrap();

    conflict(reverse(&hotel, folio, line.id).await);
    invalid(reverse(&hotel, folio, reversal.id).await);

    let other = hotel.book(None).await;
    assert!(matches!(reverse(&hotel, other.guests[0], line.id).await, Err(FolioError::NotFound("line"))));
    let mut tx = hotel.tx().await;
    invalid(folio::reverse(&mut tx, hotel.tenant, None, hotel.property, folio, line.id, " ").await);
    assert_eq!(hotel.lines_on(&[folio]).await.len(), 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_transfer_moves_the_exact_amount_and_links_both_lines(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    let (from, to) = (hotel.book(None).await, hotel.book(None).await);
    let line = hotel.post(from.guests[0], hotel.charge(Source::Manual, "pos:BAR", 4_321, "LKR")).await.unwrap();

    let (reversal, moved) = transfer(&hotel, line.id, to.guests[0]).await.unwrap();

    assert_eq!((reversal.folio_id, reversal.reverses, reversal.total), (from.guests[0], Some(line.id), -line.total));
    assert_eq!((moved.folio_id, moved.reverses, moved.total), (to.guests[0], None, line.total));
    assert_eq!(
        (moved.net, moved.base_total, moved.category.as_str(), moved.source),
        (line.net, line.base_total, "pos:BAR", Source::Manual)
    );
    assert!(reversal.transfer_id.is_some());
    assert_eq!(reversal.transfer_id, moved.transfer_id, "both lines share the transfer");
    let (source, target) = (view(&hotel, from.guests[0]).await, view(&hotel, to.guests[0]).await);
    assert_eq!(source.balances, vec![Balance { currency: "LKR".into(), amount: 0, base_amount: 0 }]);
    assert_eq!(
        target.balances,
        vec![Balance { currency: "LKR".into(), amount: line.total, base_amount: line.base_total }]
    );
    assert_eq!(taxes(&target, moved.id), taxes(&source, line.id), "the taxes move with the line");
    let audited = hotel.audited("folio.line_transferred").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(
        (&audited[0].1["total"], &audited[0].1["base_total"], &audited[0].1["currency"]),
        (&serde_json::json!(line.total), &serde_json::json!(line.base_total), &serde_json::json!("LKR"))
    );

    conflict(transfer(&hotel, line.id, to.guests[0]).await);
    invalid(transfer(&hotel, reversal.id, to.guests[0]).await);
    invalid(transfer(&hotel, moved.id, to.guests[0]).await);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_s_line_moves_only_to_its_own_reservation_s_folios_or_another_stay(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let (acme, other_account) = (hotel.account().await, hotel.account().await);
    let stay = hotel.book(Some(acme)).await;
    let other = hotel.book(Some(other_account)).await;
    let mut tx = hotel.tx().await;
    let acme_folio = folio::ensure_account_folio(&mut tx, hotel.tenant, hotel.property, acme).await.unwrap();
    let other_folio = folio::ensure_account_folio(&mut tx, hotel.tenant, hotel.property, other_account).await.unwrap();
    tx.commit().await.unwrap();
    let post = || hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR"));

    invalid(transfer(&hotel, post().await.unwrap().id, other.deposit).await);
    invalid(transfer(&hotel, post().await.unwrap().id, other_folio).await);
    let to_deposit = transfer(&hotel, post().await.unwrap().id, stay.deposit).await.unwrap().1;
    transfer(&hotel, post().await.unwrap().id, acme_folio).await.unwrap();
    let back = transfer(&hotel, to_deposit.id, stay.guests[0]).await.unwrap().1;
    assert_eq!(back.folio_id, stay.guests[0], "and back again");
    invalid(transfer(&hotel, post().await.unwrap().id, stay.guests[0]).await);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_transfer_to_another_property_s_folio_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let here = hotel.book(None).await;
    let there = hotel.book_in(kandy, 1, None, None).await;
    let line = hotel.post(here.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await.unwrap();

    assert!(matches!(transfer(&hotel, line.id, there.guests[0]).await, Err(FolioError::NotFound("folio"))));
    assert!(matches!(transfer(&hotel, line.id, Uuid::now_v7()).await, Err(FolioError::NotFound("folio"))));
    assert_eq!(hotel.lines_on(&[here.guests[0], there.guests[0]]).await.len(), 1, "nothing moved");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_posting_key_replay_is_a_no_op(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let key = format!("room:{}:{}", stay.rooms[0], hotel.business_date);
    let charge = NewCharge { posting_key: Some(key.clone()), ..hotel.charge(Source::Room, "room", 2_000, "LKR") };

    let first = hotel.post(stay.guests[0], charge.clone()).await.unwrap();
    let again = hotel.post(stay.guests[0], NewCharge { unit_net: 9_999, ..charge }).await.unwrap();

    assert_eq!(again, first, "the replay returns the original line");
    assert_eq!(hotel.lines_on(&[stay.guests[0]]).await, vec![(stay.guests[0], first.total, Some(key))]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invoiced_folio_refuses_charges_reversals_and_transfers(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (stay, other) = (hotel.book(None).await, hotel.book(None).await);
    let line = hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query("update folio set status = 'invoiced', version = version + 1 where id = $1")
        .bind(stay.guests[0])
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let message = conflict(hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await);
    assert_eq!(message, "the folio is invoiced; a credit note reopens it for new lines");
    conflict(reverse(&hotel, stay.guests[0], line.id).await);
    conflict(transfer(&hotel, line.id, other.guests[0]).await);
    assert_eq!(hotel.lines_on(&[stay.guests[0], other.guests[0]]).await.len(), 1);
}

/// Counts the statements sqlx runs on this thread (its `sqlx::query` events) while installed.
struct Statements(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn folios_for_reads_three_folios_in_one_statement_per_kind(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(300, 0)).await;
    let booking = hotel.book_in(hotel.property, 2, None, None).await;
    let unrelated = hotel.book(None).await;
    let first = hotel.post(booking.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await.unwrap();
    let second = hotel.post(booking.guests[0], hotel.charge(Source::Manual, "pos:BAR", 200, "USD")).await.unwrap();
    let third = hotel.post(booking.guests[1], hotel.charge(Source::Manual, "laundry", 300, "LKR")).await.unwrap();
    hotel.post(unrelated.guests[0], hotel.charge(Source::Manual, "misc", 400, "LKR")).await.unwrap();
    let payment = Uuid::now_v7();
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into payment (id, tenant_id, folio_id, method, amount, currency, base_amount, rate, rate_date,
                              business_date)
         values ($1, $2, $3, 'cash', 5000, 'LKR', 5000, 1, $4, $4)",
    )
    .bind(payment)
    .bind(hotel.tenant.0)
    .bind(booking.deposit)
    .bind(hotel.business_date)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let statements = Arc::new(AtomicUsize::new(0));
    let views = {
        let _counting = tracing_subscriber::registry().with(Statements(statements.clone())).set_default();
        folio::folios_for(&mut tx, FolioQuery::Reservation { property: hotel.property, reservation: booking.id })
            .await
            .unwrap()
    };

    assert_eq!(statements.load(Ordering::SeqCst), 4, "folios, lines, taxes and payments: one statement each");
    let shape: Vec<_> = views
        .iter()
        .map(|view| {
            (
                view.folio.id,
                view.folio.kind,
                view.folio.status,
                view.lines.iter().map(|line| line.line.id).collect(),
                view.payments.iter().map(|payment| payment.id).collect(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (booking.deposit, FolioKind::Deposit, FolioStatus::Open, vec![], vec![payment]),
            (booking.guests[0], FolioKind::Guest, FolioStatus::Open, vec![first.id, second.id], vec![]),
            (booking.guests[1], FolioKind::Guest, FolioStatus::Open, vec![third.id], vec![]),
        ]
    );
    assert!(views[1].lines.iter().all(|line| !line.taxes.is_empty()), "each line carries its own taxes");
    assert!(views[1].lines[1].taxes.iter().any(|tax| tax.code == "SC"), "the bar line's service charge");
    assert_eq!(views[0].balances, vec![Balance { currency: "LKR".into(), amount: -5000, base_amount: -5000 }]);
    assert_eq!(
        views[1].balances,
        vec![
            Balance { currency: "LKR".into(), amount: first.total, base_amount: first.base_total },
            Balance { currency: "USD".into(), amount: second.total, base_amount: second.base_total },
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_transfer_to_an_invoiced_folio_is_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (stay, other) = (hotel.book(None).await, hotel.book(None).await);
    let line = hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query("update folio set status = 'invoiced', version = version + 1 where id = $1")
        .bind(other.guests[0])
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    conflict(transfer(&hotel, line.id, other.guests[0]).await);
    assert_eq!(hotel.lines_on(&[stay.guests[0], other.guests[0]]).await.len(), 1, "nothing moved");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reversed_key_stays_used_and_its_copies_carry_no_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (stay, other) = (hotel.book(None).await, hotel.book(None).await);
    let key = |n: u8| format!("laundry:{}:{n}", stay.rooms[0]);
    let charge =
        |n: u8| NewCharge { posting_key: Some(key(n)), ..hotel.charge(Source::Laundry, "laundry", 700, "LKR") };
    let reversed = hotel.post(stay.guests[0], charge(1)).await.unwrap();
    let moved = hotel.post(stay.guests[0], charge(2)).await.unwrap();

    let reversal = reverse(&hotel, stay.guests[0], reversed.id).await.unwrap();
    let (out, into) = transfer(&hotel, moved.id, other.guests[0]).await.unwrap();
    assert_eq!((reversal.posting_key, out.posting_key, into.posting_key), (None, None, None));

    assert_eq!(hotel.post(stay.guests[0], charge(1)).await.unwrap(), reversed, "the replay is still a no-op");
    assert_eq!(hotel.post(other.guests[0], charge(2)).await.unwrap(), moved, "found on any folio of the property");
    assert_eq!(hotel.lines_on(&[stay.guests[0], other.guests[0]]).await.len(), 5);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_charge_on_another_property_s_folio_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let there = hotel.book_in(kandy, 1, None, None).await;

    let result = hotel.post(there.guests[0], hotel.charge(Source::Manual, "misc", 100, "LKR")).await;

    assert!(matches!(result, Err(FolioError::NotFound("folio"))), "{result:?}");
    assert!(hotel.lines_on(&[there.guests[0]]).await.is_empty());
}

/// A charge on `folio` of `property`, committed.
async fn post_in(hotel: &Hotel, property: Uuid, folio: Uuid) -> LineRow {
    let mut tx = hotel.tx().await;
    let charge = hotel.charge(Source::Manual, "misc", 100, "LKR");
    let line = folio::post_charge(&mut tx, hotel.tenant, Some(hotel.user), property, folio, charge).await.unwrap();
    tx.commit().await.unwrap();
    line
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reversing_on_another_property_s_folio_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let there = hotel.book_in(kandy, 1, None, None).await;
    let line = post_in(&hotel, kandy, there.guests[0]).await;

    let result = reverse(&hotel, there.guests[0], line.id).await;

    assert!(matches!(result, Err(FolioError::NotFound("folio"))), "{result:?}");
    assert_eq!(hotel.lines_on(&[there.guests[0]]).await.len(), 1, "nothing reversed");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn moving_another_property_s_line_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let here = hotel.book(None).await;
    let there = hotel.book_in(kandy, 1, None, None).await;
    let line = post_in(&hotel, kandy, there.guests[0]).await;

    let result = transfer(&hotel, line.id, here.guests[0]).await;

    assert!(matches!(result, Err(FolioError::NotFound("line"))), "{result:?}");
    assert_eq!(hotel.lines_on(&[here.guests[0], there.guests[0]]).await.len(), 1, "nothing moved");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_concurrent_reversals_of_a_line_make_one_reversal_and_a_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stay = hotel.book(None).await;
    let folio = stay.guests[0];
    let line = hotel.post(folio, hotel.charge(Source::Manual, "misc", 1_000, "LKR")).await.unwrap();

    let mut first = hotel.tx().await;
    folio::reverse(&mut first, hotel.tenant, Some(hotel.user), hotel.property, folio, line.id, "Twice").await.unwrap();
    let (pool, tenant, user, property) = (hotel.pool.clone(), hotel.tenant, hotel.user, hotel.property);
    let second = tokio::spawn(async move {
        let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
        folio::reverse(&mut tx, tenant, Some(user), property, folio, line.id, "Twice").await
    });
    hotel.until_blocked().await;
    first.commit().await.unwrap();

    conflict(second.await.unwrap());
    let reversals: i64 = sqlx::query_scalar("select count(*) from folio_line where reverses = $1")
        .bind(line.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(reversals, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn opposing_concurrent_transfers_do_not_deadlock(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (one, two) = (hotel.book(None).await, hotel.book(None).await);
    let (g1, g2) = (one.guests[0], two.guests[0]);
    let on_one = hotel.post(g1, hotel.charge(Source::Manual, "misc", 100, "LKR")).await.unwrap();
    let on_two = hotel.post(g2, hotel.charge(Source::Manual, "misc", 200, "LKR")).await.unwrap();

    let mut first = hotel.tx().await;
    folio::transfer(&mut first, hotel.tenant, Some(hotel.user), hotel.property, on_one.id, g2, "Swap").await.unwrap();
    let (pool, tenant, user, property) = (hotel.pool.clone(), hotel.tenant, hotel.user, hotel.property);
    let second = tokio::spawn(async move {
        let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
        let moved = folio::transfer(&mut tx, tenant, Some(user), property, on_two.id, g1, "Swap").await;
        if moved.is_ok() {
            tx.commit().await.unwrap();
        }
        moved
    });
    hotel.until_blocked().await;
    first.commit().await.unwrap();

    match second.await.unwrap() {
        Ok(_) | Err(FolioError::Conflict(_)) => {}
        Err(other) => panic!("expected the second transfer to finish or conflict, got {other:?}"),
    }
    let (one_view, two_view) = (view(&hotel, g1).await, view(&hotel, g2).await);
    assert_eq!(
        one_view.balances,
        vec![Balance { currency: "LKR".into(), amount: on_two.total, base_amount: on_two.base_total }]
    );
    assert_eq!(
        two_view.balances,
        vec![Balance { currency: "LKR".into(), amount: on_one.total, base_amount: on_one.base_total }]
    );
}
