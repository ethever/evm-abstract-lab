//! Bounds that add no facts preserve the complete value without reduction work.

#[cfg(test)]
use crate::analysis::testing::{TestValue, TestValueQuery};
use crate::domain::EvmNumericAlgebra as _;

use crate::domain::{EvmNumericAlgebra, ValueAlgebra, WordElement};

use super::context;
use crate::{
    analysis::{ExecutionConfig, FrontierReason, transfer::WorkBudget},
    domain::{NumericValue, ProductAlgebra, identity::Symbol, provenance::Origin},
};
use alloy_primitives::U256;

fn marked_value(numeric: NumericValue) -> TestValue {
    let mut value = TestValue::top()
        .with_symbol(Symbol::CallValue, Some(7))
        .with_identity(11, 13)
        .with_numeric(numeric)
        .with_symbolic_limit();
    value.set_origin(Origin::Calldata);
    value.with_origin(Origin::Storage)
}

#[test]
fn top_and_equal_bounds_preserve_every_value_layer_with_only_comparison_work() {
    let config = ExecutionConfig::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let finite = domain.join(
        &TestValue::constant(U256::ONE),
        &TestValue::constant(U256::from(3)),
    );
    let runtime_alias = TestValue::top().with_identity(11, 13);
    for numeric in [
        finite.numeric().clone(),
        NumericValue::unsigned_range(U256::ONE, U256::from(9)).unwrap(),
        NumericValue::constant(U256::from(7)),
    ] {
        let before = marked_value(numeric.clone());
        for bounds in [NumericValue::top(), numeric.clone()] {
            let comparison = before.numeric().work_size() + bounds.work_size();
            assert!(comparison < domain.refinement_work(&before, 1));
            let mut budget = WorkBudget::new(comparison);
            let mut value = before.clone();
            assert_eq!(
                super::super::finish_refine(
                    &mut value,
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::default())
                    ),
                    TestValueQuery::Bounds(Box::new(bounds)),
                ),
                Ok(true)
            );
            assert_eq!(value, before);
            // TestValue equality deliberately excludes temporary identities.
            assert!(value.same_identity(&runtime_alias));
            assert!(value.symbolic_limit_reached());
            assert_eq!(budget.used(), comparison);
            assert!(!budget.exhausted());
        }
    }
}

#[test]
fn changed_bounds_keep_full_refinement_work_and_reject_disjoint_values_atomically() {
    let config = ExecutionConfig::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let before = marked_value(NumericValue::unsigned_range(U256::ONE, U256::from(9)).unwrap());
    for (lower, upper, feasible) in [(3, 5, true), (10, 12, false)] {
        let bounds = NumericValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap();
        let comparison = before.numeric().work_size() + bounds.work_size();
        let algebra = domain;
        let finite = before
            .numeric()
            .constants()
            .map_or(0, |values| values.len())
            + bounds.constants().map_or(0, |values| values.len());
        let import = domain
            .refinement_import_work(before.numeric(), 1, finite, 0)
            .unwrap()
            + before.work_size()
            - before.numeric().work_size();
        // Independent detached scalar meet, not a measured finish_refine run.
        let closure = before
            .numeric()
            .constrain(&bounds.observation())
            .map_or(0, |numeric| algebra.numeric_reduction_work(&numeric));
        // Clearing an inherited symbolic limit is an independent query on the
        // actual refined result; import/closure does not prepay that decision.
        let singleton_work = if feasible {
            let expected =
                domain.reduce(&TestValue::unsigned_range(U256::from(3), U256::from(5)).unwrap());
            domain.project(&expected.value).singleton_work()
        } else {
            0
        };
        let import_and_closure = comparison + import + closure;
        let fee = import_and_closure + singleton_work;
        let mut budget = WorkBudget::new(fee);
        let mut value = before.clone();
        assert_eq!(
            super::super::finish_refine(
                &mut value,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::default())
                ),
                TestValueQuery::Bounds(Box::new(bounds.clone())),
            ),
            Ok(feasible)
        );
        if feasible {
            let expected =
                domain.reduce(&TestValue::unsigned_range(U256::from(3), U256::from(5)).unwrap());
            assert_eq!(value.numeric(), domain.project(&expected.value).numeric());
            assert_eq!(
                value.interval().unsigned_bounds(),
                (U256::from(3), U256::from(5))
            );
            assert!(!value.contains(U256::from(2)) && !value.contains(U256::from(6)));
            assert!(value.contains(U256::from(3)) && value.contains(U256::from(5)));
            assert_eq!(value.expression(), before.expression());
            assert_eq!(value.provenance(), before.provenance());
            assert_eq!(value.identity(), before.identity());
            assert!(value.symbolic_limit_reached());
            assert!(value.same_identity(&TestValue::top().with_identity(11, 13)));
        } else {
            assert_eq!(value, before);
        }
        assert_eq!(budget.used(), fee);
        assert!(!budget.exhausted());

        let mut budget = WorkBudget::new(import_and_closure - 1);
        let mut value = before.clone();
        assert_eq!(
            super::super::finish_refine(
                &mut value,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::default())
                ),
                TestValueQuery::Bounds(Box::new(bounds.clone())),
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(value, before);
        let paid_prefix = comparison + if feasible { import } else { 0 };
        assert_eq!(budget.used(), paid_prefix);
        assert!(budget.exhausted());

        if feasible {
            let mut budget = WorkBudget::new(fee - 1);
            let mut value = before.clone();
            assert_eq!(
                super::super::finish_refine(
                    &mut value,
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::default())
                    ),
                    TestValueQuery::Bounds(Box::new(bounds)),
                ),
                Err(FrontierReason::Work)
            );
            assert_eq!(value, before);
            assert_eq!(budget.used(), import_and_closure);
            assert!(budget.exhausted());
        }
    }
}

#[test]
fn comparison_refusal_preserves_original_value_and_prior_paid_work() {
    let config = ExecutionConfig::default();
    let before = marked_value(NumericValue::unknown_byte());
    let prior = 7;
    let bounds = before.numeric().clone();
    let comparison = before.numeric().work_size() + bounds.work_size();
    let mut budget = WorkBudget::new(prior + comparison - 1);
    assert!(budget.charge(prior));
    let mut value = before.clone();
    assert_eq!(
        super::super::finish_refine(
            &mut value,
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::default())
            ),
            TestValueQuery::Bounds(Box::new(bounds)),
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), prior);
    assert!(budget.exhausted());

    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut budget = WorkBudget::new(10_000_000);
        assert!(budget.charge(prior));
        cancellation.cancel();
        let mut value = before.clone();
        assert_eq!(
            super::super::finish_refine(
                &mut value,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::default())
                ),
                TestValueQuery::Bounds(Box::new(NumericValue::top())),
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(value, before);
        assert_eq!(budget.used(), prior);
        assert!(budget.exhausted());
    });
}
