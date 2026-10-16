mod common;

use common::Hotel;
use folio::{Balance, FolioError, FolioQuery, FolioView, Method, PaymentRow, Source};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn invalid<T: std::fmt::Debug>(result: Result<T, FolioError>) -> String {
    match result {
        Err(FolioError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn not_found<T: std::fmt::Debug>(result: Result<T, FolioError>) {
    assert!(matches!(result, Err(FolioError::NotFound(_))), "expected NotFound, got {result:?}");
}

fn card(approval: &str, last4: &str, brand: &str) -> Method {
    Method::CardTerminal { approval: approval.into(), last4: last4.into(), brand: brand.into() }
}

impl Hotel {
    /// `take_payment` in the hotel's property, in its own transaction, committed if it succeeds.
    async fn pay(
        &self,
        folio: Uuid,
        method: Method,
        amount: i64,
        currency: &str,
        evidence: Option<&str>,
    ) -> Result<PaymentRow, FolioError> {
        self.pay_in(self.property, folio, method, amount, currency, evidence).await
    }

    async fn pay_in(
        &self,
        property: Uuid,
        folio: Uuid,
        method: Method,
        amount: i64,
        currency: &str,
        evidence: Option<&str>,
    ) -> Result<PaymentRow, FolioError> {
        let mut tx = self.tx().await;
        let payment = folio::take_payment(
            &mut tx,
            self.tenant,
            Some(self.user),
            property,
            folio,
            method,
            amount,
            currency,
            evidence,
        )
        .await?;
        tx.commit().await.unwrap();
        Ok(payment)
    }

    /// `refund` in `property`, in its own transaction, committed if it succeeds.
    async fn refund_in(&self, property: Uuid, payment: Uuid, amount: i64) -> Result<PaymentRow, FolioError> {
        let mut tx = self.tx().await;
        let refund =
            folio::refund(&mut tx, self.tenant, Some(self.user), property, payment, amount, "Overpaid").await?;
        tx.commit().await.unwrap();
        Ok(refund)
    }

    async fn refund(&self, payment: Uuid, amount: i64) -> Result<PaymentRow, FolioError> {
        self.refund_in(self.property, payment, amount).await
    }

    /// `move_deposit` in `property`, in its own transaction, committed if it succeeds.
    async fn move_deposit_in(&self, property: Uuid, reservation: Uuid, stay: Uuid) -> Result<(), FolioError> {
        let mut tx = self.tx().await;
        folio::move_deposit(&mut tx, self.tenant, Some(self.user), property, reservation, stay).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    /// `post_cancellation_fee` for `stay` in `property`, exclusive, in its own transaction, committed if it
    /// succeeds.
    async fn cancellation_fee_in(
        &self,
        property: Uuid,
        stay: Uuid,
        amount: i64,
        currency: &str,
    ) -> Result<(), FolioError> {
        let mut tx = self.tx().await;
        folio::post_cancellation_fee(&mut tx, self.tenant, Some(self.user), property, stay, amount, currency, false)
            .await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    async fn view(&self, folio: Uuid) -> FolioView {
        let query = FolioQuery::Folio { property: self.property, folio };
        folio::folios_for(&mut self.tx().await, query).await.unwrap().pop().expect("the folio")
    }

    /// Marks `stay` checked in, as `reservations::check_in` does before it moves the deposit.
    async fn mark_checked_in(&self, stay: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query(
            "update reservation_room rr
             set status = 'checked_in', checked_in_at = now(), checked_in_business_date = p.business_date
             from property p where rr.id = $1 and p.id = rr.property_id",
        )
        .bind(stay)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_usd_card_payment_stores_its_base_amount_at_the_business_date_s_rate(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(3_005, 1)).await;
    let stay = hotel.book(None).await;

    let paid = hotel
        .pay(stay.guests[0], card(" A1B2C3 ", "4242", " Visa "), 10_050, "USD", Some(" Encashment receipt 77 "))
        .await
        .unwrap();

    // 100.50 USD × 300.5 = 30,200.25 LKR.
    assert_eq!((paid.amount, paid.currency.as_str(), paid.base_amount), (10_050, "USD", 3_020_025));
    assert_eq!(
        (paid.rate, paid.rate_date, paid.business_date),
        (Decimal::new(3_005, 1), hotel.business_date, hotel.business_date)
    );
    assert_eq!(paid.method, "card_terminal");
    assert_eq!(paid.details, serde_json::json!({ "approval": "A1B2C3", "last4": "4242", "brand": "Visa" }));
    assert_eq!(paid.evidence.as_deref(), Some("Encashment receipt 77"));
    assert_eq!((paid.folio_id, paid.taken_by, paid.refund_of), (Some(stay.guests[0]), Some(hotel.user.0), None));
    assert_eq!(
        hotel.view(stay.guests[0]).await.balances,
        vec![Balance { currency: "USD".into(), amount: -10_050, base_amount: -3_020_025 }]
    );
    assert_eq!(
        hotel.audited("folio.payment_taken").await,
        vec![(
            stay.guests[0],
            serde_json::json!({
                "payment": paid.id,
                "folio": stay.guests[0],
                "amount": 10_050,
                "currency": "USD",
                "method": "card_terminal",
                "brand": "Visa",
                "last4": "4242",
            })
        )],
        "never the approval code"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_payment_needs_a_positive_amount_an_accepted_currency_and_a_rate(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.book(None).await.guests[0];

    for amount in [0, -1] {
        assert_eq!(invalid(hotel.pay(folio, Method::Cash, amount, "LKR", None).await), "a payment is more than 0");
    }
    assert_eq!(
        invalid(hotel.pay(folio, Method::Cash, 100, "JPY", None).await),
        "JPY is not a currency this property accepts"
    );
    match hotel.pay(folio, Method::Cash, 100, "EUR", None).await {
        Err(FolioError::Conflict(message)) => {
            assert_eq!(message, format!("No exchange rate for EUR on {}", hotel.business_date));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert!(hotel.view(folio).await.payments.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_card_number_is_never_recorded(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.book(None).await.guests[0];

    for number in ["4111111111111111", "4111 1111 1111 1111", "4111-1111-1111-1111"] {
        let refused = [
            hotel.pay(folio, card(number, "1111", "Visa"), 100, "LKR", None).await,
            hotel.pay(folio, card("A1", "1111", number), 100, "LKR", None).await,
            hotel.pay(folio, Method::BankTransfer { reference: format!("Ref {number}") }, 100, "LKR", None).await,
            hotel.pay(folio, Method::Cash, 100, "LKR", Some(&format!("Card {number}"))).await,
        ];
        for result in refused {
            let message = invalid(result);
            assert!(message.contains("card number"), "{message}");
        }
    }
    for last4 in ["411", "41111", "41a1", " 4111"] {
        assert_eq!(
            invalid(hotel.pay(folio, card("A1", last4, "Visa"), 100, "LKR", None).await),
            "the card's last 4 are exactly 4 digits"
        );
    }
    for brand in ["", "  ", &"V".repeat(21)] {
        assert_eq!(
            invalid(hotel.pay(folio, card("A1", "1111", brand), 100, "LKR", None).await),
            "the card brand has 1 to 20 characters"
        );
    }
    let paid = hotel.pay(folio, card("A1", "1111", &"V".repeat(20)), 100, "LKR", Some("  ")).await.unwrap();
    assert_eq!(paid.evidence, None, "blank evidence is none");
    let refund = hotel.refund(paid.id, 1).await;
    assert!(refund.is_ok());
    let mut tx = hotel.tx().await;
    let message = invalid(
        folio::refund(&mut tx, hotel.tenant, None, hotel.property, paid.id, 1, "card 4111 1111 1111 1111").await,
    );
    assert!(message.contains("card number"), "{message}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn refunds_never_add_up_to_more_than_the_payment(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::from(300)).await;
    let folio = hotel.book(None).await.guests[0];
    let paid = hotel.pay(folio, Method::BankTransfer { reference: "TT-1".into() }, 10_000, "USD", None).await.unwrap();

    assert_eq!(invalid(hotel.refund(paid.id, 10_001).await), "at most 10000 of the payment is left to refund");
    let first = hotel.refund(paid.id, 4_000).await.unwrap();
    let second = hotel.refund(paid.id, 6_000).await.unwrap();
    assert_eq!(invalid(hotel.refund(paid.id, 1).await), "at most 0 of the payment is left to refund");
    assert_eq!(invalid(hotel.refund(paid.id, 0).await), "a refund is more than 0");

    for (refund, amount) in [(&first, -4_000), (&second, -6_000)] {
        assert_eq!((refund.amount, refund.base_amount), (amount, amount * 300));
        assert_eq!((refund.method.as_str(), refund.currency.as_str()), ("bank_transfer", "USD"));
        assert_eq!((refund.refund_of, refund.folio_id), (Some(paid.id), Some(folio)));
        assert_eq!(refund.details, serde_json::json!({ "reason": "Overpaid" }));
    }
    assert_eq!(invalid(hotel.refund(first.id, 1).await), "a refund can't be refunded");
    assert_eq!(hotel.view(folio).await.balances, vec![Balance { currency: "USD".into(), amount: 0, base_amount: 0 }]);
    let audited = hotel.audited("folio.refunded").await;
    assert_eq!(audited.len(), 2);
    assert_eq!(
        audited[0],
        (
            folio,
            serde_json::json!({
                "payment": first.id,
                "refund_of": paid.id,
                "folio": folio,
                "amount": -4_000,
                "currency": "USD",
                "method": "bank_transfer",
                "reason": "Overpaid",
            })
        )
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn concurrent_refunds_of_one_payment_run_one_at_a_time(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.book(None).await.guests[0];
    let paid = hotel.pay(folio, Method::Cash, 10_000, "LKR", None).await.unwrap();

    let mut first = hotel.tx().await;
    folio::refund(&mut first, hotel.tenant, Some(hotel.user), hotel.property, paid.id, 6_000, "Overpaid")
        .await
        .unwrap();
    let (pool, tenant, user, property) = (hotel.pool.clone(), hotel.tenant, hotel.user, hotel.property);
    let second = tokio::spawn(async move {
        let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
        let refund = folio::refund(&mut tx, tenant, Some(user), property, paid.id, 6_000, "Overpaid").await;
        if refund.is_ok() {
            tx.commit().await.unwrap();
        }
        refund
    });
    hotel.until_blocked().await;
    first.commit().await.unwrap();

    assert_eq!(invalid(second.await.unwrap()), "at most 4000 of the payment is left to refund");
    let refunded: i64 = sqlx::query_scalar("select -sum(amount)::bigint from payment where refund_of = $1")
        .bind(paid.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(refunded, 6_000);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_charge_to_account_credits_the_stay_and_charges_the_account_untaxed(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    let acme = hotel.account().await;
    let booking = hotel.book(Some(acme)).await;
    let guest = booking.guests[0];
    let spa = hotel.post(guest, hotel.charge(Source::Manual, "misc", 10_000, "LKR")).await.unwrap();

    let paid = hotel.pay(guest, Method::Account, spa.total, "LKR", None).await.unwrap();

    let account =
        folio::folios_for(&mut hotel.tx().await, FolioQuery::Account { property: hotel.property, account: acme })
            .await
            .unwrap()
            .pop()
            .expect("the account's folio");
    let [line] = &account.lines[..] else { panic!("one line on the account's folio: {:?}", account.lines) };
    assert_eq!((line.line.source, line.line.category.as_str()), (Source::Adjustment, "misc"));
    assert_eq!(line.line.description, "Charged to account from RES-000001");
    assert_eq!((line.line.net, line.line.total, line.line.base_total), (spa.total, spa.total, spa.total));
    assert!(line.taxes.is_empty(), "taxed once, on the stay: {:?}", line.taxes);
    assert_eq!((paid.method.as_str(), paid.amount), ("account", spa.total));
    assert!(paid.transfer_id.is_some());
    assert_eq!(line.line.transfer_id, paid.transfer_id, "the two halves are linked");
    assert_eq!(hotel.view(guest).await.balances, vec![Balance { currency: "LKR".into(), amount: 0, base_amount: 0 }]);
    let (_, audited) = &hotel.audited("folio.payment_taken").await[0];
    assert_eq!(audited["account_folio"], serde_json::json!(account.folio.id));

    assert_eq!(invalid(hotel.refund(paid.id, 1).await), "Move the account charge back to the stay instead");
    let mut tx = hotel.tx().await;
    let reversed =
        folio::reverse(&mut tx, hotel.tenant, None, hotel.property, account.folio.id, line.line.id, "Oops").await;
    assert_eq!(invalid(reversed), "Move the account charge back to the stay instead");
    drop(tx);

    // The undo: the account's line moves back to the stay.
    let mut tx = hotel.tx().await;
    folio::transfer(&mut tx, hotel.tenant, None, hotel.property, line.line.id, guest, "Guest pays").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        hotel.view(account.folio.id).await.balances,
        vec![Balance { currency: "LKR".into(), amount: 0, base_amount: 0 }]
    );
    assert_eq!(
        hotel.view(guest).await.balances,
        vec![Balance { currency: "LKR".into(), amount: spa.total, base_amount: spa.base_total }],
        "the stay owes the spa again"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_reservation_billed_to_an_account_charges_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book(None).await;
    let acme = hotel.account().await;
    let mut tx = hotel.tx().await;
    let account_folio = folio::ensure_account_folio(&mut tx, hotel.tenant, hotel.property, acme).await.unwrap();
    tx.commit().await.unwrap();

    for folio in [booking.guests[0], booking.deposit, account_folio] {
        let message = invalid(hotel.pay(folio, Method::Account, 100, "LKR", None).await);
        assert_eq!(message, "only a stay or deposit of a reservation billed to an account can charge it");
    }
    assert!(hotel.audited("folio.payment_taken").await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_deposit_moves_to_the_first_stay_checked_in_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::from(300)).await;
    let booking = hotel.book_in(hotel.property, 2, None, None).await;
    hotel.pay(booking.deposit, Method::Cash, 10_000, "LKR", None).await.unwrap();
    hotel.pay(booking.deposit, card("A1", "4242", "Visa"), 5_000, "USD", None).await.unwrap();
    // A later rate: the deposit moves at the base amounts it was paid at.
    hotel.rate("USD", Decimal::from(310)).await;

    hotel.mark_checked_in(booking.rooms[0]).await;
    hotel.move_deposit_in(hotel.property, booking.id, booking.rooms[0]).await.unwrap();

    let zero = |currency: &str| Balance { currency: currency.into(), amount: 0, base_amount: 0 };
    assert_eq!(hotel.view(booking.deposit).await.balances, vec![zero("LKR"), zero("USD")]);
    let guest = hotel.view(booking.guests[0]).await;
    assert_eq!(
        guest.balances,
        vec![
            Balance { currency: "LKR".into(), amount: -10_000, base_amount: -10_000 },
            Balance { currency: "USD".into(), amount: -5_000, base_amount: -1_500_000 },
        ]
    );
    let deposit = hotel.view(booking.deposit).await;
    for moved in &guest.payments {
        assert_eq!(moved.method, "transfer");
        let pair = deposit.payments.iter().find(|payment| payment.transfer_id == moved.transfer_id).expect("its pair");
        assert_eq!((pair.amount, pair.base_amount), (-moved.amount, -moved.base_amount));
        assert_eq!(invalid(hotel.refund(moved.id, 1).await), "a transfer between folios can't be refunded");
    }
    assert_eq!(hotel.audited("folio.deposit_moved").await.len(), 1);

    // A deposit taken after the first check-in stays where it is.
    hotel.pay(booking.deposit, Method::Cash, 2_000, "LKR", None).await.unwrap();
    hotel.mark_checked_in(booking.rooms[1]).await;
    hotel.move_deposit_in(hotel.property, booking.id, booking.rooms[1]).await.unwrap();

    assert!(hotel.view(booking.guests[1]).await.payments.is_empty());
    assert_eq!(
        hotel.view(booking.deposit).await.balances,
        vec![Balance { currency: "LKR".into(), amount: -2_000, base_amount: -2_000 }, zero("USD")]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn moving_an_empty_deposit_does_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book(None).await;

    hotel.move_deposit_in(hotel.property, booking.id, booking.rooms[0]).await.unwrap();

    assert!(hotel.view(booking.guests[0]).await.payments.is_empty());
    assert!(hotel.view(booking.deposit).await.payments.is_empty());
    assert!(hotel.audited("folio.deposit_moved").await.is_empty());
    let other = hotel.book(None).await;
    not_found(hotel.move_deposit_in(hotel.property, booking.id, other.rooms[0]).await);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cancellation_posts_its_penalty_to_the_deposit_folio_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book(None).await;

    hotel.cancellation_fee_in(hotel.property, booking.rooms[0], 10_000, "LKR").await.unwrap();
    hotel.cancellation_fee_in(hotel.property, booking.rooms[0], 10_000, "LKR").await.unwrap();

    let deposit = hotel.view(booking.deposit).await;
    let [line] = &deposit.lines[..] else { panic!("one line: {:?}", deposit.lines) };
    assert_eq!((line.line.source, line.line.category.as_str()), (Source::CancellationFee, "fee"));
    assert_eq!(line.line.posting_key, Some(format!("cancellation:{}", booking.rooms[0])));
    assert_eq!(line.line.net, 10_000);
    let codes: Vec<&str> = line.taxes.iter().map(|tax| tax.code.as_str()).collect();
    assert_eq!(codes, ["SSCL", "TDL"], "a penalty is a supply, taxed under the \"*\" rules");
    assert_eq!(line.line.total, 10_000 + 100 + 253);

    let free = hotel.book(None).await;
    hotel.cancellation_fee_in(hotel.property, free.rooms[0], 0, "LKR").await.unwrap();
    assert!(hotel.view(free.deposit).await.lines.is_empty(), "a free cancellation posts nothing");
    assert_eq!(
        invalid(hotel.cancellation_fee_in(hotel.property, free.rooms[0], -1, "LKR").await),
        "a cancellation penalty is not negative"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_payment_command_is_confined_to_its_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let acme = hotel.account().await;
    let there = hotel.book_in(kandy, 1, Some(acme), None).await;
    let here = hotel.book(None).await;

    // A folio of Kandy, named from here.
    not_found(hotel.pay(there.guests[0], Method::Cash, 100, "LKR", None).await);
    not_found(hotel.pay(there.guests[0], Method::Account, 100, "LKR", None).await);
    // A payment of Kandy, refunded from here.
    let paid = hotel.pay_in(kandy, there.deposit, Method::Cash, 1_000, "LKR", None).await.unwrap();
    not_found(hotel.refund(paid.id, 100).await);
    // Kandy's reservation and stay, moved or charged from here.
    not_found(hotel.move_deposit_in(hotel.property, there.id, there.rooms[0]).await);
    not_found(hotel.move_deposit_in(kandy, here.id, here.rooms[0]).await);
    not_found(hotel.cancellation_fee_in(hotel.property, there.rooms[0], 100, "LKR").await);

    assert_eq!(hotel.lines_on(&[there.deposit, there.guests[0]]).await, vec![]);
    let payments: i64 =
        sqlx::query_scalar("select count(*) from payment").fetch_one(&mut *hotel.tx().await).await.unwrap();
    assert_eq!(payments, 1, "only Kandy's own payment");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_refund_after_the_rate_changes_still_nets_the_payment_to_zero_in_lkr(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    hotel.rate("USD", Decimal::new(3_005, 1)).await;
    let folio = hotel.book(None).await.guests[0];
    let paid = hotel.pay(folio, Method::Cash, 10_001, "USD", None).await.unwrap();
    assert_eq!(paid.base_amount, 3_005_301, "100.01 × 300.5 = 30,053.005");
    hotel.rate("USD", Decimal::from(320)).await;

    let first = hotel.refund(paid.id, 3_333).await.unwrap();
    let last = hotel.refund(paid.id, 6_668).await.unwrap();

    assert_eq!(first.base_amount, -1_001_567, "3,005,301 × 3,333 / 10,001, half-up");
    assert_eq!(last.base_amount, -(3_005_301 - 1_001_567), "the last refund takes exactly the base left");
    for refund in [&first, &last] {
        assert_eq!((refund.rate, refund.rate_date), (paid.rate, paid.rate_date), "the original's rate");
    }
    assert_eq!(hotel.view(folio).await.balances, vec![Balance { currency: "USD".into(), amount: 0, base_amount: 0 }]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_moved_deposit_is_refunded_from_the_stay_up_to_the_credit_it_holds(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book(None).await;
    let (deposit, guest) = (booking.deposit, booking.guests[0]);
    let paid = hotel.pay(deposit, Method::Cash, 10_000, "LKR", None).await.unwrap();
    hotel.mark_checked_in(booking.rooms[0]).await;
    hotel.move_deposit_in(hotel.property, booking.id, booking.rooms[0]).await.unwrap();

    let refund = hotel.refund(paid.id, 4_000).await.unwrap();

    assert_eq!((refund.folio_id, refund.refund_of, refund.amount), (Some(guest), Some(paid.id), -4_000));
    let lkr = |amount: i64| vec![Balance { currency: "LKR".into(), amount, base_amount: amount }];
    assert_eq!(hotel.view(guest).await.balances, lkr(-6_000));
    assert_eq!(hotel.view(deposit).await.balances, lkr(0));

    // The stay spends 5176 (5000 and its taxes) of the 6000 credit; only 824 is left to refund.
    let spent = hotel.post(guest, hotel.charge(Source::Manual, "misc", 5_000, "LKR")).await.unwrap();
    assert_eq!(spent.total, 5_176);
    let message = invalid(hotel.refund(paid.id, 825).await);
    assert_eq!(message, "the stay's folio holds only 824 LKR of the deposit to refund");
    assert!(hotel.refund(paid.id, 824).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invoiced_folio_takes_no_payment_and_no_refund(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.book(None).await.guests[0];
    let paid = hotel.pay(folio, Method::Cash, 1_000, "LKR", None).await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query("update folio set status = 'invoiced' where id = $1").bind(folio).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    for result in [hotel.pay(folio, Method::Cash, 1_000, "LKR", None).await, hotel.refund(paid.id, 100).await] {
        assert!(matches!(result, Err(FolioError::Conflict(_))), "expected Conflict, got {result:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn payment_texts_have_length_limits(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let folio = hotel.book(None).await.guests[0];
    let long = |n: usize| "A".repeat(n);

    assert!(hotel.pay(folio, card(&long(50), "1111", "Visa"), 100, "LKR", Some(&long(500))).await.is_ok());
    assert!(hotel.pay(folio, Method::BankTransfer { reference: long(100) }, 100, "LKR", None).await.is_ok());
    let refused = [
        (
            hotel.pay(folio, card(&long(51), "1111", "Visa"), 100, "LKR", None).await,
            "the approval code has 1 to 50 characters",
        ),
        (
            hotel.pay(folio, card(" ", "1111", "Visa"), 100, "LKR", None).await,
            "the approval code has 1 to 50 characters",
        ),
        (
            hotel.pay(folio, Method::BankTransfer { reference: long(101) }, 100, "LKR", None).await,
            "the bank reference has 1 to 100 characters",
        ),
        (hotel.pay(folio, Method::Cash, 100, "LKR", Some(&long(501))).await, "the evidence has 1 to 500 characters"),
    ];
    for (result, message) in refused {
        assert_eq!(invalid(result), message);
    }
}
