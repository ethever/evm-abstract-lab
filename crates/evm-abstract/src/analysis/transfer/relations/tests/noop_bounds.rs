//! Bounds that add no facts preserve the complete value without reduction work.

use super::context;
use crate::{
    analysis::{ExecutionConfig, FrontierReason, transfer::WorkBudget},
    domain::{
        AbstractValue, Domain, NumericValue, identity::Symbol, provenance::Origin,
        relational::ValueQuery,
    },
};
use alloy_primitives::U256;

fn marked_value(numeric: NumericValue) -> AbstractValue {
    let mut value = AbstractValue::top()
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
    let domain = Domain::default();
    let finite = domain.join(
        &AbstractValue::constant(U256::ONE),
        &AbstractValue::constant(U256::from(3)),
    );
    let runtime_alias = AbstractValue::top().with_identity(11, 13);
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
                    &mut context(&config, &mut budget),
                    ValueQuery::Bounds(Box::new(bounds)),
                ),
                Ok(true)
            );
            assert_eq!(value, before);
            // AbstractValue equality deliberately excludes temporary identities.
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
    let domain = Domain::default();
    let before = marked_value(NumericValue::unsigned_range(U256::ONE, U256::from(9)).unwrap());
    for (lower, upper, feasible) in [(3, 5, true), (10, 12, false)] {
        let bounds = NumericValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap();
        let comparison = before.numeric().work_size() + bounds.work_size();
        let fee = comparison + domain.refinement_work(&before, 1);
        let mut budget = WorkBudget::new(fee);
        let mut value = before.clone();
        assert_eq!(
            super::super::finish_refine(
                &mut value,
                &mut context(&config, &mut budget),
                ValueQuery::Bounds(Box::new(bounds.clone())),
            ),
            Ok(feasible)
        );
        if feasible {
            let expected = domain
                .reduce(&AbstractValue::unsigned_range(U256::from(3), U256::from(5)).unwrap());
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
            assert!(value.same_identity(&AbstractValue::top().with_identity(11, 13)));
        } else {
            assert_eq!(value, before);
        }
        assert_eq!(budget.used(), fee);
        assert!(!budget.exhausted());

        let mut budget = WorkBudget::new(fee - 1);
        let mut value = before.clone();
        assert_eq!(
            super::super::finish_refine(
                &mut value,
                &mut context(&config, &mut budget),
                ValueQuery::Bounds(Box::new(bounds)),
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(value, before);
        assert_eq!(budget.used(), comparison);
        assert!(budget.exhausted());
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
            &mut context(&config, &mut budget),
            ValueQuery::Bounds(Box::new(bounds)),
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
                &mut context(&config, &mut budget),
                ValueQuery::Bounds(Box::new(NumericValue::top())),
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(value, before);
        assert_eq!(budget.used(), prior);
        assert!(budget.exhausted());
    });
}
