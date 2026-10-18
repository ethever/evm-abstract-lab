use super::{Failure, GuardRetention, RetentionCause};
use crate::{
    U256,
    analysis::{ExecutionConfig, FrontierReason},
    domain::{
        AbstractValue,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprLimits, StateError},
    },
};
use revm_bytecode::opcode;

fn symbolic() -> AbstractValue {
    AbstractValue::top().with_expression(ExprId::fresh().unwrap())
}

// Execute the original eight operations to obtain raw errors, including the
// error-free suffix after a failed GT or AND. Numeric facts alone are not causes.
fn guard_trace(
    offset: &AbstractValue,
    size: &AbstractValue,
    max_nodes: usize,
) -> (AbstractValue, GuardRetention, Vec<Option<ExprError>>) {
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = max_nodes;
    config.analysis.relations.max_depth = 256;
    let domain = config.domain().unwrap();
    let limit = AbstractValue::constant(super::super::logical::MAX_END);
    let mut retention = GuardRetention::new(offset, size);
    let mut errors = Vec::new();
    let mut apply = |op, args: &[AbstractValue]| {
        let reduction = domain.apply_detailed(op, args);
        retention.record(reduction.symbolic_error.as_ref());
        errors.push(reduction.symbolic_error);
        reduction.value
    };
    let empty = apply(opcode::ISZERO, std::slice::from_ref(size));
    let greater = apply(opcode::GT, &[offset.clone(), limit.clone()]);
    let offset_fits = apply(opcode::ISZERO, &[greater]);
    let remaining = apply(opcode::SUB, &[limit, offset.clone()]);
    let greater = apply(opcode::GT, &[size.clone(), remaining]);
    let size_fits = apply(opcode::ISZERO, &[greater]);
    let fits = apply(opcode::AND, &[offset_fits, size_fits]);
    let condition = apply(opcode::OR, &[empty, fits]);
    (condition, retention, errors)
}

#[test]
fn finite_guard_failures_survive_the_error_free_suffix() {
    for (max_nodes, failed_step, nodes, depth) in [(10, 6, 11, 5), (4, 4, 5, 3)] {
        let (condition, retention, errors) = guard_trace(&symbolic(), &symbolic(), max_nodes);
        assert_eq!(errors.len(), 8);
        assert!(errors[..failed_step].iter().all(Option::is_none));
        assert_eq!(errors[failed_step], Some(ExprError::Limit { nodes, depth }));
        assert!(errors[failed_step + 1..].iter().all(Option::is_none));
        assert!(condition.symbolic_expression().is_none());
        assert!(condition.symbolic_limit_reached());
        assert!(condition.may_be_zero() && condition.may_be_nonzero());
        assert_eq!(retention.cause(), Some(RetentionCause::Expression));
        let failure = Failure::missing_expression(Some(&retention));
        assert_eq!(
            failure.reason,
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        );
        assert_eq!(failure.retention, Some(RetentionCause::Expression));
    }
}

#[test]
fn finite_evidence_does_not_change_a_proven_constant_condition() {
    let (condition, retention, errors) =
        guard_trace(&symbolic(), &AbstractValue::constant(U256::ZERO), 2);
    assert_eq!(errors[1], Some(ExprError::Limit { nodes: 3, depth: 2 }));
    assert_eq!(retention.cause(), Some(RetentionCause::Expression));
    // The caller can return before requesting a missing-expression Failure.
    assert_eq!(condition.singleton(), Some(U256::from(1)));
    assert!(!condition.may_be_zero());
}

#[test]
fn retained_and_anonymous_guards_without_raw_errors_have_no_cause() {
    for (offset, size, retained) in [
        (symbolic(), symbolic(), true),
        (AbstractValue::top(), AbstractValue::top(), false),
    ] {
        let (condition, retention, errors) = guard_trace(&offset, &size, 64);
        assert!(errors.iter().all(Option::is_none));
        assert_eq!(condition.symbolic_expression().is_some(), retained);
        assert!(!condition.symbolic_limit_reached());
        assert_eq!(retention.cause(), None);
        assert_eq!(
            Failure::missing_expression(Some(&retention)).retention,
            None
        );
    }
    assert_eq!(Failure::missing_expression(None).retention, None);
}

#[test]
fn either_inherited_marker_blocks_a_later_real_finite_failure() {
    for (offset, size) in [
        (symbolic().with_symbolic_limit(), symbolic()),
        (symbolic(), symbolic().with_symbolic_limit()),
    ] {
        let (_, retention, errors) = guard_trace(&offset, &size, 10);
        assert_eq!(
            errors[6],
            Some(ExprError::Limit {
                nodes: 11,
                depth: 5
            })
        );
        assert_eq!(retention.cause(), None);
        assert_eq!(
            Failure::missing_expression(Some(&retention)).retention,
            None
        );
    }
}

#[test]
fn nonfinite_errors_block_finite_evidence_in_both_orders() {
    let finite = ExprId::operation(
        opcode::AND,
        &[ExprId::fresh().unwrap(), ExprId::fresh().unwrap()],
        ExprLimits {
            max_nodes: 2,
            max_depth: 256,
        },
    )
    .unwrap_err();
    assert_eq!(finite, ExprError::Limit { nodes: 3, depth: 2 });
    for blocker in [
        ExprError::Limit {
            nodes: usize::MAX,
            depth: 1,
        },
        ExprError::Limit {
            nodes: 1,
            depth: usize::MAX,
        },
        StateError::IdentityExhausted.into(),
        ExprError::Cancelled,
        ExprError::Unsupported(opcode::STOP),
        ExprError::Arity {
            opcode: opcode::GT,
            expected: 2,
            actual: 1,
        },
        ExprError::ExportBytes { bytes: 2, limit: 1 },
    ] {
        for finite_first in [true, false] {
            let mut retention = GuardRetention::new(&symbolic(), &symbolic());
            let errors = if finite_first {
                [&finite, &blocker]
            } else {
                [&blocker, &finite]
            };
            retention.record(Some(errors[0]));
            retention.record(None);
            retention.record(Some(errors[1]));
            retention.record(None);
            assert_eq!(retention.cause(), None, "{blocker:?}, {finite_first}");
            assert_eq!(
                Failure::missing_expression(Some(&retention)).retention,
                None,
                "{blocker:?}, {finite_first}"
            );
        }
    }
}

#[test]
fn independent_failures_stay_plain_despite_eligible_guard_evidence() {
    let (_, retention, _) = guard_trace(&symbolic(), &symbolic(), 10);
    assert_eq!(retention.cause(), Some(RetentionCause::Expression));
    for reason in [
        FrontierReason::Work,
        FrontierReason::Relations(QueryReason::Cancelled),
        FrontierReason::Relations(QueryReason::ConstraintLimit),
        FrontierReason::Relations(QueryReason::ExpressionLimit),
    ] {
        let failure = Failure::plain(reason.clone());
        assert_eq!(failure.reason, reason);
        assert_eq!(failure.retention, None);
    }
}
