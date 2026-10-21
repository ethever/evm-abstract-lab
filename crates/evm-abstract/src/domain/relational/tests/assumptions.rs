//! Assumption retention and optional feasibility work have different outcomes.

fn product_algebra() -> crate::domain::ValueAlgebra<crate::domain::ProductAlgebra> {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
}
use super::super::{CheckResult, Constraint, QueryReason, RelationLimits, RelationState};
use crate::domain::symbolic::{ByteStateId, ByteView, ExprId, SourceId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn hash(limits: &RelationLimits) -> ExprId {
    let length = ExprId::constant(U256::from(64));
    ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            ExprId::constant(U256::ZERO),
            length,
        ),
        limits.expressions(),
    )
    .unwrap()
}

#[test]
fn opaque_hash_guards_are_retained_without_claiming_consistency() {
    let limits = RelationLimits {
        max_nodes: 64,
        ..RelationLimits::default()
    };
    let condition = hash(&limits);
    assert!(condition.nodes() < limits.max_nodes);
    assert!(condition.depth() < limits.max_depth);
    for nonzero in [false, true] {
        let mut state = RelationState::default();
        assert_eq!(
            state.check(&limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            state.assume_checked(&condition, nonzero, &limits, product_algebra().borrowed()),
            Ok(CheckResult::MayBeFeasible)
        );
        assert_eq!(state.len(), 1);
        assert!(state.constraints().any(|atom| atom
            == &Constraint::Truth {
                expression: condition.clone(),
                nonzero,
            }));
        assert!(!state.is_bottom());
    }
}

#[test]
fn expression_retention_failure_leaves_the_original_state_unchanged() {
    let mut state = RelationState::default();
    let input = ExprId::fresh().unwrap();
    assert_eq!(
        state.assume_checked(
            &input,
            true,
            &RelationLimits::default(),
            product_algebra().borrowed()
        ),
        Ok(CheckResult::MayBeFeasible)
    );
    let condition = ExprId::operation(
        opcode::ISZERO,
        &[input],
        RelationLimits::default().expressions(),
    )
    .unwrap();
    let before = state.clone();
    for limits in [
        RelationLimits {
            max_nodes: 1,
            ..RelationLimits::default()
        },
        RelationLimits {
            max_depth: 1,
            ..RelationLimits::default()
        },
    ] {
        assert_eq!(
            state.assume_checked(&condition, true, &limits, product_algebra().borrowed()),
            Err(QueryReason::ExpressionLimit)
        );
        assert_eq!(state, before);
    }
}

#[test]
fn constraint_retention_failure_keeps_existing_facts_and_accepts_duplicates() {
    let limits = RelationLimits {
        max_constraints: 1,
        ..RelationLimits::default()
    };
    let mut state = RelationState::default();
    let first = ExprId::fresh().unwrap();
    assert_eq!(
        state.assume_checked(&first, true, &limits, product_algebra().borrowed()),
        Ok(CheckResult::MayBeFeasible)
    );
    let before = state.clone();
    assert_eq!(
        state.assume_checked(
            &ExprId::fresh().unwrap(),
            false,
            &limits,
            product_algebra().borrowed()
        ),
        Err(QueryReason::ConstraintLimit)
    );
    assert_eq!(state, before);
    assert_eq!(
        state.assume_checked(&first, true, &limits, product_algebra().borrowed()),
        Ok(CheckResult::MayBeFeasible)
    );
    assert_eq!(state, before);
}

#[test]
fn disabled_and_invalid_policy_fail_before_retaining_an_atom() {
    let mut state = RelationState::default();
    let condition = ExprId::fresh().unwrap();
    for (limits, reason) in [
        (
            RelationLimits {
                enabled: false,
                ..RelationLimits::default()
            },
            QueryReason::Disabled,
        ),
        (
            RelationLimits {
                max_nodes: 0,
                ..RelationLimits::default()
            },
            QueryReason::Configuration,
        ),
    ] {
        assert_eq!(
            state.assume_checked(&condition, true, &limits, product_algebra().borrowed()),
            Err(reason)
        );
        assert_eq!(state, RelationState::default());
    }
}

#[test]
fn cancellation_after_retention_is_not_a_rejected_assumption() {
    let limits = RelationLimits::default();
    let condition = ExprId::fresh().unwrap();
    let token = analysis_control::Cancellation::new();
    token.cancel();
    let mut state = RelationState::default();
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            state.assume_checked(&condition, true, &limits, product_algebra().borrowed()),
            Ok(CheckResult::Undetermined(QueryReason::Cancelled))
        );
    });
    assert!(state.constraints().any(|atom| atom
        == &Constraint::Truth {
            expression: condition.clone(),
            nonzero: true,
        }));
    assert!(!state.is_bottom());
}
