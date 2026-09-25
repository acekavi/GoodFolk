//! Property-based check of the derivation rules: random sequences of "derive a plan" and "move a plan to
//! another parent" must be accepted exactly when a simple model of the rules says so (same currency, a
//! standard or derived parent, at most 3 levels, no cycles), and every accepted tree must keep the rules.

mod common;

use common::Hotel;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{ChangeMode, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, RatesError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::HashMap;
use uuid::Uuid;

/// Sequences tried per run. Each run draws new ones; a failure prints the sequence that broke a rule.
const SEQUENCES: usize = 12;

#[derive(Debug, Clone)]
enum Op {
    /// Derive a new plan from plan `parent`, in its currency or deliberately in the other one.
    Derive { parent: usize, other_currency: bool },
    /// Move plan `plan` under plan `parent`.
    Move { plan: usize, parent: usize },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..12usize, prop::bool::weighted(0.2))
            .prop_map(|(parent, other_currency)| Op::Derive { parent, other_currency }),
        (0..12usize, 0..12usize).prop_map(|(plan, parent)| Op::Move { plan, parent }),
    ]
}

/// The rules, on plans held in memory: id to (kind, currency, parent).
struct Model(HashMap<Uuid, (PlanKind, String, Option<Uuid>)>);

impl Model {
    fn depth(&self, id: Uuid) -> usize {
        let mut depth = 0;
        let mut current = self.0[&id].2;
        while let Some(parent) = current {
            depth += 1;
            current = self.0[&parent].2;
        }
        depth
    }

    /// Levels of derived plans below `id`.
    fn height(&self, id: Uuid) -> usize {
        self.0
            .iter()
            .filter(|(_, plan)| plan.2 == Some(id))
            .map(|(child, _)| 1 + self.height(*child))
            .max()
            .unwrap_or(0)
    }

    fn is_below(&self, id: Uuid, ancestor: Uuid) -> bool {
        let mut current = Some(id);
        while let Some(plan) = current {
            if plan == ancestor {
                return true;
            }
            current = self.0[&plan].2;
        }
        false
    }

    fn may_derive(&self, parent: Uuid, currency: &str, height: usize) -> bool {
        let (kind, parent_currency, _) = &self.0[&parent];
        *kind != PlanKind::Custom && parent_currency == currency && self.depth(parent) + 1 + height <= 3
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn random_derivations_and_moves_follow_the_rules(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let roots = [
        hotel.plan(hotel.standard_plan("USD1", "USD")).await,
        hotel.plan(hotel.standard_plan("USD2", "USD")).await,
        hotel.plan(hotel.standard_plan("LKR1", "LKR")).await,
        hotel.plan(NewRatePlan { kind: PlanKind::Custom, ..hotel.standard_plan("CUSTOM", "USD") }).await,
    ];
    let mut runner = TestRunner::default();

    for sequence in 0..SEQUENCES {
        let ops = prop::collection::vec(op(), 1..16).new_tree(&mut runner).unwrap().current();
        let mut plans: Vec<RatePlan> = roots.to_vec();
        plans.extend(hotel.plans().await.into_iter().filter(|plan| plan.kind == PlanKind::Derived));
        let mut model = Model(plans.iter().map(|p| (p.id, (p.kind, p.currency.clone(), p.parent_id))).collect());

        for (step, op) in ops.iter().enumerate() {
            let context = format!("sequence {sequence}, step {step} of {ops:?}");
            match *op {
                Op::Derive { parent, other_currency } => {
                    let parent = plans[parent % plans.len()].clone();
                    let currency = match (other_currency, parent.currency.as_str()) {
                        (false, currency) => currency.to_owned(),
                        (true, "USD") => "LKR".into(),
                        (true, _) => "USD".into(),
                    };
                    let expected = model.may_derive(parent.id, &currency, 0);
                    let code = format!("D{sequence}-{step}");
                    let input =
                        NewRatePlan { currency, ..hotel.derived_plan(&code, &parent, ChangeMode::Percent, 500) };
                    match hotel.try_plan(input).await {
                        Ok(created) => {
                            assert!(expected, "accepted against the rules: {context}");
                            model.0.insert(created.id, (created.kind, created.currency.clone(), created.parent_id));
                            plans.push(created);
                        }
                        Err(RatesError::Invalid(_)) => assert!(!expected, "refused a valid derivation: {context}"),
                        Err(err) => panic!("{err:?}: {context}"),
                    }
                }
                Op::Move { plan, parent } => {
                    let (plan, parent) = (plans[plan % plans.len()].clone(), plans[parent % plans.len()].clone());
                    if plan.kind != PlanKind::Derived {
                        continue;
                    }
                    let expected = !model.is_below(parent.id, plan.id)
                        && model.may_derive(parent.id, &plan.currency, model.height(plan.id));
                    let changes = RatePlanChanges { parent_id: Some(parent.id), ..RatePlanChanges::default() };
                    match hotel.try_update(&plan, changes).await {
                        Ok(moved) => {
                            assert!(expected, "accepted against the rules: {context}");
                            model.0.get_mut(&moved.id).unwrap().2 = moved.parent_id;
                        }
                        Err(RatesError::Invalid(_)) => assert!(!expected, "refused a valid move: {context}"),
                        Err(err) => panic!("{err:?}: {context}"),
                    }
                    // Versions change with every move; refresh them.
                    let fresh = hotel.plans().await;
                    for plan in &mut plans {
                        *plan = fresh.iter().find(|p| p.id == plan.id).unwrap().clone();
                    }
                }
            }

            let listed = hotel.plans().await;
            assert_eq!(listed.len(), model.0.len(), "every plan is in the tree: {context}");
            for plan in listed {
                assert!(plan.depth <= 3, "{} is {} levels deep: {context}", plan.code, plan.depth);
                assert_eq!(usize::try_from(plan.depth).unwrap(), model.depth(plan.id), "{context}");
                if let Some(parent) = plan.parent_id {
                    assert_eq!(model.0[&parent].1, plan.currency, "currency of {}: {context}", plan.code);
                    assert_ne!(model.0[&parent].0, PlanKind::Custom, "{context}");
                }
            }
        }
    }
}
