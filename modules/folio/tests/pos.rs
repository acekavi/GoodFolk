mod common;

use common::Hotel;
use folio::{
    FolioError, FolioQuery, LineWithTaxes, Method, NewOutlet, OutletChanges, PaymentRow, PosBillIn, PosBillRow,
    PosItem, PosOutcome, PosTarget, Source,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn item(description: &str, quantity: i32, unit_net: i64, category: Option<&str>) -> PosItem {
    PosItem { description: description.into(), quantity, unit_net, category: category.map(Into::into) }
}

fn bill(outlet: &str, bill_no: &str, items: Vec<PosItem>, target: PosTarget) -> PosBillIn {
    PosBillIn {
        outlet: outlet.into(),
        bill_no: bill_no.into(),
        business_date: None,
        currency: "LKR".into(),
        items,
        target,
    }
}

fn cash() -> PosTarget {
    PosTarget::Settled { method: Method::Cash }
}

fn posted(outcome: PosOutcome) -> PosBillRow {
    match outcome {
        PosOutcome::Posted(bill) => bill,
        other => panic!("expected Posted, got {other:?}"),
    }
}

fn invalid<T: std::fmt::Debug>(result: Result<T, FolioError>) -> String {
    match result {
        Err(FolioError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// `(code, base, amount)` of each tax of `line`, in the preset's calculation order (a folio lists them by code).
fn taxes(line: &LineWithTaxes) -> Vec<(&str, i64, i64)> {
    let mut taxes: Vec<_> = line.taxes.iter().map(|tax| (tax.code.as_str(), tax.base, tax.amount)).collect();
    taxes.sort_by_key(|(code, ..)| ["SC", "TDL", "SSCL", "VAT"].iter().position(|preset| preset == code));
    taxes
}

/// The Sri Lanka preset's taxes of a 1,000,000 net `pos:` item, with service charge (the golden order A figures).
const WITH_SC: [(&str, i64, i64); 4] =
    [("SC", 1_000_000, 100_000), ("TDL", 1_000_000, 10_000), ("SSCL", 1_110_000, 27_750), ("VAT", 1_137_750, 204_795)];

impl Hotel {
    /// `post_pos_bill` in the hotel's property, in its own transaction, committed if it succeeds.
    async fn pos(&self, bill: PosBillIn) -> Result<PosOutcome, FolioError> {
        let mut tx = self.tx().await;
        let outcome = folio::post_pos_bill(&mut tx, self.tenant, Some(self.user), self.property, bill).await?;
        tx.commit().await.unwrap();
        Ok(outcome)
    }

    /// The lines of `folio`, with their taxes, oldest first.
    async fn folio_lines(&self, folio: Uuid) -> Vec<LineWithTaxes> {
        let query = FolioQuery::Folio { property: self.property, folio };
        folio::folios_for(&mut self.tx().await, query).await.unwrap().pop().expect("the folio").lines
    }

    async fn bills(&self) -> i64 {
        sqlx::query_scalar("select count(*) from pos_bill").fetch_one(&mut *self.tx().await).await.unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bill_to_a_room_posts_its_items_taxed_with_service_charge(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    hotel.outlet("BAR", "Pool Bar", true).await;
    let stay = hotel.book(None).await;
    hotel.mark_checked_in(stay.rooms[0]).await;

    let items = vec![item("Lion lager", 2, 500_000, None), item("Cigarettes", 1, 1_000, Some("misc"))];
    let bill = posted(hotel.pos(bill("BAR", "B-1", items, PosTarget::Stay(stay.rooms[0]))).await.unwrap());

    assert_eq!((bill.reservation_room_id, bill.settled, bill.lines.len()), (Some(stay.rooms[0]), false, 0));
    assert_eq!(bill.business_date, hotel.business_date, "the property's business date when the bill names none");
    let lines = hotel.folio_lines(stay.guests[0]).await;
    assert_eq!(lines.len(), 2);
    let (beer, misc) = (&lines[0], &lines[1]);
    assert_eq!(beer.line.source, Source::Pos);
    assert_eq!(beer.line.category, "pos:BAR");
    assert_eq!(beer.line.description, "Pool Bar: Lion lager");
    assert_eq!(beer.line.posting_key, Some(format!("pos:{}:1", bill.id)));
    assert_eq!((beer.line.quantity, beer.line.net, beer.line.total), (2, 1_000_000, 1_342_545));
    assert_eq!(taxes(beer), WITH_SC);
    // misc takes no service charge: TDL 10, SSCL 2.5% of 1,010 = 25, VAT 18% of 1,035 = 186.
    assert_eq!(misc.line.category, "misc");
    assert_eq!(misc.line.posting_key, Some(format!("pos:{}:2", bill.id)));
    assert_eq!(taxes(misc), vec![("TDL", 1_000, 10), ("SSCL", 1_010, 25), ("VAT", 1_035, 186)]);
    assert_eq!(misc.line.total, 1_221);

    let audited = hotel.audited("pos_bill.posted").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].0, bill.id);
    assert_eq!(audited[0].1["outlet"], "BAR");
    assert_eq!(audited[0].1["bill_no"], "B-1");
    assert_eq!(audited[0].1["total"], 1_342_545 + 1_221);
    assert_eq!(audited[0].1["target"], "stay");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_outlet_with_service_charge_off_gets_none_and_its_bases_exclude_it(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    hotel.outlet("SPA", "Spa", false).await;
    let stay = hotel.book(None).await;
    hotel.mark_checked_in(stay.rooms[0]).await;

    hotel
        .pos(bill("SPA", "S-1", vec![item("Massage", 1, 1_000_000, None)], PosTarget::Stay(stay.rooms[0])))
        .await
        .unwrap();

    // By hand: TDL 1% of 1,000,000 = 10,000; SSCL 2.5% of 1,010,000 = 25,250; VAT 18% of 1,035,250 = 186,345.
    let lines = hotel.folio_lines(stay.guests[0]).await;
    assert_eq!(
        taxes(&lines[0]),
        vec![("TDL", 1_000_000, 10_000), ("SSCL", 1_010_000, 25_250), ("VAT", 1_035_250, 186_345)]
    );
    assert_eq!((lines[0].line.net, lines[0].line.total), (1_000_000, 1_221_595));

    let settled =
        posted(hotel.pos(bill("SPA", "S-2", vec![item("Massage", 1, 1_000_000, None)], cash())).await.unwrap());
    assert_eq!(settled.lines[0].total, 1_221_595, "a settled bill is taxed the same way");
    assert!(settled.lines[0].taxes.iter().all(|tax| tax.code != "SC"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_replay_returns_the_original_and_a_changed_replay_conflicts(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;
    let stay = hotel.book(None).await;
    hotel.mark_checked_in(stay.rooms[0]).await;
    let request = bill("BAR", "B-7", vec![item("Tea", 2, 300, None)], PosTarget::Stay(stay.rooms[0]));

    let first = posted(hotel.pos(request.clone()).await.unwrap());
    let again = hotel.pos(request.clone()).await.unwrap();

    assert_eq!(again, PosOutcome::Replayed(first.clone()));
    assert_eq!(hotel.folio_lines(stay.guests[0]).await.len(), 1, "a replay posts nothing");
    assert_eq!(hotel.audited("pos_bill.posted").await.len(), 1);

    let changed = PosBillIn { items: vec![item("Tea", 3, 300, None)], ..request.clone() };
    match hotel.pos(changed).await {
        Err(FolioError::Conflict(message)) => {
            assert_eq!(message, "POS bill B-7 was already posted with different contents");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    let settled_instead = PosBillIn { target: cash(), ..request };
    assert!(matches!(hotel.pos(settled_instead).await, Err(FolioError::Conflict(_))), "the target is in the body");
    assert_eq!(hotel.bills().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn concurrent_identical_posts_make_one_bill(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;
    let stay = hotel.book(None).await;
    hotel.mark_checked_in(stay.rooms[0]).await;
    let request =
        bill("BAR", "B-9", vec![item("Tea", 1, 300, None), item("Cake", 1, 500, None)], PosTarget::Stay(stay.rooms[0]));

    let mut first = hotel.tx().await;
    let outcome = folio::post_pos_bill(&mut first, hotel.tenant, Some(hotel.user), hotel.property, request.clone())
        .await
        .unwrap();
    let original = posted(outcome);
    let (pool, tenant, user, property) = (hotel.pool.clone(), hotel.tenant, hotel.user, hotel.property);
    let second = tokio::spawn(async move {
        let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
        let outcome = folio::post_pos_bill(&mut tx, tenant, Some(user), property, request).await;
        tx.commit().await.unwrap();
        outcome
    });
    hotel.until_blocked().await;
    first.commit().await.unwrap();

    assert_eq!(second.await.unwrap().unwrap(), PosOutcome::Replayed(original));
    assert_eq!(hotel.bills().await, 1);
    assert_eq!(hotel.folio_lines(stay.guests[0]).await.len(), 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_settled_bill_records_a_payment_on_no_folio_and_keeps_its_lines(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_vat_registered(true).await;
    hotel.outlet("REST", "Restaurant", true).await;

    let items = vec![item("Rice and curry", 2, 500_000, None), item("Water", 1, 1_000, Some("misc"))];
    let settled = posted(hotel.pos(bill("REST", "R-1", items, cash())).await.unwrap());

    assert_eq!((settled.reservation_room_id, settled.settled), (None, true));
    assert_eq!(settled.lines.len(), 2);
    let rice = &settled.lines[0];
    assert_eq!(
        (rice.description.as_str(), rice.quantity, rice.unit_net, rice.category.as_str()),
        ("Rice and curry", 2, 500_000, "pos:REST")
    );
    assert_eq!((rice.net, rice.total), (1_000_000, 1_342_545));
    let rice_taxes: Vec<(&str, i64, i64)> =
        rice.taxes.iter().map(|tax| (tax.code.as_str(), tax.base, tax.amount)).collect();
    assert_eq!(rice_taxes, WITH_SC);
    assert_eq!((settled.lines[1].category.as_str(), settled.lines[1].total), ("misc", 1_221));

    let payments: Vec<PaymentRow> = sqlx::query_as(
        "select id, folio_id, method, amount, currency::text as currency, base_amount, rate, rate_date, details,
                evidence, refund_of, business_date, transfer_id, taken_by, taken_at, pos_bill_id
         from payment where pos_bill_id = $1",
    )
    .bind(settled.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(payments.len(), 1);
    let payment = &payments[0];
    assert_eq!(payment.folio_id, None);
    assert_eq!((payment.method.as_str(), payment.amount, payment.base_amount), ("cash", 1_343_766, 1_343_766));
    assert_eq!(payment.business_date, hotel.business_date);
    let audited = hotel.audited("pos_bill.posted").await;
    assert_eq!((audited[0].1["target"].as_str(), audited[0].1["total"].as_i64()), (Some("settled"), Some(1_343_766)));

    let usd =
        PosBillIn { currency: "USD".into(), ..bill("REST", "R-2", vec![item("Dinner", 1, 1_000_000, None)], cash()) };
    match hotel.pos(usd.clone()).await {
        Err(FolioError::Conflict(message)) => {
            assert_eq!(message, format!("No exchange rate for USD on {}", hotel.business_date));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    hotel.rate("USD", rust_decimal::Decimal::new(300_000_000, 6)).await;
    let usd_bill = posted(hotel.pos(usd).await.unwrap());
    let base: i64 = sqlx::query_scalar("select base_amount from payment where pos_bill_id = $1")
        .bind(usd_bill.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(usd_bill.lines[0].total, 1_342_545);
    assert_eq!(base, 1_342_545 * 300, "base at the business date's rate");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_settled_bill_is_never_charged_to_account_and_never_holds_a_card_number(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;

    let to_account =
        bill("BAR", "B-1", vec![item("Tea", 1, 300, None)], PosTarget::Settled { method: Method::Account });
    invalid(hotel.pos(to_account).await);
    let card =
        Method::CardTerminal { approval: "4111 1111 1111 1111".into(), last4: "1111".into(), brand: "VISA".into() };
    invalid(hotel.pos(bill("BAR", "B-2", vec![item("Tea", 1, 300, None)], PosTarget::Settled { method: card })).await);
    assert_eq!(hotel.bills().await, 0, "nothing is kept of a refused bill");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_not_in_house_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;
    let stay = hotel.book(None).await;

    let message =
        invalid(hotel.pos(bill("BAR", "B-1", vec![item("Tea", 1, 300, None)], PosTarget::Stay(stay.rooms[0]))).await);

    assert_eq!(message, "Room is not in house");
    assert_eq!(hotel.bills().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn outlets_and_stays_of_another_property_are_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let kandy = hotel.add_property("KAN").await;
    let here = hotel.outlet("BAR", "Bar", true).await;
    let mut tx = hotel.tx().await;
    let input = NewOutlet { code: "KBAR".into(), name: "Kandy bar".into(), service_charge: true };
    folio::create_outlet(&mut tx, hotel.tenant, Some(hotel.user), kandy, input).await.unwrap();
    tx.commit().await.unwrap();
    let there = hotel.book_in(kandy, 1, None, None).await;
    hotel.mark_checked_in(there.rooms[0]).await;

    let tea = || vec![item("Tea", 1, 300, None)];
    let result = hotel.pos(bill("KBAR", "K-1", tea(), cash())).await;
    assert!(matches!(result, Err(FolioError::NotFound("outlet"))), "{result:?}");
    let result = hotel.pos(bill("BAR", "B-1", tea(), PosTarget::Stay(there.rooms[0]))).await;
    assert!(matches!(result, Err(FolioError::NotFound("reservation room"))), "{result:?}");

    let mut tx = hotel.tx().await;
    let result = folio::update_outlet(
        &mut tx,
        hotel.tenant,
        Some(hotel.user),
        kandy,
        here.id,
        here.version,
        OutletChanges::default(),
    )
    .await;
    assert!(matches!(result, Err(FolioError::NotFound("outlet"))), "{result:?}");
    assert_eq!(folio::list_outlets(&mut tx, hotel.property).await.unwrap(), vec![here]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bill_is_checked_before_it_is_posted(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;
    let tea = || item("Tea", 1, 300, None);

    invalid(hotel.pos(bill("BAR", "B-1", vec![], cash())).await);
    invalid(hotel.pos(bill("BAR", "B-1", vec![tea(); 201], cash())).await);
    assert!(hotel.pos(bill("BAR", "B-200", vec![tea(); 200], cash())).await.is_ok());
    invalid(hotel.pos(bill("BAR", "", vec![tea()], cash())).await);
    invalid(hotel.pos(bill("BAR", &"9".repeat(51), vec![tea()], cash())).await);
    for padded in [" B-7", "B-7 ", "B-7\t"] {
        assert_eq!(
            invalid(hotel.pos(bill("BAR", padded, vec![tea()], cash())).await),
            "a bill number has no spaces before or after it"
        );
    }
    invalid(hotel.pos(PosBillIn { currency: "JPY".into(), ..bill("BAR", "B-2", vec![tea()], cash()) }).await);
    invalid(hotel.pos(bill("BAR", "B-3", vec![item("Tea", 1, 300, Some("pos:SPA"))], cash())).await);
    invalid(hotel.pos(bill("BAR", "B-4", vec![item(" ", 1, 300, None)], cash())).await);
    invalid(hotel.pos(bill("BAR", "B-5", vec![item("Tea", 0, 300, None)], cash())).await);
    let long = posted(hotel.pos(bill("BAR", "B-6", vec![item(&"x".repeat(200), 1, 300, None)], cash())).await.unwrap());
    assert_eq!(long.lines[0].description.len(), 200);
    assert_eq!(hotel.bills().await, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_long_item_description_is_capped_on_the_folio(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.outlet("BAR", "Bar", true).await;
    let stay = hotel.book(None).await;
    hotel.mark_checked_in(stay.rooms[0]).await;

    let items = vec![item(&"x".repeat(200), 1, 300, None)];
    hotel.pos(bill("BAR", "B-1", items, PosTarget::Stay(stay.rooms[0]))).await.unwrap();

    let description = hotel.folio_lines(stay.guests[0]).await[0].line.description.clone();
    assert_eq!(description, format!("Bar: {}", "x".repeat(195)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn outlets_are_created_updated_and_listed_with_unique_codes(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.outlet("BAR", "Pool Bar", true).await;
    assert_eq!(
        (bar.code.as_str(), bar.name.as_str(), bar.service_charge, bar.active, bar.version),
        ("BAR", "Pool Bar", true, true, 1)
    );

    let mut tx = hotel.tx().await;
    let duplicate = NewOutlet { code: "BAR".into(), name: "Other".into(), service_charge: false };
    let result = folio::create_outlet(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, duplicate).await;
    assert!(matches!(result, Err(FolioError::Conflict(_))), "{result:?}");
    drop(tx);
    for (code, name) in [("bar", "Bar"), ("B-1", "Bar"), ("ABCDEFGHIJK", "Bar"), ("SPA", " ")] {
        let mut tx = hotel.tx().await;
        let input = NewOutlet { code: code.into(), name: name.into(), service_charge: true };
        invalid(folio::create_outlet(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, input).await);
    }
    let spa = hotel.outlet("SPA", "Spa", false).await;

    let past = posted(hotel.pos(bill("BAR", "B-1", vec![item("Tea", 1, 300, None)], cash())).await.unwrap());
    let mut tx = hotel.tx().await;
    let stale = folio::update_outlet(
        &mut tx,
        hotel.tenant,
        Some(hotel.user),
        hotel.property,
        bar.id,
        bar.version + 1,
        OutletChanges { active: Some(false), ..OutletChanges::default() },
    )
    .await;
    assert!(matches!(stale, Err(FolioError::VersionMismatch("outlet"))), "{stale:?}");
    let changes = OutletChanges { name: Some(" Lobby Bar ".into()), service_charge: Some(false), active: Some(false) };
    let updated =
        folio::update_outlet(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, bar.id, bar.version, changes)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        (updated.name.as_str(), updated.service_charge, updated.active, updated.version),
        ("Lobby Bar", false, false, 2)
    );

    let replay = hotel.pos(bill("BAR", "B-1", vec![item("Tea", 1, 300, None)], cash())).await.unwrap();
    assert_eq!(replay, PosOutcome::Replayed(past), "a deactivated outlet keeps its past bills");
    invalid(hotel.pos(bill("BAR", "B-2", vec![item("Tea", 1, 300, None)], cash())).await);

    let listed = folio::list_outlets(&mut hotel.tx().await, hotel.property).await.unwrap();
    assert_eq!(listed, vec![updated.clone(), spa]);
    let audited = hotel.audited("outlet.created").await;
    assert_eq!(audited.len(), 2);
    let audited = hotel.audited("outlet.updated").await;
    assert_eq!(
        audited,
        vec![(
            bar.id,
            serde_json::json!({
                "name": "Lobby Bar",
                "service_charge": false,
                "active": false,
                "previous": { "name": "Pool Bar", "service_charge": true, "active": true },
            })
        )]
    );

    // Changes that change nothing are refused, and nothing is audited.
    for changes in [
        OutletChanges::default(),
        OutletChanges { name: Some("Lobby Bar ".into()), service_charge: Some(false), active: Some(false) },
    ] {
        let mut tx = hotel.tx().await;
        let result =
            folio::update_outlet(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, bar.id, 2, changes).await;
        assert_eq!(invalid(result), "Nothing to change");
    }
    assert_eq!(hotel.audited("outlet.updated").await.len(), 1);
}
