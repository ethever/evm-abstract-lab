//! Exact reuse remains scoped, bounded and independent of numeric candidates.

#[cfg(test)]
use crate::analysis::testing::TestValueQuery;

use super::{MAX_ENTRIES, QueryCache};
use crate::resource::WorkBudget;
mod payload;
use crate::domain::{
    DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra,
    relational::{CheckResult, QueryReason, RelationLimits, RelationState},
    symbolic::{ExprId, SlotStateId, SourceId},
};
use alloy_primitives::U256;

fn read(source: SourceId) -> ExprId {
    let state = SlotStateId::observed(source, Vec::new(), Default::default()).unwrap();
    ExprId::slot_read(state, ExprId::constant(U256::ZERO), Default::default()).unwrap()
}

fn observe(expression: &ExprId, value: u64, limits: &RelationLimits) -> RelationState {
    let mut state = RelationState::default();
    state
        .observe(
            expression,
            &NumericValue::constant(U256::from(value)),
            limits,
        )
        .unwrap();
    state
}

#[test]
fn complete_snapshot_and_policy_changes_invalidate_answers() {
    let limits = RelationLimits::default();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::default().with_relations(limits),
    ));
    let expression = ExprId::fresh().unwrap();
    let state = RelationState::default();
    let changed_constraints = observe(&expression, 7, &limits);
    let changed_path = changed_constraints
        .join_guarded(
            &observe(&expression, 9, &limits),
            ExprId::fresh().unwrap(),
            &limits,
        )
        .unwrap();
    assert_eq!(changed_path.constraints().count(), 0);
    assert_ne!(changed_path.path(), state.path());
    let mut bottom = state.clone();
    assert_eq!(
        bottom.assume(
            &ExprId::constant(U256::ZERO),
            true,
            &limits,
            algebra.borrowed()
        ),
        CheckResult::ProvenImpossible
    );
    for changed in [&changed_constraints, &changed_path, &bottom] {
        let mut cache = QueryCache::default();
        cache.remember(
            &state,
            &expression,
            &limits,
            &TestValueQuery::Bounds(Box::new(NumericValue::top())),
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!(
            cache.get(
                &state.clone(),
                &expression.clone(),
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            Some(TestValueQuery::Bounds(Box::new(NumericValue::top())))
        );
        assert_eq!(
            cache.get(
                changed,
                &expression,
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            None
        );
        assert!(cache.values.is_empty());
    }
    for changed in [
        RelationLimits {
            enabled: false,
            ..limits
        },
        RelationLimits {
            max_constraints: limits.max_constraints + 1,
            ..limits
        },
        RelationLimits {
            max_nodes: limits.max_nodes + 1,
            ..limits
        },
        RelationLimits {
            max_depth: limits.max_depth + 1,
            ..limits
        },
    ] {
        let mut cache = QueryCache::default();
        cache.remember(
            &state,
            &expression,
            &limits,
            &TestValueQuery::Bounds(Box::new(NumericValue::top())),
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!(
            cache.get(
                &state,
                &expression,
                &changed,
                &mut WorkBudget::new(usize::MAX)
            ),
            None
        );
    }
}

#[test]
fn structurally_equal_expressions_reuse_only_the_same_source_identity() {
    let limits = RelationLimits::default();
    let relations = RelationState::default();
    let token = ExprId::fresh().unwrap();
    let source = SourceId::scoped(token.clone(), U256::ZERO);
    let expression = read(source.clone());
    let mut cache = QueryCache::default();
    cache.remember(
        &relations,
        &expression,
        &limits,
        &TestValueQuery::Bounds(Box::new(NumericValue::top())),
        &mut WorkBudget::new(usize::MAX),
    );
    assert_eq!(
        cache.get(
            &relations,
            &read(source),
            &limits,
            &mut WorkBudget::new(usize::MAX)
        ),
        Some(TestValueQuery::Bounds(Box::new(NumericValue::top())))
    );
    for source in [
        SourceId::fresh().unwrap(),
        SourceId::scoped(token, U256::ONE),
    ] {
        assert_eq!(
            cache.get(
                &relations,
                &read(source),
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            None
        );
    }
}

#[test]
fn unknown_answers_are_never_retained() {
    let limits = RelationLimits::default();
    let state = RelationState::default();
    let expression = ExprId::fresh().unwrap();
    let mut cache = QueryCache::default();
    for reason in [
        QueryReason::ConstraintLimit,
        QueryReason::Cancelled,
        QueryReason::ExpressionLimit,
    ] {
        cache.remember(
            &state,
            &expression,
            &limits,
            &TestValueQuery::Undetermined(reason),
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!(
            cache.get(
                &state,
                &expression,
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            None
        );
        assert!(cache.snapshot.is_none());
        assert_eq!(cache.retained_work, 0);
    }
    for answer in [
        TestValueQuery::Exact(U256::from(7)),
        TestValueQuery::Bounds(Box::new(NumericValue::top())),
        TestValueQuery::ProvenImpossible,
    ] {
        let mut cache = QueryCache::default();
        cache.remember(
            &state,
            &expression,
            &limits,
            &answer,
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!(
            cache.get(
                &state,
                &expression,
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            Some(answer)
        );
    }
}

#[test]
fn entry_and_retained_work_bounds_evict_only_optional_proofs() {
    let state = RelationState::default();
    for (limits, count) in [
        (RelationLimits::default(), MAX_ENTRIES + 1),
        (
            RelationLimits {
                max_nodes: state.work_size()
                    + 2 * (1
                        + 1
                        + super::answer_work(&TestValueQuery::Bounds(Box::new(
                            NumericValue::top(),
                        )))),
                ..RelationLimits::default()
            },
            3,
        ),
    ] {
        let mut cache = QueryCache::default();
        let expressions: Vec<_> = (0..count).map(|_| ExprId::fresh().unwrap()).collect();
        let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
            DomainSpec::default().with_relations(limits),
        ));
        for expression in &expressions {
            cache.remember(
                &state,
                expression,
                &limits,
                &TestValueQuery::Bounds(Box::new(NumericValue::top())),
                &mut WorkBudget::new(usize::MAX),
            );
            assert!(cache.values.len() <= MAX_ENTRIES);
            assert!(cache.retained_work <= limits.max_nodes);
        }
        assert_eq!(
            cache.get(
                &state,
                &expressions[0],
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            None
        );
        assert_eq!(
            cache.get(
                &state,
                expressions.last().unwrap(),
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            Some(TestValueQuery::Bounds(Box::new(NumericValue::top())))
        );
        // A missed proof is obtained from the original query again, then fits
        // exactly the same bounded cache without changing the query policy.
        let answer = state.local_value(&expressions[0], &limits, algebra.borrowed());
        assert_eq!(
            answer,
            TestValueQuery::Bounds(Box::new(NumericValue::top()))
        );
        cache.remember(
            &state,
            &expressions[0],
            &limits,
            &answer,
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!(
            cache.get(
                &state,
                &expressions[0],
                &limits,
                &mut WorkBudget::new(usize::MAX)
            ),
            Some(answer)
        );
    }
    let mut cache = QueryCache::default();
    let expression = ExprId::fresh().unwrap();
    let too_small = RelationLimits {
        max_nodes: 2,
        ..RelationLimits::default()
    };
    cache.remember(
        &state,
        &expression,
        &too_small,
        &TestValueQuery::Bounds(Box::new(NumericValue::top())),
        &mut WorkBudget::new(usize::MAX),
    );
    assert!(cache.snapshot.is_none());
    assert_eq!(
        cache.get(
            &state,
            &expression,
            &too_small,
            &mut WorkBudget::new(usize::MAX)
        ),
        None
    );
}
