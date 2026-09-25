//! Property-based check of price derivation: random chains of up to 3 derived plans with random formulas
//! (percent or amount, negative values, rounding steps), random prices and random bulk changes on the
//! standard plan. After every step each derived plan must hold exactly its parent's cells, each equal to the
//! parent's price through its formula, computed here with exact arithmetic.

mod common;

use common::{Hotel, derived};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{BulkChange, ChangeMode, Price, PriceChange, PriceChangeMode, RatePlan, RatePlanChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Chains tried per run. Each run draws new ones; a failure prints the chain and step.
const CHAINS: usize = 10;

#[derive(Debug, Clone, Copy)]
struct Formula {
    mode: ChangeMode,
    value: i64,
    step: i64,
}

fn formula() -> impl Strategy<Value = Formula> {
    let step = prop_oneof![Just(1_i64), Just(5), Just(50), Just(100), Just(1_000)];
    prop_oneof![
        // Percent: at most +200% to keep root 10M × 3 derivations × 3^(+200%) under MAX_AMOUNT.
        // Bound: 10M × 3^3 × 3^(200%) ≈ 13.3B < 100B.
        (-10_000..=20_000_i64, step.clone()).prop_map(|(value, step)| Formula {
            mode: ChangeMode::Percent,
            value,
            step
        }),
        (-2_000_000..=2_000_000_i64, step).prop_map(|(value, step)| Formula { mode: ChangeMode::Amount, value, step }),
    ]
}

#[derive(Debug, Clone)]
enum Op {
    /// Set `amount` for `occupancy` adults of DLX on days `[day, day + days)`.
    SetPrices { day: i64, days: i64, occupancy: i32, amount: i64 },
    /// Change DLX on `[day, day + days)` by `mode` and `value`.
    Bulk { day: i64, days: i64, mode: PriceChangeMode, value: i64 },
    /// Give plan `level` (1-based) a new formula.
    Reformulate { level: usize, formula: Formula },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        // Root amounts up to 10M; 3 derivations × 3^(+200%) ≈ 270M < 100B.
        (0..40_i64, 1..10_i64, 1..=3_i32, 0..=10_000_000_i64)
            .prop_map(|(day, days, occupancy, amount)| Op::SetPrices { day, days, occupancy, amount }),
        // Bulk percent: -10000..=100000 (same as select() bounds), capped at ±200% for safety.
        (0..40_i64, 1..20_i64, -10_000..=20_000_i64).prop_map(|(day, days, value)| Op::Bulk {
            day,
            days,
            mode: PriceChangeMode::Percent,
            value
        }),
        (0..40_i64, 1..20_i64, -500_000..=500_000_i64).prop_map(|(day, days, value)| Op::Bulk {
            day,
            days,
            mode: PriceChangeMode::Amount,
            value
        }),
        (1..=3_usize, formula()).prop_map(|(level, formula)| Op::Reformulate { level, formula }),
    ]
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derived_prices_follow_their_formulas_down_the_chain(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut runner = TestRunner::default();

    for chain in 0..CHAINS {
        let formulas = prop::collection::vec(formula(), 1..=3).new_tree(&mut runner).unwrap().current();
        let ops = prop::collection::vec(op(), 1..12).new_tree(&mut runner).unwrap().current();
        let root = hotel.plan(hotel.standard_plan(&format!("ROOT{chain}"), "USD")).await;
        let mut plans: Vec<RatePlan> = vec![root.clone()];
        for (level, formula) in formulas.iter().enumerate() {
            let input = rates::NewRatePlan {
                rounding_step: formula.step,
                ..hotel.derived_plan(&format!("C{chain}L{level}"), &plans[level], formula.mode, formula.value)
            };
            plans.push(hotel.plan(input).await);
        }

        let mut root_model: std::collections::BTreeMap<(time::Date, i32), i64> = std::collections::BTreeMap::new();

        for (step, op) in ops.iter().enumerate() {
            let context = format!("chain {chain} {formulas:?}, step {step} of {ops:?}");
            let mut tx = hotel.tx().await;
            let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
            match op.clone() {
                Op::SetPrices { day, days, occupancy, amount } => {
                    let prices = hotel.prices(hotel.deluxe.id, day, day + days, occupancy, amount);
                    rates::set_prices(&mut tx, tenant, user, property, root.id, &prices).await.unwrap();
                    // Update model with new prices.
                    for price in &prices {
                        root_model.insert((price.date, price.occupancy), price.amount);
                    }
                }
                Op::Bulk { day, days, mode, value } => {
                    let change = BulkChange {
                        from: hotel.day(day),
                        to: hotel.day(day + days),
                        weekdays: vec![],
                        room_type_ids: vec![hotel.deluxe.id],
                        occupancies: vec![],
                        change: PriceChange { mode, value },
                    };
                    rates::bulk_change(&mut tx, tenant, user, property, root.id, &change).await.unwrap();
                    // Update model: apply the bulk change with same rounding.
                    for (date, occupancy) in root_model.keys().cloned().collect::<Vec<_>>() {
                        if date >= change.from && date < change.to {
                            let old_amount = root_model[&(date, occupancy)];
                            let new_amount = match mode {
                                PriceChangeMode::Percent => {
                                    let (num, denom) =
                                        (i128::from(old_amount) * (10_000 + i128::from(value)), 10_000_i128);
                                    let (whole, rest) = (num.max(0) / denom, num.max(0) % denom);
                                    let steps = if 2 * rest >= denom { whole + 1 } else { whole };
                                    i64::try_from(steps).unwrap()
                                }
                                PriceChangeMode::Amount => (old_amount + value).max(0),
                                PriceChangeMode::Set => value,
                            };
                            root_model.insert((date, occupancy), new_amount);
                        }
                    }
                }
                Op::Reformulate { level, formula } => {
                    let Some(plan) = plans.get(level) else { continue };
                    let changes = RatePlanChanges {
                        derive_mode: Some(formula.mode),
                        derive_value: Some(formula.value),
                        rounding_step: Some(formula.step),
                        ..RatePlanChanges::default()
                    };
                    let updated =
                        rates::update_rate_plan(&mut tx, tenant, user, property, plan.id, plan.version, changes)
                            .await
                            .unwrap();
                    plans[level] = updated;
                    // Root model unchanged (reformulation affects children only).
                }
            }
            tx.commit().await.unwrap();

            // Verify root plan matches model.
            let stored_root = hotel.stored(&root).await;
            let expected_root: Vec<Price> = root_model
                .iter()
                .map(|((date, occupancy), amount)| Price {
                    room_type_id: hotel.deluxe.id,
                    date: *date,
                    occupancy: *occupancy,
                    amount: *amount,
                })
                .collect();
            assert_eq!(stored_root, expected_root, "root {}: {context}", root.code);

            for pair in plans.windows(2) {
                let (parent, child) = (&pair[0], &pair[1]);
                let expected: Vec<Price> = hotel
                    .stored(parent)
                    .await
                    .into_iter()
                    .map(|price| Price {
                        amount: derived(
                            price.amount,
                            child.derive_mode.unwrap(),
                            child.derive_value.unwrap(),
                            child.rounding_step,
                        ),
                        ..price
                    })
                    .collect();
                assert_eq!(hotel.stored(child).await, expected, "{} from {}: {context}", child.code, parent.code);
            }
        }
    }
}
