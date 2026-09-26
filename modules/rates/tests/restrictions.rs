mod common;

use common::Hotel;
use rates::{ChangeMode, RatePlanChanges, RatesError, Restriction, RestrictionChange};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn invalid(result: Result<impl std::fmt::Debug, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// `(day offset, room type code, closed, min stay, closed to arrival)` of each row, for compact assertions.
fn summary(hotel: &Hotel, rows: &[Restriction]) -> Vec<(i64, &'static str, bool, Option<i32>, bool)> {
    rows.iter()
        .map(|r| {
            let code = if r.room_type_id == hotel.deluxe.id { "DLX" } else { "STD" };
            ((r.date - hotel.day(0)).whole_days(), code, r.closed, r.min_stay, r.closed_to_arrival)
        })
        .collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_are_set_per_day_and_fields_left_out_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);

    let closed = hotel.try_restrict(&bar, RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 0, 2) });
    closed.await.unwrap();
    let min_stay = RestrictionChange { min_stay: Some(Some(2)), ..hotel.restrict(&[deluxe, standard], 1, 3) };
    hotel.try_restrict(&bar, min_stay).await.unwrap();
    let arrival = RestrictionChange {
        closed_to_arrival: Some(true),
        weekdays: vec![hotel.day(1).weekday()],
        ..hotel.restrict(&[standard], 0, 14)
    };
    let arrivals_closed = hotel.try_restrict(&bar, arrival).await.unwrap();
    let before_clearing = hotel.restrictions(&bar).await;
    let cleared = RestrictionChange { min_stay: Some(None), ..hotel.restrict(&[deluxe, standard], 0, 14) };
    hotel.try_restrict(&bar, cleared).await.unwrap();

    assert_eq!(arrivals_closed, 2, "two of the fourteen days fall on that weekday");
    let week = |day: i64| (day, "STD", false, None, true);
    assert_eq!(
        summary(&hotel, &before_clearing),
        [
            (0, "DLX", true, None, false),
            (1, "DLX", true, Some(2), false),
            (1, "STD", false, Some(2), true),
            (2, "DLX", false, Some(2), false),
            (2, "STD", false, Some(2), false),
            week(8),
        ]
    );
    assert!(hotel.restrictions(&bar).await.iter().all(|r| r.min_stay.is_none()));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inheriting_plans_hold_a_copy_of_their_parents_restrictions(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let inheriting = |code: &str, parent: &rates::RatePlan| rates::NewRatePlan {
        inherit_restrictions: true,
        ..hotel.derived_plan(code, parent, ChangeMode::Percent, 1_000)
    };
    let ota = hotel.plan(inheriting("OTA", &bar)).await;
    let ota_nr = hotel.plan(inheriting("OTA-NR", &ota)).await;
    let own = hotel.plan(hotel.derived_plan("OWN", &bar, ChangeMode::Percent, 1_000)).await;
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);

    let parent = RestrictionChange { closed: Some(true), min_stay: Some(Some(3)), ..hotel.restrict(&[deluxe], 0, 3) };
    hotel.try_restrict(&bar, parent).await.unwrap();
    let on_ota = hotel.try_restrict(&ota, RestrictionChange { closed: Some(false), ..hotel.restrict(&[deluxe], 0, 1) });
    let on_ota = on_ota.await;
    let on_own = RestrictionChange { closed_to_arrival: Some(true), ..hotel.restrict(&[standard], 5, 6) };
    hotel.try_restrict(&own, on_own).await.unwrap();

    let bar_rows = hotel.restrictions(&bar).await;
    assert_eq!(bar_rows.len(), 3);
    assert_eq!(hotel.restrictions(&ota).await, bar_rows, "a copy, rewritten with the parent's");
    assert_eq!(hotel.restrictions(&ota_nr).await, bar_rows, "inherited down the chain");
    assert_eq!(summary(&hotel, &hotel.restrictions(&own).await), [(5, "STD", false, None, true)]);
    assert_eq!(invalid(on_ota), "OTA inherits BAR's restrictions; change them there, or stop inheriting");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inheriting_replaces_own_restrictions_and_stopping_keeps_the_copy(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let own = hotel.plan(hotel.derived_plan("OWN", &bar, ChangeMode::Percent, 1_000)).await;
    let deluxe = hotel.deluxe.id;
    hotel
        .try_restrict(&bar, RestrictionChange { min_stay: Some(Some(3)), ..hotel.restrict(&[deluxe], 1, 2) })
        .await
        .unwrap();
    hotel
        .try_restrict(&own, RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 5, 6) })
        .await
        .unwrap();

    let inherit = |on: bool| RatePlanChanges { inherit_restrictions: Some(on), ..RatePlanChanges::default() };
    let own = hotel.try_update(&own, inherit(true)).await.unwrap();
    let while_inheriting = hotel.restrictions(&own).await;
    let own = hotel.try_update(&own, inherit(false)).await.unwrap();
    let after_stopping = hotel.restrictions(&own).await;
    let own_again = RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 1, 2) };
    hotel.try_restrict(&own, own_again).await.unwrap();

    assert_eq!(summary(&hotel, &while_inheriting), [(1, "DLX", false, Some(3), false)]);
    assert_eq!(after_stopping, while_inheriting);
    assert_eq!(summary(&hotel, &hotel.restrictions(&own).await), [(1, "DLX", true, Some(3), false)]);
    assert_eq!(summary(&hotel, &hotel.restrictions(&bar).await), [(1, "DLX", false, Some(3), false)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_are_checked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar =
        hotel.plan(rates::NewRatePlan { room_type_ids: vec![hotel.deluxe.id], ..hotel.standard_plan("BAR", "USD") });
    let bar = bar.await;
    let deluxe = hotel.deluxe.id;
    hotel
        .try_restrict(&bar, RestrictionChange { max_stay: Some(Some(5)), ..hotel.restrict(&[deluxe], 0, 7) })
        .await
        .unwrap();

    let longer_than_max = RestrictionChange { min_stay: Some(Some(7)), ..hotel.restrict(&[deluxe], 0, 7) };
    let zero_nights = RestrictionChange { min_stay: Some(Some(0)), ..hotel.restrict(&[deluxe], 0, 7) };
    let unsold = RestrictionChange { closed: Some(true), ..hotel.restrict(&[hotel.standard.id], 0, 7) };
    let past = RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], -1, 7) };
    let nothing = hotel.restrict(&[deluxe], 0, 7);

    assert_eq!(
        invalid(hotel.try_restrict(&bar, longer_than_max).await),
        "a minimum stay cannot exceed the maximum stay"
    );
    assert_eq!(invalid(hotel.try_restrict(&bar, zero_nights).await), "minimum and maximum stays are 1 to 365 nights");
    assert_eq!(invalid(hotel.try_restrict(&bar, unsold).await), "BAR does not sell this room type");
    assert_eq!(
        invalid(hotel.try_restrict(&bar, past).await),
        "prices and restrictions are set from the business date for 730 days"
    );
    assert_eq!(invalid(hotel.try_restrict(&bar, nothing).await), "set at least one restriction");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_inheriting_plan_moved_to_another_parent_copies_the_new_parents_restrictions(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let p1 = hotel.plan(hotel.standard_plan("P1", "USD")).await;
    let p2 = hotel.plan(hotel.standard_plan("P2", "USD")).await;
    let deluxe = hotel.deluxe.id;

    // P1 closed on day 3
    hotel.try_restrict(&p1, RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 3, 4) }).await.unwrap();

    // P2 has min_stay 2 on day 5
    hotel
        .try_restrict(&p2, RestrictionChange { min_stay: Some(Some(2)), ..hotel.restrict(&[deluxe], 5, 6) })
        .await
        .unwrap();

    // D derives from P1 with inheritance
    let inheriting = |code: &str, parent: &rates::RatePlan| rates::NewRatePlan {
        inherit_restrictions: true,
        ..hotel.derived_plan(code, parent, ChangeMode::Percent, 1_000)
    };
    let d = hotel.plan(inheriting("D", &p1)).await;

    // G (grandchild) inherits from D
    let g = hotel.plan(inheriting("G", &d)).await;

    // Before moving, D and G have P1's restrictions (closure on day 3)
    let d_before = hotel.restrictions(&d).await;
    let g_before = hotel.restrictions(&g).await;
    assert_eq!(summary(&hotel, &d_before), [(3, "DLX", true, None, false)]);
    assert_eq!(summary(&hotel, &g_before), [(3, "DLX", true, None, false)]);

    // Move D to P2
    let move_changes = RatePlanChanges { parent_id: Some(p2.id), ..RatePlanChanges::default() };
    let d = hotel.try_update(&d, move_changes).await.unwrap();

    // After moving, D and G should have P2's restrictions (min_stay 2 on day 5), not P1's
    let d_after = hotel.restrictions(&d).await;
    let g_after = hotel.restrictions(&g).await;
    assert_eq!(summary(&hotel, &d_after), [(5, "DLX", false, Some(2), false)]);
    assert_eq!(summary(&hotel, &g_after), [(5, "DLX", false, Some(2), false)]);
    // P1's closure on day 3 should be gone
    assert!(!d_after.iter().any(|r| r.date == hotel.day(3)));
    assert!(!g_after.iter().any(|r| r.date == hotel.day(3)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn adding_a_room_type_to_an_inheriting_plan_copies_the_parents_restrictions_for_it(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let deluxe = hotel.deluxe.id;
    let standard = hotel.standard.id;

    // Parent sells both STD and DLX
    let parent = hotel.plan(hotel.standard_plan("PARENT", "USD")).await;

    // Parent has restrictions on DLX only
    hotel
        .try_restrict(&parent, RestrictionChange { min_stay: Some(Some(3)), ..hotel.restrict(&[deluxe], 0, 7) })
        .await
        .unwrap();

    // Derived plan inherits and sells only STD initially
    let derived = hotel
        .plan(rates::NewRatePlan {
            room_type_ids: vec![standard],
            inherit_restrictions: true,
            ..hotel.derived_plan("DERIVED", &parent, ChangeMode::Percent, 1_000)
        })
        .await;

    // DERIVED initially has no restrictions (only sells STD, parent's restrictions are on DLX)
    assert!(hotel.restrictions(&derived).await.is_empty());

    // Add DLX to DERIVED
    let add_types_changes =
        RatePlanChanges { room_type_ids: Some(vec![standard, deluxe]), ..RatePlanChanges::default() };
    let derived = hotel.try_update(&derived, add_types_changes).await.unwrap();

    // DERIVED should now have the parent's DLX restrictions
    let derived_restrictions = hotel.restrictions(&derived).await;
    assert_eq!(
        summary(&hotel, &derived_restrictions),
        [
            (0, "DLX", false, Some(3), false),
            (1, "DLX", false, Some(3), false),
            (2, "DLX", false, Some(3), false),
            (3, "DLX", false, Some(3), false),
            (4, "DLX", false, Some(3), false),
            (5, "DLX", false, Some(3), false),
            (6, "DLX", false, Some(3), false),
        ]
    );
}
