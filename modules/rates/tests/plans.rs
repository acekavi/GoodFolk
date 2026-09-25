mod common;

use common::Hotel;
use rates::{ChangeMode, PlanKind, RatePlanChanges, RatesError, Residency, Segment};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn invalid<T: std::fmt::Debug>(result: Result<T, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_are_listed_as_a_tree_with_their_room_types(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -1_000)).await;
    let custom = rates::NewRatePlan {
        kind: PlanKind::Custom,
        segment: Segment::Ta,
        room_type_ids: vec![hotel.deluxe.id],
        ..hotel.standard_plan("TA", "USD")
    };
    hotel.plan(custom).await;
    let local = rates::NewRatePlan { segment: Segment::FitL, ..hotel.standard_plan("FITL", "LKR") };
    let local = hotel.plan(local).await;

    let listed = hotel.plans().await;

    let tree: Vec<(&str, i32)> = listed.iter().map(|plan| (plan.code.as_str(), plan.depth)).collect();
    assert_eq!(tree, [("BAR", 0), ("OTA", 1), ("OTA-NR", 2), ("FITL", 0), ("TA", 0)]);
    assert_eq!(ota_nr.parent_id, Some(ota.id));
    assert_eq!(ota_nr.derive_mode, Some(ChangeMode::Amount));
    assert_eq!(ota_nr.derive_value, Some(-1_000));
    assert_eq!(listed[4].room_type_ids, [hotel.deluxe.id]);
    assert_eq!(bar.room_type_ids, [hotel.deluxe.id, hotel.standard.id]);
    assert_eq!(local.residency, Some(Residency::Resident), "FIT-L is sold to residents");
    assert_eq!((bar.version, bar.active), (1, true));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derivation_is_at_most_three_levels_deep(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let one = hotel.plan(hotel.derived_plan("L1", &bar, ChangeMode::Percent, 1_000)).await;
    let two = hotel.plan(hotel.derived_plan("L2", &one, ChangeMode::Percent, 1_000)).await;
    let three = hotel.plan(hotel.derived_plan("L3", &two, ChangeMode::Percent, 1_000)).await;

    let four = hotel.try_plan(hotel.derived_plan("L4", &three, ChangeMode::Percent, 1_000)).await;

    assert_eq!(three.depth, 3);
    assert_eq!(invalid(four), "derived plans are at most 3 levels below a standard plan");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_uses_its_parents_currency(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let local = hotel.plan(hotel.standard_plan("LOCAL", "LKR")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;

    let in_rupees = hotel.try_plan(rates::NewRatePlan {
        currency: "LKR".into(),
        ..hotel.derived_plan("OTA-LKR", &bar, ChangeMode::Percent, 1_500)
    });
    let moved_to_rupees =
        hotel.try_update(&ota, RatePlanChanges { parent_id: Some(local.id), ..RatePlanChanges::default() });

    let expected =
        "a derived plan uses its parent's currency (USD); resident prices in another currency are set by hand";
    assert_eq!(invalid(in_rupees.await), expected);
    assert!(invalid(moved_to_rupees.await).starts_with("a derived plan uses its parent's currency (LKR)"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn re_parenting_cannot_make_a_cycle_or_go_too_deep(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rack = hotel.plan(hotel.standard_plan("RACK", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Percent, -500)).await;
    let ibe = hotel.plan(hotel.derived_plan("IBE", &rack, ChangeMode::Percent, -500)).await;
    let ibe_2 = hotel.plan(hotel.derived_plan("IBE2", &ibe, ChangeMode::Percent, -500)).await;
    let reparent = |to: Uuid| RatePlanChanges { parent_id: Some(to), ..RatePlanChanges::default() };

    let under_its_child = hotel.try_update(&ota, reparent(ota_nr.id)).await;
    let under_itself = hotel.try_update(&ota, reparent(ota.id)).await;
    let too_deep = hotel.try_update(&ota, reparent(ibe_2.id)).await;
    let moved = hotel.try_update(&ota, reparent(ibe.id)).await.unwrap();

    assert_eq!(invalid(under_its_child), "OTA cannot derive from OTA-NR: that would make a cycle");
    assert_eq!(invalid(under_itself), "OTA cannot derive from OTA: that would make a cycle");
    assert_eq!(invalid(too_deep), "derived plans are at most 3 levels below a standard plan");
    assert_eq!((moved.parent_id, moved.depth, moved.version), (Some(ibe.id), 2, 2));
    let depths: Vec<(String, i32)> = hotel.plans().await.into_iter().map(|p| (p.code, p.depth)).collect();
    assert!(depths.contains(&("OTA-NR".into(), 3)), "{depths:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn custom_plans_stand_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let custom = rates::NewRatePlan { kind: PlanKind::Custom, ..hotel.standard_plan("CORP", "USD") };
    let custom = hotel.plan(custom).await;

    let from_custom = hotel.try_plan(hotel.derived_plan("CORP-10", &custom, ChangeMode::Percent, -1_000)).await;
    let custom_with_parent = hotel
        .try_plan(rates::NewRatePlan {
            kind: PlanKind::Custom,
            ..hotel.derived_plan("CORP-5", &custom, ChangeMode::Percent, -500)
        })
        .await;
    let inheriting_standard =
        hotel.try_plan(rates::NewRatePlan { inherit_restrictions: true, ..hotel.standard_plan("BAR", "USD") }).await;

    assert_eq!(invalid(from_custom), "CORP is a custom plan; only standard and derived plans have derived plans");
    assert_eq!(invalid(custom_with_parent), "only derived plans have a parent and a formula");
    assert_eq!(invalid(inheriting_standard), "only derived plans inherit restrictions");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fit_segments_set_and_check_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let foreign = rates::NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") };
    let foreign = hotel.plan(foreign).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "LKR")).await;

    let residents_on_fit_f = hotel
        .try_plan(rates::NewRatePlan {
            segment: Segment::FitF,
            residency: Some(Residency::Resident),
            ..hotel.standard_plan("FITF2", "USD")
        })
        .await;
    let now_local =
        hotel.try_update(&bar, RatePlanChanges { segment: Some(Segment::FitL), ..RatePlanChanges::default() }).await;

    assert_eq!(foreign.residency, Some(Residency::NonResident));
    assert_eq!(invalid(residents_on_fit_f), "FIT_F plans are sold to non-residents only");
    assert_eq!(now_local.unwrap().residency, Some(Residency::Resident));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_sells_only_room_types_its_parent_sells(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(rates::NewRatePlan {
        room_type_ids: vec![hotel.deluxe.id],
        ..hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)
    });
    let ota = ota.await;
    let deluxe_only = RatePlanChanges { room_type_ids: Some(vec![hotel.deluxe.id]), ..RatePlanChanges::default() };
    let standard_only = RatePlanChanges { room_type_ids: Some(vec![hotel.standard.id]), ..RatePlanChanges::default() };

    let bar = hotel.try_update(&bar, deluxe_only).await.unwrap();
    let dropping_a_type_the_child_sells = hotel.try_update(&bar, standard_only.clone()).await;
    let child_adds_an_unsold_type = hotel.try_update(&ota, standard_only).await;
    let unknown_type = hotel
        .try_plan(rates::NewRatePlan { room_type_ids: vec![Uuid::now_v7()], ..hotel.standard_plan("X", "USD") })
        .await;

    assert_eq!(bar.room_type_ids, [hotel.deluxe.id]);
    assert!(
        matches!(&dropping_a_type_the_child_sells, Err(RatesError::Conflict(m)) if m == "OTA still sells DLX; remove it there first"),
        "{dropping_a_type_the_child_sells:?}"
    );
    assert_eq!(invalid(child_adds_an_unsold_type), "OTA can only sell room types its parent sells, not STD");
    assert_eq!(invalid(unknown_type), "no such active room type in this property");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_and_updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rename = |name: &str| RatePlanChanges { name: Some(name.into()), ..RatePlanChanges::default() };

    let duplicate = hotel.try_plan(hotel.standard_plan("BAR", "LKR")).await;
    let renamed = hotel.try_update(&bar, rename("Best available")).await.unwrap();
    let stale = hotel.try_update(&bar, rename("Rack")).await;
    let unknown = hotel.try_update(&rates::RatePlan { id: Uuid::now_v7(), ..bar.clone() }, rename("X")).await;
    let mut tx = hotel.tx().await;
    let elsewhere =
        rates::create_rate_plan(&mut tx, hotel.tenant, hotel.user, Uuid::now_v7(), hotel.standard_plan("X", "USD"))
            .await;

    assert!(matches!(&duplicate, Err(RatesError::Conflict(m)) if m == "a rate plan with code BAR already exists"));
    assert_eq!((renamed.name.as_str(), renamed.version, renamed.code.as_str()), ("Best available", 2, "BAR"));
    assert!(matches!(stale, Err(RatesError::VersionMismatch("rate plan"))), "{stale:?}");
    assert!(matches!(unknown, Err(RatesError::NotFound("rate plan"))), "{unknown:?}");
    assert!(matches!(elsewhere, Err(RatesError::NotFound("property"))), "{elsewhere:?}");
}
