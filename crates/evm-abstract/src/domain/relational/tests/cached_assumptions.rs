//! Cached branch assumptions still retain and encode the complete conjunction.

use super::super::{CheckResult, Constraint, QueryReason, RelationLimits, RelationState};
use super::{c, limits, op};
use crate::{
    domain::symbolic::{ByteStateId, ByteView, ExprId, SourceId},
    resource::WorkBudget,
};
use alloy_primitives::U256;
use embedded_smt::{CacheScope, Cancellation, with_cancellation};
use revm_bytecode::opcode;

fn assume(
    state: &mut RelationState,
    condition: &ExprId,
    nonzero: bool,
    limits: &RelationLimits,
    scope: CacheScope,
    ledger: &mut WorkBudget,
) -> Result<CheckResult, QueryReason> {
    state.assume_checked_cached(
        condition,
        nonzero,
        limits,
        &mut ledger.smt_cache_query(scope),
    )
}

fn assert_guard(state: &RelationState, expression: &ExprId, nonzero: bool) {
    assert!(state.constraints().any(|atom| atom
        == &Constraint::Truth {
            expression: expression.clone(),
            nonzero,
        }));
}

#[test]
fn rejected_cached_retention_preserves_the_prior_state_without_consulting_a_warm_cache() {
    let policy = limits();
    let first = ExprId::fresh().unwrap();
    let condition = op(opcode::ISZERO, &[ExprId::fresh().unwrap()]);
    let mut prior = RelationState::default();
    assert_eq!(
        prior.assume_checked(&first, true, &policy),
        Ok(CheckResult::Sat)
    );
    let mut ledger = WorkBudget::new(100_000_000);
    assert_eq!(
        assume(
            &mut prior.clone(),
            &condition,
            true,
            &policy,
            policy.cache_scope(),
            &mut ledger
        ),
        Ok(CheckResult::Sat)
    );
    let before_cache = ledger.smt_cache_stats();
    for (restricted, reason) in [
        (
            RelationLimits {
                max_nodes: 1,
                ..policy
            },
            QueryReason::ExpressionLimit,
        ),
        (
            RelationLimits {
                max_depth: 1,
                ..policy
            },
            QueryReason::ExpressionLimit,
        ),
        (
            RelationLimits {
                max_constraints: 1,
                ..policy
            },
            QueryReason::ConstraintLimit,
        ),
        (
            RelationLimits {
                enabled: false,
                ..policy
            },
            QueryReason::Disabled,
        ),
        (
            RelationLimits {
                rlimit: 0,
                ..policy
            },
            QueryReason::Configuration,
        ),
        (
            RelationLimits {
                max_constraints: 0,
                ..policy
            },
            QueryReason::Configuration,
        ),
    ] {
        let mut state = prior.clone();
        assert_eq!(
            assume(
                &mut state,
                &condition,
                true,
                &restricted,
                policy.cache_scope(),
                &mut ledger
            ),
            Err(reason)
        );
        assert_eq!(state, prior);
        assert_eq!(ledger.smt_cache_stats(), before_cache);
    }
    assert!(!ledger.exhausted());
}

#[test]
fn a_warm_answer_cannot_skip_encoding_existing_atoms_or_current_cancellation() {
    let policy = limits();
    let complex = op(
        opcode::EQ,
        &[op(opcode::ADD, &[ExprId::fresh().unwrap(), c(1)]), c(5)],
    );
    let condition = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.assume_checked(&complex, true, &policy),
        Ok(CheckResult::Sat)
    );
    let mut ledger = WorkBudget::new(100_000_000);
    for _ in 0..2 {
        assert_eq!(
            assume(
                &mut state,
                &condition,
                true,
                &policy,
                policy.cache_scope(),
                &mut ledger
            ),
            Ok(CheckResult::Sat)
        );
    }
    assert_eq!(ledger.smt_cache_stats().candidate_hits, 1);
    let before_cache = ledger.smt_cache_stats();
    let before_state = state.clone();
    // Intentionally keep the warm namespace, so validation cannot rely only on
    // a different policy key. The duplicated shallow guard fits retention.
    for (restricted, reason) in [
        (
            RelationLimits {
                max_nodes: 2,
                ..policy
            },
            QueryReason::ExpressionLimit,
        ),
        (
            RelationLimits {
                max_depth: 1,
                ..policy
            },
            QueryReason::ExpressionLimit,
        ),
        (
            RelationLimits {
                max_constraints: 1,
                ..policy
            },
            QueryReason::ConstraintLimit,
        ),
    ] {
        assert_eq!(
            state.assume_checked(&condition, true, &restricted),
            Ok(CheckResult::Unknown(reason.clone()))
        );
        assert_eq!(
            assume(
                &mut state,
                &condition,
                true,
                &restricted,
                policy.cache_scope(),
                &mut ledger
            ),
            Ok(CheckResult::Unknown(reason))
        );
        assert_eq!(state, before_state);
        assert_eq!(ledger.smt_cache_stats(), before_cache);
    }
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            assume(
                &mut state,
                &condition,
                true,
                &policy,
                policy.cache_scope(),
                &mut ledger
            ),
            Ok(CheckResult::Unknown(QueryReason::Cancelled))
        );
    });
    assert_eq!(state, before_state);
    assert_eq!(ledger.smt_cache_stats(), before_cache);
    assert!(!state.is_bottom());
}

#[test]
fn encoding_and_native_unknown_keep_the_complete_new_guard_without_installing_bottom() {
    let encoding = RelationLimits {
        max_nodes: 64,
        ..RelationLimits::default()
    };
    let length = c(64);
    let hash = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            c(0),
            length,
        ),
        encoding.expressions(),
    )
    .unwrap();
    assert!(hash.nodes() < encoding.max_nodes);
    let native = RelationLimits {
        rlimit: 1,
        ..RelationLimits::default()
    };
    let equality = op(opcode::EQ, &[ExprId::fresh().unwrap(), c(7)]);
    let mut ledger = WorkBudget::new(100_000_000);
    for (condition, policy, reason) in [
        (hash, encoding, QueryReason::ExpressionLimit),
        (equality, native, QueryReason::ResourceLimit),
    ] {
        for nonzero in [false, true] {
            let mut independent = RelationState::default();
            let expected = independent.assume_checked(&condition, nonzero, &policy);
            assert_eq!(expected, Ok(CheckResult::Unknown(reason.clone())));
            let mut state = RelationState::default();
            assert_eq!(
                assume(
                    &mut state,
                    &condition,
                    nonzero,
                    &policy,
                    policy.cache_scope(),
                    &mut ledger
                ),
                expected
            );
            assert_eq!(state, independent);
            assert_guard(&state, &condition, nonzero);
            assert!(!state.is_bottom());
        }
    }
    assert_eq!(ledger.smt_cache_stats().entries, 0);
}

#[test]
fn full_guarded_path_and_definition_suffix_determine_cached_feasibility() {
    let policy = limits();
    let word = ExprId::fresh().unwrap();
    let selector = ExprId::fresh().unwrap();
    let branch = |number| {
        let mut state = RelationState::default();
        assert_eq!(
            state.add_equal(&word, &c(number), &policy),
            CheckResult::Sat
        );
        state
    };
    let joined = branch(2)
        .join_guarded(&branch(3), selector.clone(), &policy)
        .unwrap();
    let derived = op(opcode::ADD, &[word, c(1)]);
    let mut ledger = WorkBudget::new(100_000_000);
    for (number, expected) in [(3, CheckResult::Sat), (4, CheckResult::Unsat)] {
        let condition = op(opcode::EQ, &[derived.clone(), c(number)]);
        for _ in 0..2 {
            let mut state = joined.clone();
            assert_eq!(
                state.assume_checked(&selector, true, &policy),
                Ok(CheckResult::Sat)
            );
            let mut independent = state.clone();
            assert_eq!(
                independent.assume_checked(&condition, true, &policy),
                Ok(expected.clone())
            );
            assert_eq!(
                assume(
                    &mut state,
                    &condition,
                    true,
                    &policy,
                    policy.cache_scope(),
                    &mut ledger
                ),
                Ok(expected.clone())
            );
            assert_eq!(state, independent);
        }
    }
    assert_eq!(ledger.smt_cache_stats().candidate_hits, 2);
    assert_eq!(ledger.smt_cache_stats().entries, 2);
    assert!(!ledger.exhausted());
}

#[test]
fn already_bottom_is_immediate_unsat_even_when_policy_and_current_token_reject_queries() {
    let policy = limits();
    let mut ledger = WorkBudget::new(100_000_000);
    let mut state = RelationState::default();
    assert_eq!(
        assume(
            &mut state,
            &ExprId::constant(U256::ZERO),
            true,
            &policy,
            policy.cache_scope(),
            &mut ledger
        ),
        Ok(CheckResult::Unsat)
    );
    let before_state = state.clone();
    let before_cache = ledger.smt_cache_stats();
    let token = Cancellation::new();
    token.cancel();
    let invalid = RelationLimits {
        enabled: false,
        rlimit: 0,
        max_nodes: 0,
        ..policy
    };
    with_cancellation(&token, || {
        assert_eq!(
            assume(
                &mut state,
                &ExprId::fresh().unwrap(),
                false,
                &invalid,
                policy.cache_scope(),
                &mut ledger
            ),
            Ok(CheckResult::Unsat)
        );
    });
    assert_eq!(state, before_state);
    assert_eq!(ledger.smt_cache_stats(), before_cache);
}
