mod common;

use common::Hotel;
use rates::{
    CancellationPolicyChanges, CancellationRule, MealPlan, MealSupplementChanges, NewCancellationPolicy,
    NewMealSupplement, Penalty, PenaltyKind, RatesError,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn breakfast(hotel: &Hotel, currency: &str, from: i64, to: Option<i64>) -> NewMealSupplement {
    NewMealSupplement {
        meal_plan: MealPlan::Bb,
        currency: currency.into(),
        adult_amount: 1_500,
        child_amount: 750,
        from: hotel.day(from),
        to: to.map(|days| hotel.day(days)),
    }
}

async fn add(hotel: &Hotel, input: NewMealSupplement) -> Result<rates::MealSupplement, RatesError> {
    let mut tx = hotel.tx().await;
    let created = rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await?;
    tx.commit().await.unwrap();
    Ok(created)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn supplements_are_per_meal_plan_and_currency_without_overlaps(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let usd = add(&hotel, breakfast(&hotel, "USD", 0, Some(180))).await.unwrap();
    let lkr = add(&hotel, NewMealSupplement { adult_amount: 450_000, ..breakfast(&hotel, "LKR", 0, None) }).await;
    let half_board = add(&hotel, NewMealSupplement { meal_plan: MealPlan::Hb, ..breakfast(&hotel, "USD", 0, None) });
    let half_board = half_board.await.unwrap();

    let overlapping = add(&hotel, breakfast(&hotel, "USD", 90, None)).await;
    let room_only = add(&hotel, NewMealSupplement { meal_plan: MealPlan::Ro, ..breakfast(&hotel, "USD", 0, None) });
    let backwards = add(&hotel, breakfast(&hotel, "EUR", 10, Some(10))).await;
    let next_season = add(&hotel, breakfast(&hotel, "USD", 180, None)).await.unwrap();
    let listed = rates::list_meal_supplements(&mut hotel.tx().await, hotel.property).await.unwrap();

    assert_eq!((usd.from, usd.to, usd.version), (hotel.day(0), Some(hotel.day(180)), 1));
    assert_eq!(lkr.unwrap().to, None, "open-ended");
    assert!(
        matches!(&overlapping, Err(RatesError::Conflict(m)) if m == "a BB supplement in USD already covers some of these dates"),
        "{overlapping:?}"
    );
    assert!(
        matches!(&room_only.await, Err(RatesError::Invalid(m)) if m == "room only (RO) has no supplement"),
        "RO is always 0"
    );
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "a supplement ends after it starts"));
    let order: Vec<(&str, MealPlan, time::Date)> =
        listed.iter().map(|s| (s.currency.as_str(), s.meal_plan, s.from)).collect();
    assert_eq!(
        order,
        [
            ("LKR", MealPlan::Bb, hotel.day(0)),
            ("USD", MealPlan::Bb, hotel.day(0)),
            ("USD", MealPlan::Bb, hotel.day(180)),
            ("USD", MealPlan::Hb, hotel.day(0)),
        ]
    );
    assert_eq!(next_season.from, hotel.day(180));
    assert_eq!(half_board.meal_plan, MealPlan::Hb);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_supplement_is_changed_with_its_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let usd = add(&hotel, breakfast(&hotel, "USD", 0, None)).await.unwrap();
    add(&hotel, breakfast(&hotel, "USD", -60, Some(0))).await.unwrap();
    let change = |changes: MealSupplementChanges, version: i32| {
        let hotel = &hotel;
        async move {
            let mut tx = hotel.tx().await;
            let updated = rates::update_meal_supplement(
                &mut tx,
                hotel.tenant,
                hotel.user,
                hotel.property,
                usd.id,
                version,
                changes,
            )
            .await?;
            tx.commit().await.unwrap();
            Ok::<_, RatesError>(updated)
        }
    };

    let raised = change(
        MealSupplementChanges {
            adult_amount: Some(1_800),
            to: Some(Some(hotel.day(365))),
            ..MealSupplementChanges::default()
        },
        1,
    )
    .await
    .unwrap();
    let stale = change(MealSupplementChanges { child_amount: Some(900), ..MealSupplementChanges::default() }, 1).await;
    let into_last_season =
        change(MealSupplementChanges { from: Some(hotel.day(-30)), ..MealSupplementChanges::default() }, 2).await;
    let reopened = change(MealSupplementChanges { to: Some(None), ..MealSupplementChanges::default() }, 2).await;

    assert_eq!(
        (raised.adult_amount, raised.child_amount, raised.to, raised.version),
        (1_800, 750, Some(hotel.day(365)), 2)
    );
    assert!(matches!(stale, Err(RatesError::VersionMismatch("meal supplement"))), "{stale:?}");
    assert!(matches!(into_last_season, Err(RatesError::Conflict(_))), "{into_last_season:?}");
    assert_eq!(reopened.unwrap().to, None);
}

fn penalty(kind: PenaltyKind, value: i64) -> Penalty {
    Penalty { kind, value }
}

fn flexible() -> NewCancellationPolicy {
    NewCancellationPolicy {
        name: "Flexible".into(),
        rules: vec![
            CancellationRule { days_before_arrival: 2, penalty: penalty(PenaltyKind::Nights, 1) },
            CancellationRule { days_before_arrival: 14, penalty: penalty(PenaltyKind::Percent, 2_500) },
        ],
        no_show: penalty(PenaltyKind::Percent, 10_000),
    }
}

async fn add_policy(hotel: &Hotel, input: NewCancellationPolicy) -> Result<rates::CancellationPolicy, RatesError> {
    let mut tx = hotel.tx().await;
    let created = rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await?;
    tx.commit().await.unwrap();
    Ok(created)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancellation_policies_keep_their_rules_in_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let policy = add_policy(&hotel, flexible()).await.unwrap();

    let duplicate = add_policy(&hotel, flexible()).await;
    let same_day_twice = add_policy(
        &hotel,
        NewCancellationPolicy {
            name: "Twice".into(),
            rules: vec![
                CancellationRule { days_before_arrival: 3, penalty: penalty(PenaltyKind::Nights, 1) },
                CancellationRule { days_before_arrival: 3, penalty: penalty(PenaltyKind::Nights, 2) },
            ],
            ..flexible()
        },
    )
    .await;
    let too_much = add_policy(
        &hotel,
        NewCancellationPolicy { name: "Harsh".into(), no_show: penalty(PenaltyKind::Percent, 20_000), ..flexible() },
    )
    .await;
    let mut tx = hotel.tx().await;
    let strict = CancellationPolicyChanges {
        name: Some("Strict".into()),
        rules: Some(vec![CancellationRule { days_before_arrival: 30, penalty: penalty(PenaltyKind::Amount, 50_000) }]),
        no_show: None,
    };
    let renamed =
        rates::update_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy.id, 1, strict)
            .await
            .unwrap();
    let listed = rates::list_cancellation_policies(&mut tx, hotel.property).await.unwrap();

    let days: Vec<i32> = policy.rules.iter().map(|rule| rule.days_before_arrival).collect();
    assert_eq!(days, [14, 2], "furthest from arrival first");
    assert!(
        matches!(&duplicate, Err(RatesError::Conflict(m)) if m == "a cancellation policy named Flexible already exists")
    );
    assert!(
        matches!(&same_day_twice, Err(RatesError::Invalid(m)) if m == "each rule needs its own number of days before arrival")
    );
    assert!(
        matches!(&too_much, Err(RatesError::Invalid(m)) if m == "a percent penalty is 1 to 10000 basis points"),
        "{too_much:?}"
    );
    assert_eq!((renamed.name.as_str(), renamed.version, renamed.no_show), ("Strict", 2, policy.no_show));
    assert_eq!(listed, [renamed]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rate_plan_names_a_cancellation_policy_of_its_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let policy = add_policy(&hotel, flexible()).await.unwrap();

    let with_policy =
        rates::NewRatePlan { cancellation_policy_id: Some(policy.id), ..hotel.standard_plan("BAR", "USD") };
    let with_policy = hotel.try_plan(with_policy).await.unwrap();
    let with_unknown =
        rates::NewRatePlan { cancellation_policy_id: Some(Uuid::now_v7()), ..hotel.standard_plan("RACK", "USD") };
    let with_unknown = hotel.try_plan(with_unknown).await;

    assert_eq!(with_policy.cancellation_policy_id, Some(policy.id));
    assert!(
        matches!(&with_unknown, Err(RatesError::Invalid(m)) if m == "no such cancellation policy in this property"),
        "{with_unknown:?}"
    );
}
