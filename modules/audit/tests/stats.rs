//! A business date's statistics (`audit::compute`), their snapshot (`audit::store`) and their reading by range
//! (`audit::daily_stats`), on stays seeded straight into the reservation tables and money posted through `folio`.

mod common;

use audit::{BreakdownRow, DailyStats};
use common::{Hotel, Stay};
use folio::{Method, PosBillIn, PosItem, PosOutcome, PosTarget};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rust_decimal::Decimal;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeMap;

// The golden day's figures, all in LKR cents (the base currency), net of tax. USD converts at 300.000000, so a USD
// cent is 300 LKR cents. The property is not VAT registered: its Sri Lanka preset charges SC 10% on POS items, TDL 1%
// of net, and SSCL 2.5% of net + SC + TDL, each rounded half-up in the line's currency.
//
// Room revenue, on D:
//   A (LKR)         night D               10 000
//   B (USD 120.00)  night D 12 000 × 300  3 600 000
//   C (OTA)         night D                8 000
//   C               reversal of D − 1     −8 000
//   E (comp)        night D                    0
//   E               manual "room" charge   3 000  (no key: unattributed)
//                                       = 3 613 000
const ROOM_REVENUE: i64 = 3_613_000;
// Meal-plan revenue: A 2 000, its transfer to the account folio (−2 000 + 2 000), B 1 500 × 300 = 450 000.
const MEAL_PLAN_REVENUE: i64 = 452_000;
// POS revenue: BAR's 4 000 posted to A, REST's 2 500 settled at the outlet.
const POS_REVENUE: i64 = 6_500;
// Fee revenue: G's no-show penalty, one night of 10 000 (its terms charge one night; prices exclude tax).
const FEE_REVENUE: i64 = 10_000;
// Other revenue: the laundry charge of 1 000 on C. The charge to account's line is an adjustment: not revenue.
const OTHER_REVENUE: i64 = 1_000;
// SC: BAR 400 + REST 250.
const SC: i64 = 650;
// TDL: A room 100 + A meal 20 (its transfer −20 + 20) + B room 120 × 300 + B meal 15 × 300 + C room 80 − 80 +
// manual room 30 + laundry 10 + BAR 40 + REST 25 + fee 100.
const TDL: i64 = 100 + 20 + 36_000 + 4_500 + 30 + 10 + 40 + 25 + 100;
// SSCL: A room 10 100 → 253, A meal 2 020 → 51 (transfer nets 0), B room 12 120 → 303 × 300, B meal 1 515 → 38 × 300,
// C room 8 080 → 202 − 202, manual room 3 030 → 76, laundry 1 010 → 25, BAR 4 440 → 111, REST 2 775 → 69,
// fee 10 100 → 253.
const SSCL: i64 = 253 + 51 + 90_900 + 11_400 + 76 + 25 + 111 + 69 + 253;
// Payments: cash 20 000 (more than A owes) − its refund 2 000 + REST's settled bill (2 500 + SC 250 + TDL 25 +
// SSCL 69 = 2 844); card USD 50.00 × 300; the charge to account 5 000.
const CASH: i64 = 20_000 - 2_000 + 2_844;
const CARD: i64 = 5_000 * 300;
const ACCOUNT: i64 = 5_000;

/// One property, D:
/// - rooms 101 to 110 active (110 out of order over D; 109 out of service; 108's out-of-order block released; 107's
///   ended on D) and 111 inactive;
/// - sold: A (LKR, `FIT_L`, a walk-in arriving D, with breakfast), B (USD, `FIT_F`, a non-resident in `DLX` booked by
///   email, arriving D with a child), C (`OTA` through the channel manager, since D − 2), E (complimentary, one adult,
///   since D − 1);
/// - F checked out, leaving D; G a no-show with a one-night penalty; H (D + 5) cancelled;
/// - D − 1's nights posted the day before, and C's reversed on D;
/// - a POS bill at BAR posted to A and one at REST settled in cash;
/// - A's meal line moved to its company's account folio, and a charge to that account;
/// - cash with a refund on A, USD by card on B, a manual room charge on E and laundry on C.
async fn seed_golden(hotel: &Hotel) {
    let p = hotel.property;
    hotel.rate(p, "USD", Decimal::new(300, 0)).await;
    hotel.room_type(p, "DLX").await;
    let a = hotel
        .stay(Stay {
            room: Some("101"),
            segment: "FIT_L",
            meal_amount: 2_000,
            booked: 0,
            ..Stay::new(p, "checked_in", 0, 2)
        })
        .await;
    let b = hotel
        .stay(Stay {
            room: Some("102"),
            currency: "USD",
            segment: "FIT_F",
            residency: "non_resident",
            source: "email",
            room_type: "DLX",
            room_amount: 12_000,
            meal_amount: 1_500,
            children: 1,
            ..Stay::new(p, "checked_in", 0, 3)
        })
        .await;
    let c = hotel
        .stay(Stay {
            room: Some("103"),
            segment: "OTA",
            source: "channel",
            room_amount: 8_000,
            ..Stay::new(p, "checked_in", -2, 1)
        })
        .await;
    let e = hotel
        .stay(Stay {
            room: Some("104"),
            segment: "FIT_L",
            room_amount: 0,
            adults: 1,
            ..Stay::new(p, "checked_in", -1, 1)
        })
        .await;
    let f = hotel.stay(Stay { room: Some("105"), ..Stay::new(p, "checked_in", -2, 0) }).await;
    let terms = serde_json::json!({ "rules": [], "no_show": { "kind": "nights", "value": 1 } });
    let g = hotel.stay(Stay { terms: Some(terms), ..Stay::new(p, "confirmed", 0, 2) }).await;
    let h = hotel.stay(Stay::new(p, "confirmed", 5, 7)).await;
    let mut rooms = Vec::new();
    for number in ["106", "107", "108", "109", "110"] {
        rooms.push(hotel.room(p, number, true).await);
    }
    hotel.room(p, "111", false).await;
    hotel.block(p, rooms[4], "out_of_order", -1, 2, false).await;
    hotel.block(p, rooms[3], "out_of_service", 0, 1, false).await;
    hotel.block(p, rooms[2], "out_of_order", -1, 1, true).await;
    hotel.block(p, rooms[1], "out_of_order", -3, 0, false).await;

    // D − 1: C, E and F were in house.
    hotel.move_business_date(p, -1).await;
    hotel.post_nights(p, -1).await;
    hotel.move_business_date(p, 0).await;
    hotel.check_out(f.id).await;

    hotel.post_nights(p, 0).await;
    let user = Some(hotel.user);
    let mut tx = hotel.tx().await;
    let (folio, line) = hotel.keyed_line(&format!("room:{}:{}", c.id, hotel.day(-1))).await;
    folio::reverse(&mut tx, hotel.tenant, user, p, folio, line, "Wrong night").await.unwrap();
    let account = hotel.bill_to_account(a.reservation).await;
    let account_folio = folio::ensure_account_folio(&mut tx, hotel.tenant, p, account).await.unwrap();
    let (folio, line) = hotel.keyed_line(&format!("meal_plan:{}:{}", a.id, hotel.day(0))).await;
    folio::transfer(&mut tx, hotel.tenant, user, p, folio, line, account_folio, "Company pays meals").await.unwrap();
    reservations::mark_no_show(&mut tx, hotel.tenant, user, p, g.id, 1).await.unwrap();
    reservations::cancel_room(&mut tx, hotel.tenant, user, p, h.id, 1).await.unwrap();
    tx.commit().await.unwrap();
    hotel.pay(p, a.guest_folio, Method::Account, 5_000, "LKR", None).await;

    hotel.outlet(p, "BAR", "Bar").await;
    hotel.outlet(p, "REST", "Restaurant").await;
    hotel.pos_bill(p, "BAR", "B1", 4_000, PosTarget::Stay(a.id)).await;
    hotel.pos_bill(p, "REST", "R1", 2_500, PosTarget::Settled { method: Method::Cash }).await;
    hotel.charge(p, e.guest_folio, "room", 3_000).await;
    hotel.charge(p, c.guest_folio, "laundry", 1_000).await;

    let cash = hotel.pay(p, a.guest_folio, Method::Cash, 20_000, "LKR", None).await;
    hotel.refund(p, cash.id, 2_000).await;
    let card = Method::CardTerminal { approval: "A1".into(), last4: "4242".into(), brand: "Visa".into() };
    hotel.pay(p, b.guest_folio, card, 5_000, "USD", Some("passport")).await;
}

fn row(dimension: &str, key: &str, room_nights: i32, room_revenue: i64, revenue: i64) -> BreakdownRow {
    BreakdownRow { dimension: dimension.into(), key: key.into(), room_nights, room_revenue, revenue }
}

/// Every invariant of the breakdown: per stay dimension, the rows add up to the rooms sold, the room revenue, and the
/// room plus meal-plan revenue; the outlets add up to the POS revenue.
fn assert_invariants(stats: &DailyStats) {
    for dimension in ["segment", "source", "residency", "room_type"] {
        let rows: Vec<&BreakdownRow> = stats.breakdown.iter().filter(|row| row.dimension == dimension).collect();
        assert_eq!(rows.iter().map(|row| row.room_nights).sum::<i32>(), stats.rooms_sold, "{dimension}");
        assert_eq!(rows.iter().map(|row| row.room_revenue).sum::<i64>(), stats.room_revenue, "{dimension}");
        assert_eq!(
            rows.iter().map(|row| row.revenue).sum::<i64>(),
            stats.room_revenue + stats.meal_plan_revenue,
            "{dimension}"
        );
    }
    let outlets: Vec<&BreakdownRow> = stats.breakdown.iter().filter(|row| row.dimension == "outlet").collect();
    assert_eq!(outlets.iter().map(|row| row.revenue).sum::<i64>(), stats.pos_revenue);
    assert!(outlets.iter().all(|row| row.room_nights == 0 && row.room_revenue == 0));
}

async fn compute(hotel: &Hotel) -> DailyStats {
    audit::compute(&mut hotel.tx().await, hotel.property, hotel.business_date).await.unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_figure_of_the_golden_day_is_exact(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed_golden(&hotel).await;

    let stats = compute(&hotel).await;

    // Per stay: (room revenue, room + meal-plan revenue). A: 10 000 and 2 000 of meals; B: 3 600 000 and 450 000;
    // C: 8 000 less the reversal of its D − 1 night, 8 000; E: 0. E's manual room charge of 3 000 has no stay.
    let (a, b) = ((10_000, 12_000), (3_600_000, 4_050_000));
    let breakdown = vec![
        row("outlet", "BAR", 0, 0, 4_000),
        row("outlet", "REST", 0, 0, 2_500),
        row("residency", "non_resident", 1, b.0, b.1),
        row("residency", "resident", 3, a.0, a.1),
        row("residency", "unattributed", 0, 3_000, 3_000),
        row("room_type", "DLX", 1, b.0, b.1),
        row("room_type", "STD", 3, a.0, a.1),
        row("room_type", "unattributed", 0, 3_000, 3_000),
        row("segment", "FIT_F", 1, b.0, b.1),
        row("segment", "FIT_L", 2, a.0, a.1),
        row("segment", "OTA", 1, 0, 0),
        row("segment", "unattributed", 0, 3_000, 3_000),
        row("source", "channel", 1, 0, 0),
        row("source", "email", 1, b.0, b.1),
        row("source", "front_desk", 2, a.0, a.1),
        row("source", "unattributed", 0, 3_000, 3_000),
    ];
    let expected = DailyStats {
        business_date: hotel.business_date,
        rooms_total: 10,
        rooms_out_of_order: 1,
        rooms_available: 9,
        rooms_sold: 4,
        complimentary_rooms: 1,
        // 4 × 10 000 / 9 = 4 444.4
        occupancy_bp: 4_444,
        adults: 7,
        children: 1,
        arrivals: 2,
        departures: 1,
        no_shows: 1,
        cancellations: 1,
        walk_ins: 1,
        room_revenue: ROOM_REVENUE,
        meal_plan_revenue: MEAL_PLAN_REVENUE,
        pos_revenue: POS_REVENUE,
        fee_revenue: FEE_REVENUE,
        other_revenue: OTHER_REVENUE,
        // 3 613 000 / 4
        adr: 903_250,
        // 3 613 000 / 9 = 401 444.4
        revpar: 401_444,
        taxes: BTreeMap::from([("SC".into(), SC), ("SSCL".into(), SSCL), ("TDL".into(), TDL)]),
        payments: BTreeMap::from([("account".into(), ACCOUNT), ("card_terminal".into(), CARD), ("cash".into(), CASH)]),
        breakdown,
    };
    assert_eq!(stats, expected);
    assert_invariants(&stats);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_breakdown_adds_up_on_a_random_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let mut rng = StdRng::seed_from_u64(7);
    hotel.rate(p, "USD", Decimal::new(300, 0)).await;
    hotel.room_type(p, "DLX").await;
    hotel.outlet(p, "BAR", "Bar").await;
    hotel.outlet(p, "POOL", "Pool").await;
    let segments = ["FIT_F", "FIT_L", "OTA", "TA", "IBE"];
    let sources = ["front_desk", "ibe", "channel", "phone", "email"];
    let mut in_house = Vec::new();
    for i in 0..20 {
        let number = (101 + i).to_string();
        let leaving = i < 4;
        let stay = Stay {
            room: Some(&number),
            currency: if rng.random_bool(0.3) { "USD" } else { "LKR" },
            segment: segments[rng.random_range(0..segments.len())],
            residency: if rng.random_bool(0.5) { "resident" } else { "non_resident" },
            source: sources[rng.random_range(0..sources.len())],
            room_type: if rng.random_bool(0.5) { "STD" } else { "DLX" },
            room_amount: if rng.random_bool(0.1) { 0 } else { rng.random_range(5_000..20_000) },
            meal_amount: if rng.random_bool(0.5) { 0 } else { rng.random_range(1_000..3_000) },
            booked: rng.random_range(-10..=0),
            adults: rng.random_range(1..=2),
            children: rng.random_range(0..=1),
            ..Stay::new(p, "checked_in", if leaving { -2 } else { rng.random_range(-3..=0) }, rng.random_range(1..=3))
        };
        let stay = Stay { to: if leaving { 0 } else { stay.to }, ..stay };
        let booked = hotel.stay(stay).await;
        if leaving {
            hotel.check_out(booked.id).await;
        } else {
            in_house.push(booked);
        }
    }
    hotel.post_nights(p, 0).await;
    let d = hotel.business_date;
    let user = Some(hotel.user);
    let (mut reversed, mut moved) = (0, 0);
    for (i, stay) in in_house.iter().enumerate() {
        if rng.random_bool(0.25) {
            let (folio, line) = hotel.keyed_line(&format!("room:{}:{d}", stay.id)).await;
            let mut tx = hotel.tx().await;
            folio::reverse(&mut tx, hotel.tenant, user, p, folio, line, "Rate correction").await.unwrap();
            tx.commit().await.unwrap();
            reversed += 1;
        }
        let meal = hotel.keyed_lines(p).await.into_iter().any(|(key, _)| key == format!("meal_plan:{}:{d}", stay.id));
        if meal && rng.random_bool(0.4) {
            let (folio, line) = hotel.keyed_line(&format!("meal_plan:{}:{d}", stay.id)).await;
            let other = &in_house[(i + 1) % in_house.len()];
            let mut tx = hotel.tx().await;
            folio::transfer(&mut tx, hotel.tenant, user, p, folio, line, other.guest_folio, "Shared table")
                .await
                .unwrap();
            tx.commit().await.unwrap();
            moved += 1;
        }
        if rng.random_bool(0.3) {
            let outlet = if rng.random_bool(0.5) { "BAR" } else { "POOL" };
            hotel.pos_bill(p, outlet, &format!("S{i}"), rng.random_range(500..5_000), PosTarget::Stay(stay.id)).await;
        }
    }
    for i in 0..3 {
        let settled = PosTarget::Settled { method: Method::Cash };
        hotel.pos_bill(p, "POOL", &format!("T{i}"), rng.random_range(500..5_000), settled).await;
    }
    hotel.charge(p, in_house[0].guest_folio, "room", 2_500).await;

    let stats = compute(&hotel).await;

    assert!(reversed > 0 && moved > 0, "the seed reverses and moves lines");
    assert_eq!(stats.rooms_sold, 16);
    assert!(stats.room_revenue > 0 && stats.pos_revenue > 0);
    assert_invariants(&stats);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stored_statistics_read_back_as_computed(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    seed_golden(&hotel).await;
    let (p, d) = (hotel.property, hotel.business_date);
    let mut tx = hotel.tx().await;
    let stats = audit::compute(&mut tx, p, d).await.unwrap();
    audit::store(&mut tx, hotel.tenant, p, &stats).await.unwrap();
    tx.commit().await.unwrap();

    let read = audit::daily_stats(&mut hotel.tx().await, p, hotel.day(-1), hotel.day(1)).await.unwrap();
    assert_eq!(read, vec![stats.clone()]);

    let mut tx = hotel.tx().await;
    let again = audit::store(&mut tx, hotel.tenant, p, &stats).await.unwrap_err();
    let unique = match &again {
        audit::AuditError::Database(err) => err.as_database_error().is_some_and(|err| err.is_unique_violation()),
        _ => false,
    };
    assert!(unique, "a second snapshot of the day is refused: {again:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_range_is_at_most_366_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (p, d) = (hotel.property, hotel.business_date);
    let mut tx = hotel.tx().await;

    assert_eq!(audit::daily_stats(&mut tx, p, d, hotel.day(365)).await.unwrap(), vec![]);
    let too_long = audit::daily_stats(&mut tx, p, d, hotel.day(366)).await.unwrap_err();
    assert!(matches!(too_long, audit::AuditError::Invalid(_)), "{too_long:?}");
    let backwards = audit::daily_stats(&mut tx, p, d, hotel.day(-1)).await.unwrap_err();
    assert!(matches!(backwards, audit::AuditError::Invalid(_)), "{backwards:?}");
    let other = audit::daily_stats(&mut tx, uuid::Uuid::now_v7(), d, d).await.unwrap_err();
    assert!(matches!(other, audit::AuditError::NotFound("property")), "{other:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_day_with_nothing_is_all_zeros(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let stats = compute(&hotel).await;

    assert_eq!(
        stats,
        DailyStats {
            business_date: hotel.business_date,
            rooms_total: 0,
            rooms_out_of_order: 0,
            rooms_available: 0,
            rooms_sold: 0,
            complimentary_rooms: 0,
            occupancy_bp: 0,
            adults: 0,
            children: 0,
            arrivals: 0,
            departures: 0,
            no_shows: 0,
            cancellations: 0,
            walk_ins: 0,
            room_revenue: 0,
            meal_plan_revenue: 0,
            pos_revenue: 0,
            fee_revenue: 0,
            other_revenue: 0,
            adr: 0,
            revpar: 0,
            taxes: BTreeMap::new(),
            payments: BTreeMap::new(),
            breakdown: vec![],
        }
    );
}

/// D's lines are found through `folio_line_business_date_idx`, as the application role under row-level security,
/// among a year of the tenant's earlier lines (analyzed).
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_days_lines_are_read_through_the_business_date_index(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", 0, 1) }).await;
    sqlx::query(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency,
                                 base_total, rate, rate_date, business_date)
         select gen_random_uuid(), $1, $2, 'manual', 'misc', 'Old', 100, 100, 'LKR', 100, 1, d, d
         from generate_series(1, 5000) as g, lateral (select $3::date - 1 - g % 365 as d) as day",
    )
    .bind(hotel.tenant.0)
    .bind(stay.guest_folio)
    .bind(hotel.business_date)
    .execute(&owner)
    .await
    .unwrap();
    sqlx::query("analyze folio_line").execute(&owner).await.unwrap();

    let mut tx = hotel.tx().await;
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("explain {}", audit::DAY_LINES_SQL)))
        .bind(p)
        .bind(hotel.business_date)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("folio_line_business_date_idx"), "{plan}");
}

/// A line moved to another stay's folio the day before and reversed on D still counts against the stay whose key the
/// moved line's original carries: through the reversal to the moved line, through its transfer's reversal to the
/// original, each read from D − 1.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reversal_follows_an_earlier_transfer_back_to_its_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let p = hotel.property;
    let x = hotel
        .stay(Stay { room: Some("101"), segment: "OTA", meal_amount: 2_000, ..Stay::new(p, "checked_in", -1, 1) })
        .await;
    let y = hotel.stay(Stay { room: Some("102"), segment: "TA", ..Stay::new(p, "checked_in", -1, 1) }).await;
    hotel.move_business_date(p, -1).await;
    hotel.post_nights(p, -1).await;
    let (folio, line) = hotel.keyed_line(&format!("meal_plan:{}:{}", x.id, hotel.day(-1))).await;
    let mut tx = hotel.tx().await;
    let (_, moved) =
        folio::transfer(&mut tx, hotel.tenant, None, p, folio, line, y.guest_folio, "Shared table").await.unwrap();
    tx.commit().await.unwrap();
    hotel.move_business_date(p, 0).await;
    let mut tx = hotel.tx().await;
    folio::reverse(&mut tx, hotel.tenant, None, p, y.guest_folio, moved.id, "Not taken").await.unwrap();
    tx.commit().await.unwrap();

    let stats = compute(&hotel).await;

    assert_eq!((stats.room_revenue, stats.meal_plan_revenue), (0, -2_000));
    let segments: Vec<BreakdownRow> =
        stats.breakdown.iter().filter(|row| row.dimension == "segment").cloned().collect();
    assert_eq!(segments, vec![row("segment", "OTA", 1, 0, -2_000), row("segment", "TA", 1, 0, 0)]);
    assert_invariants(&stats);
}

/// The two items of a bill at `outlet`, one `misc`, dated `bill_date` (the business date when `None`).
fn bill(
    outlet: &str,
    bill_no: &str,
    bill_date: Option<time::Date>,
    misc: i64,
    own: i64,
    target: PosTarget,
) -> PosBillIn {
    PosBillIn {
        outlet: outlet.into(),
        bill_no: bill_no.into(),
        business_date: bill_date,
        currency: "LKR".into(),
        items: vec![
            PosItem { description: "Towel".into(), quantity: 1, unit_net: misc, category: Some("misc".into()) },
            PosItem { description: "Drinks".into(), quantity: 1, unit_net: own, category: None },
        ],
        target,
    }
}

async fn post_bill(hotel: &Hotel, bill: PosBillIn) -> PosOutcome {
    let mut tx = hotel.tx().await;
    let outcome = folio::post_pos_bill(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, bill).await.unwrap();
    tx.commit().await.unwrap();
    outcome
}

/// A bill dated a closed day (owner ruling R14) keeps its own date, but its lines and payment are today's: today's
/// review lists it with its own date, today's figures count it, and the closed day's never change. A replay returns
/// the first copy; a changed body is refused.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bill_dated_a_closed_day_counts_today(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (p, d, yesterday) = (hotel.property, hotel.business_date, hotel.day(-1));
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 2) }).await;
    hotel.outlet(p, "BAR", "Bar").await;
    let closed = compute(&hotel).await;
    let closed_day = audit::compute(&mut hotel.tx().await, p, yesterday).await.unwrap();

    let settled = bill("BAR", "L1", Some(yesterday), 0, 2_500, PosTarget::Settled { method: Method::Cash });
    let posted = bill("BAR", "L2", Some(yesterday), 0, 4_000, PosTarget::Stay(stay.id));
    let PosOutcome::Posted(settled_row) = post_bill(&hotel, settled.clone()).await else { panic!("a new bill") };
    let PosOutcome::Posted(posted_row) = post_bill(&hotel, posted).await else { panic!("a new bill") };

    assert_eq!((settled_row.business_date, posted_row.business_date), (yesterday, yesterday), "the bill's own date");
    let line_dates: Vec<time::Date> =
        sqlx::query_scalar("select business_date from folio_line where starts_with(posting_key, $1)")
            .bind(format!("pos:{}:", posted_row.id))
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(line_dates, vec![d, d]);
    let paid_on: time::Date = sqlx::query_scalar("select business_date from payment where pos_bill_id = $1")
        .bind(settled_row.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(paid_on, d);

    let review = hotel.night_audit(p).await;
    let listed: Vec<(String, time::Date)> =
        review.pos_bills[0].bills.iter().map(|bill| (bill.bill_no.clone(), bill.bill_date)).collect();
    assert_eq!(listed, vec![("L1".into(), yesterday), ("L2".into(), yesterday)]);
    assert!(review.pos_bills[0].bills.iter().all(|bill| bill.total_base > 0));

    let today = compute(&hotel).await;
    assert_eq!(today.pos_revenue - closed.pos_revenue, 2_500 + 4_000);
    assert_eq!(today.payments.get("cash").copied().unwrap_or(0) - closed.payments.get("cash").copied().unwrap_or(0), {
        settled_row.lines.iter().map(|line| line.total).sum::<i64>()
    });
    assert_eq!(audit::compute(&mut hotel.tx().await, p, yesterday).await.unwrap(), closed_day);
    assert_invariants(&today);

    let replayed = post_bill(&hotel, settled.clone()).await;
    assert_eq!(replayed, PosOutcome::Replayed(settled_row));
    let mut changed = settled;
    changed.items[1].unit_net = 2_600;
    let mut tx = hotel.tx().await;
    let refused = folio::post_pos_bill(&mut tx, hotel.tenant, Some(hotel.user), p, changed).await.unwrap_err();
    assert!(matches!(refused, folio::FolioError::Conflict(_)), "{refused:?}");
}

/// A night the audit missed, posted at check-out after its day closed, is dated today and counts in today's figures.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_missed_night_posted_at_check_out_counts_today(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (p, d) = (hotel.property, hotel.business_date);
    let stay = hotel.stay(Stay { room: Some("101"), ..Stay::new(p, "checked_in", -1, 1) }).await;

    let mut tx = hotel.tx().await;
    let posted = folio::post_unposted_nights(&mut tx, hotel.tenant, None, stay.id, hotel.day(1)).await.unwrap();
    tx.commit().await.unwrap();
    hotel.check_out(stay.id).await;

    assert_eq!(posted, 2, "D − 1's missed night and D's");
    let dates: Vec<(String, time::Date)> =
        sqlx::query_as("select posting_key, business_date from folio_line order by posting_key")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        dates,
        vec![(format!("room:{}:{}", stay.id, hotel.day(-1)), d), (format!("room:{}:{d}", stay.id), d)],
        "the key keeps the night's date"
    );
    let today = compute(&hotel).await;
    assert_eq!(today.room_revenue, 20_000);
    let closed_day = audit::compute(&mut hotel.tx().await, p, hotel.day(-1)).await.unwrap();
    assert_eq!(closed_day.room_revenue, 0);
}
