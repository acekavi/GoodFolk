//! The night-audit review (`audit::night_audit`) and its blocking count (`audit::blocking`), on stays seeded straight
//! into the reservation tables.

mod common;

use audit::{ArrivalRow, BookingBills, CategoryTotal, DepartureRow, OutletBills, PaymentGroup, PosBillRow};
use common::{Booked, Hotel, Stay};
use domain::RoomStatus;
use folio::{Method, NightPreview, PosTarget, PostReport};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// The seeded day's stays and money, as the lists show them.
struct Day {
    confirmed: Booked,
    tentative: Booked,
    overdue: Booked,
    departing: Booked,
    in_house: Booked,
    fit: Booked,
    posted_bill: Uuid,
    settled_bill: Uuid,
    settled_total: i64,
    card_base: i64,
    flagged: Uuid,
    misc_base: i64,
    departing_balance: i64,
    laundry_base: i64,
}

/// One each of: a confirmed, a tentative and an overdue (D − 1) arrival; a due departure and two in-house stays (one
/// in LKR, one in USD on a `FIT_F` plan); a POS bill posted to a stay and one settled at the outlet; a cash payment
/// with a refund, a USD card payment, and an LKR payment without evidence on the `FIT_F` stay. A future arrival and a
/// stay checked out today are in none of the lists.
async fn seed(hotel: &Hotel) -> Day {
    let p = hotel.property;
    hotel.rate(p, "USD", Decimal::new(300, 0)).await;
    let terms = serde_json::json!({ "rules": [], "no_show": { "kind": "nights", "value": 1 } });
    let confirmed = hotel
        .stay(Stay {
            room: Some("101"),
            meal_amount: 2_000,
            terms: Some(terms),
            first_name: "Kamal",
            last_name: "Perera",
            ..Stay::new(p, "confirmed", 0, 2)
        })
        .await;
    let tentative = hotel.stay(Stay { first_name: "", last_name: "Fernando", ..Stay::new(p, "tentative", 0, 1) }).await;
    let overdue = hotel.stay(Stay { room: Some("103"), ..Stay::new(p, "confirmed", -1, 2) }).await;
    hotel.stay(Stay { room: Some("108"), ..Stay::new(p, "confirmed", 1, 3) }).await;
    let departing = hotel.stay(Stay { room: Some("104"), ..Stay::new(p, "checked_in", -2, 0) }).await;
    let in_house =
        hotel.stay(Stay { room: Some("105"), meal_amount: 3_000, ..Stay::new(p, "checked_in", -1, 2) }).await;
    let fit = hotel
        .stay(Stay {
            room: Some("106"),
            currency: "USD",
            segment: "FIT_F",
            room_amount: 12_000,
            meal_amount: 1_500,
            ..Stay::new(p, "checked_in", 0, 3)
        })
        .await;
    let gone = hotel.stay(Stay { room: Some("107"), ..Stay::new(p, "checked_in", -1, 0) }).await;
    hotel.check_out(gone.id).await;

    let cash = hotel.pay(p, in_house.guest_folio, Method::Cash, 10_000, "LKR", None).await;
    hotel.refund(p, cash.id, 2_000).await;
    let card_method = Method::CardTerminal { approval: "A1".into(), last4: "4242".into(), brand: "Visa".into() };
    let card = hotel.pay(p, fit.guest_folio, card_method, 5_000, "USD", Some("passport")).await;
    let flagged = hotel.pay(p, fit.guest_folio, Method::Cash, 7_000, "LKR", None).await;

    hotel.outlet(p, "BAR", "Bar").await;
    let posted_bill = hotel.pos_bill(p, "BAR", "B1", 4_000, PosTarget::Stay(in_house.id)).await;
    let settled_bill = hotel.pos_bill(p, "BAR", "B2", 2_500, PosTarget::Settled { method: Method::Cash }).await;
    let settled_total = settled_bill.lines.iter().map(|line| line.total).sum();

    let misc = hotel.charge(p, departing.guest_folio, "misc", 5_000).await;
    let laundry = hotel.charge(p, in_house.guest_folio, "laundry", 1_000).await;

    Day {
        confirmed,
        tentative,
        overdue,
        departing,
        in_house,
        fit,
        posted_bill: posted_bill.id,
        settled_bill: settled_bill.id,
        settled_total,
        card_base: card.base_amount,
        flagged: flagged.id,
        misc_base: misc.base_total,
        departing_balance: misc.total,
        laundry_base: laundry.base_total,
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_list_of_the_seeded_day_is_exact(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let day = seed(&hotel).await;
    let d = hotel.business_date;

    let review = hotel.night_audit(hotel.property).await;

    assert_eq!(review.business_date, d);
    assert_eq!(review.blocking, 4, "three arrivals and one departure");
    let blocking = audit::blocking(&mut hotel.tx().await, hotel.property, d).await.unwrap();
    assert_eq!(blocking, 4);

    let arrival = |booked: &Booked, guest: &str, room: Option<&str>, from, to, status, total, penalty| ArrivalRow {
        reservation_room: booked.id,
        reservation_id: booked.reservation,
        confirmation: booked.confirmation.clone(),
        guest: guest.into(),
        room_number: room.map(Into::into),
        arrival: hotel.day(from),
        departure: hotel.day(to),
        status,
        total,
        currency: "LKR".into(),
        no_show_penalty: penalty,
        version: 1,
    };
    assert_eq!(
        review.arrivals,
        vec![
            arrival(&day.confirmed, "Kamal Perera", Some("101"), 0, 2, RoomStatus::Confirmed, 24_000, 10_000),
            arrival(&day.overdue, "Ada Silva", Some("103"), -1, 2, RoomStatus::Confirmed, 30_000, 0),
            arrival(&day.tentative, "Fernando", None, 0, 1, RoomStatus::Tentative, 10_000, 0),
        ],
        "by room number, an unassigned room last"
    );

    assert_eq!(
        review.departures,
        vec![DepartureRow {
            reservation_room: day.departing.id,
            reservation_id: day.departing.reservation,
            confirmation: day.departing.confirmation.clone(),
            guest: "Ada Silva".into(),
            room_number: Some("104".into()),
            departure: d,
            balances: vec![("LKR".into(), day.departing_balance)],
            version: 1,
        }]
    );

    let (posted_total, posted_base) = hotel.bill_lines(day.posted_bill).await;
    assert_eq!(
        review.pos_bills,
        vec![OutletBills {
            outlet: "Bar".into(),
            bills: vec![
                PosBillRow {
                    id: day.posted_bill,
                    bill_no: "B1".into(),
                    currency: "LKR".into(),
                    total: posted_total,
                    total_base: posted_base,
                    room_number: Some("105".into()),
                    settled: false,
                    highlight: false,
                },
                PosBillRow {
                    id: day.settled_bill,
                    bill_no: "B2".into(),
                    currency: "LKR".into(),
                    total: day.settled_total,
                    total_base: day.settled_total,
                    room_number: None,
                    settled: true,
                    highlight: false,
                },
            ],
            total_base: posted_base + day.settled_total,
        }]
    );
    assert!(posted_total > 4_000 && day.settled_total > 2_500, "both taxed");

    let night = |booked: &Booked, room: &str, room_amount, meal_amount, currency: &str| NightPreview {
        reservation_room: booked.id,
        room_number: Some(room.into()),
        date: d,
        room_amount,
        meal_amount,
        currency: currency.into(),
        problem: None,
        already_posted: 0,
    };
    let mut nights =
        vec![night(&day.in_house, "105", 10_000, 3_000, "LKR"), night(&day.fit, "106", 12_000, 1_500, "USD")];
    nights.sort_by_key(|night| night.reservation_room);
    assert_eq!(
        review.booking_bills,
        BookingBills {
            nights,
            posted: vec![
                CategoryTotal { category: "laundry".into(), lines: 1, total_base: day.laundry_base },
                CategoryTotal { category: "misc".into(), lines: 1, total_base: day.misc_base },
            ],
        }
    );

    assert_eq!(
        review.payments,
        vec![
            PaymentGroup {
                method: "card_terminal".into(),
                currency: "USD".into(),
                count: 1,
                amount: 5_000,
                base_amount: day.card_base,
                refunds: 0,
                flagged: vec![],
            },
            PaymentGroup {
                method: "cash".into(),
                currency: "LKR".into(),
                count: 3,
                amount: 10_000 - 2_000 + 7_000 + day.settled_total,
                base_amount: 10_000 - 2_000 + 7_000 + day.settled_total,
                refunds: -2_000,
                flagged: vec![day.flagged],
            },
        ]
    );
    assert_eq!(day.card_base, 1_500_000, "USD 50.00 at 300");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_charge_to_account_shows_as_the_account_method(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", 0, 2) }).await;
    let account = Uuid::now_v7();
    let mut tx = hotel.tx().await;
    sqlx::query("insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'LKR')")
        .bind(account)
        .bind(hotel.tenant.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("update reservation set account_id = $2 where id = $1")
        .bind(stay.reservation)
        .bind(account)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.charge(p, stay.guest_folio, "misc", 3_000).await;

    let charged = hotel.pay(p, stay.guest_folio, Method::Account, 3_000, "LKR", None).await;

    let payments = hotel.night_audit(p).await.payments;
    assert_eq!(
        payments,
        vec![PaymentGroup {
            method: "account".into(),
            currency: "LKR".into(),
            count: 1,
            amount: 3_000,
            base_amount: charged.base_amount,
            refunds: 0,
            flagged: vec![],
        }],
        "the city ledger"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_preview_is_what_posting_then_posts(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let day = seed(&hotel).await;
    let d = hotel.business_date;

    let preview = hotel.night_audit(hotel.property).await.booking_bills.nights;
    let report = folio::post_business_date_charges(&hotel.pool, hotel.tenant, None, hotel.property, d).await.unwrap();

    assert_eq!(report, PostReport { stays: 2, lines: 4, skipped_existing: 0, failures: vec![] });
    let mut expected: Vec<(String, i64)> = preview
        .iter()
        .flat_map(|night| {
            [
                (format!("meal_plan:{}:{d}", night.reservation_room), night.meal_amount),
                (format!("room:{}:{d}", night.reservation_room), night.room_amount),
            ]
        })
        .collect();
    let posted: Vec<(String, i64)> = hotel
        .keyed_lines(hotel.property)
        .await
        .into_iter()
        .filter(|(key, _)| key.starts_with("room:") || key.starts_with("meal_plan:"))
        .collect();
    expected.sort();
    assert_eq!(posted, expected);
    let mut stays = vec![day.in_house.id, day.fit.id];
    stays.sort();
    assert_eq!(preview.iter().map(|night| night.reservation_room).collect::<Vec<_>>(), stays);

    let again = hotel.night_audit(hotel.property).await.booking_bills.nights;
    assert!(again.iter().all(|night| night.already_posted == 2 && night.problem.is_none()), "{again:?}");
    let rerun = folio::post_business_date_charges(&hotel.pool, hotel.tenant, None, hotel.property, d).await.unwrap();
    assert_eq!(rerun, PostReport { stays: 2, lines: 0, skipped_existing: 4, failures: vec![] });
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_preview_flags_a_stay_posting_would_fail(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let d = hotel.business_date;
    let usd = hotel.stay(Stay { room: Some("101"), currency: "USD", ..Stay::new(p, "checked_in", 0, 2) }).await;
    let unassigned = hotel.stay(Stay::new(p, "checked_in", 0, 2)).await;
    let unpriced = hotel.stay(Stay { room: Some("103"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    let mut tx = hotel.tx().await;
    sqlx::query("delete from reservation_night where reservation_room_id = $1 and date = $2")
        .bind(unpriced.id)
        .bind(d)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let preview = hotel.night_audit(p).await.booking_bills.nights;

    let problems: Vec<(Uuid, Option<String>)> =
        preview.iter().map(|night| (night.reservation_room, night.problem.clone())).collect();
    let mut expected = vec![
        (usd.id, Some(format!("No exchange rate for USD on {d}"))),
        (unassigned.id, Some("No room assigned".into())),
        (unpriced.id, Some(format!("No price for {d}"))),
    ];
    expected.sort();
    assert_eq!(problems, expected);
    let report = folio::post_business_date_charges(&hotel.pool, hotel.tenant, None, p, d).await.unwrap();
    let mut failed: Vec<Uuid> = report.failures.iter().map(|(stay, _)| *stay).collect();
    failed.sort();
    assert_eq!(failed, vec![usd.id, unassigned.id, unpriced.id], "posting fails for the same stays");
    assert_eq!(report.lines, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bill_on_a_stay_checked_out_with_a_balance_is_highlighted(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    hotel.outlet(p, "BAR", "Bar").await;
    let owing = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 1) }).await;
    let settled = hotel.stay(Stay { room: Some("102"), ..Stay::new(p, "checked_in", -1, 1) }).await;
    let staying = hotel.stay(Stay { room: Some("103"), ..Stay::new(p, "checked_in", -1, 1) }).await;
    let owing_bill = hotel.pos_bill(p, "BAR", "B1", 4_000, PosTarget::Stay(owing.id)).await;
    let settled_bill = hotel.pos_bill(p, "BAR", "B2", 4_000, PosTarget::Stay(settled.id)).await;
    hotel.pos_bill(p, "BAR", "B3", 4_000, PosTarget::Stay(staying.id)).await;
    let (total, _) = hotel.bill_lines(settled_bill.id).await;
    hotel.pay(p, settled.guest_folio, Method::Cash, total, "LKR", None).await;
    hotel.check_out(owing.id).await;
    hotel.check_out(settled.id).await;

    let bills = &hotel.night_audit(p).await.pos_bills[0].bills;

    let highlighted: Vec<(String, bool)> = bills.iter().map(|bill| (bill.bill_no.clone(), bill.highlight)).collect();
    assert_eq!(highlighted, vec![("B1".into(), true), ("B2".into(), false), ("B3".into(), false)]);
    assert_eq!(bills[0].id, owing_bill.id);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_propertys_rows_never_appear(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let other = hotel.add_property("KAN").await;
    hotel.stay(Stay { room: Some("101"), ..Stay::new(other, "confirmed", 0, 2) }).await;
    let departing = hotel.stay(Stay { room: Some("102"), ..Stay::new(other, "checked_in", -1, 0) }).await;
    let in_house = hotel.stay(Stay { room: Some("103"), ..Stay::new(other, "checked_in", -1, 2) }).await;
    hotel.charge(other, departing.guest_folio, "misc", 5_000).await;
    hotel.pay(other, in_house.guest_folio, Method::Cash, 5_000, "LKR", None).await;
    hotel.outlet(other, "BAR", "Bar").await;
    hotel.pos_bill(other, "BAR", "B1", 4_000, PosTarget::Stay(in_house.id)).await;
    hotel.pos_bill(other, "BAR", "B2", 4_000, PosTarget::Settled { method: Method::Cash }).await;

    let review = hotel.night_audit(hotel.property).await;

    assert_eq!(review.blocking, 0);
    assert!(review.arrivals.is_empty() && review.departures.is_empty() && review.pos_bills.is_empty());
    assert_eq!(review.booking_bills, BookingBills { nights: vec![], posted: vec![] });
    assert!(review.payments.is_empty());
    let blocking = audit::blocking(&mut hotel.tx().await, hotel.property, hotel.business_date).await.unwrap();
    assert_eq!(blocking, 0);
    let theirs = hotel.night_audit(other).await;
    assert_eq!((theirs.blocking, theirs.pos_bills[0].bills.len(), theirs.payments.len()), (2, 2, 1));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_of_another_tenant_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let err = audit::night_audit(&mut hotel.tx().await, Uuid::now_v7()).await.unwrap_err();

    assert!(matches!(err, audit::AuditError::NotFound("property")), "{err:?}");
}

/// Arrivals and departures are found through `reservation_room_arrival_idx`, as the application role under row-level
/// security: their bound is on the stored, leakproof `arrival`, so it is an index condition. The property also holds
/// a season of future bookings (analyzed), which the bound leaves out, as a working hotel's would.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn arrivals_and_departures_use_the_arrival_index(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts).await;
    let day = seed(&hotel).await;
    sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
         select gen_random_uuid(), tenant_id, property_id, reservation_id, room_type_id,
                daterange(arrival + g, arrival + g + 2), 2, 0, rate_plan_id, meal_plan, 'confirmed',
                primary_guest_id, currency
         from reservation_room, generate_series(1, 2000) as g
         where id = $1",
    )
    .bind(day.tentative.id)
    .execute(&owner)
    .await
    .unwrap();
    sqlx::query("analyze reservation_room").execute(&owner).await.unwrap();

    for sql in [audit::ARRIVALS_SQL, audit::DEPARTURES_SQL] {
        let mut tx = hotel.tx().await;
        let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("explain {sql}")))
            .bind(hotel.property)
            .bind(hotel.business_date)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        let plan = plan.join("\n");
        assert!(plan.contains("reservation_room_arrival_idx"), "{plan}");
    }
}
