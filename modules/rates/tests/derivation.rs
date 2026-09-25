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
        (-10_000..=100_000_i64, step.clone()).prop_map(|(value, step)| Formula {
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
        (0..40_i64, 1..10_i64, 1..=3_i32, 0..=100_000_000_i64)
            .prop_map(|(day, days, occupancy, amount)| Op::SetPrices { day, days, occupancy, amount }),
        (0..40_i64, 1..20_i64, -5_000..=5_000_i64).prop_map(|(day, days, value)| Op::Bulk {
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

        for (step, op) in ops.iter().enumerate() {
            let context = format!("chain {chain} {formulas:?}, step {step} of {ops:?}");
            let mut tx = hotel.tx().await;
            let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
            match op.clone() {
                Op::SetPrices { day, days, occupancy, amount } => {
                    let prices = hotel.prices(hotel.deluxe.id, day, day + days, occupancy, amount);
                    rates::set_prices(&mut tx, tenant, user, property, root.id, &prices).await.unwrap();
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
                }
            }
            tx.commit().await.unwrap();

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
