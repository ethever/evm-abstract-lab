//! Full independent-query oracles for immutable encoding reuse.

use super::check;
use crate::{
    domain::{
        AbstractValue,
        relational::{
            CandidateError, CheckResult, Constraint, QueryReason, RelationLimits, RelationState,
            SmtProvider, ValueQuery,
        },
        symbolic::{ByteStateId, ExprId, SlotStateId, SourceId, StateBinding},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn op(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, Default::default()).unwrap()
}

fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        max_nodes: 8192,
        rlimit: 10_000_000,
        ..RelationLimits::default()
    }
}

fn batch(
    state: &RelationState,
    expression: &ExprId,
    candidates: &[U256],
    limits: &RelationLimits,
) -> Vec<CheckResult> {
    let cost = state
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(limits.rlimit as usize);
    let mut ledger = WorkBudget::new(cost.saturating_mul(candidates.len()));
    let answers = check(
        state,
        expression,
        candidates,
        limits,
        &mut ledger.query_budget(cost),
    )
    .unwrap();
    assert_eq!(ledger.used(), cost * candidates.len());
    assert!(!ledger.exhausted());
    answers
}

fn oracle(
    state: &RelationState,
    expression: &ExprId,
    candidates: &[U256],
    limits: &RelationLimits,
) -> Vec<CheckResult> {
    candidates
        .iter()
        .map(|candidate| state.can_equal(expression, *candidate, limits))
        .collect()
}

#[test]
fn multiple_models_still_filter_all_256_byte_candidates_to_the_same_64() {
    let limits = policy(SmtProvider::Z3);
    let x = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    state
        .observe(
            &x,
            AbstractValue::unsigned_range(U256::ZERO, U256::from(255))
                .unwrap()
                .numeric(),
            &limits,
        )
        .unwrap();
    let square = op(opcode::MUL, &[x.clone(), x.clone()]);
    let guard = op(opcode::EQ, &[op(opcode::AND, &[square, c(15)]), c(1)]);
    state
        .retain(
            Constraint::Truth {
                expression: guard,
                nonzero: true,
            },
            &limits,
        )
        .unwrap();
    assert_eq!(state.unique_value(&x, &limits), ValueQuery::Multiple);
    let candidates = (0u64..256).map(U256::from).collect::<Vec<_>>();
    let expected = oracle(&state, &x, &candidates, &limits);
    assert_eq!(batch(&state, &x, &candidates, &limits), expected);
    let retained = expected
        .into_iter()
        .enumerate()
        .filter_map(|(value, answer)| match answer {
            CheckResult::Sat => Some(value),
            CheckResult::Unsat => None,
            CheckResult::Unknown(reason) => panic!("oracle stopped: {reason:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 64);
    assert_eq!(
        retained,
        (0usize..256)
            .filter(|value| matches!(value % 16, 1 | 7 | 9 | 15))
            .collect::<Vec<_>>()
    );
}

#[test]
fn complete_unconstrained_byte_suffix_survives_all_context_chunks() {
    let limits = policy(SmtProvider::Z3);
    let expression = op(opcode::AND, &[ExprId::fresh().unwrap(), c(255)]);
    let candidates = (0u64..256).map(U256::from).collect::<Vec<_>>();
    let state = RelationState::default();
    assert_eq!(
        batch(&state, &expression, &candidates, &limits),
        vec![CheckResult::Sat; 256]
    );
}

#[test]
fn shared_encoding_retains_byte_bindings_padding_and_symbolic_storage_aliases() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let input = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            c(1),
            vec![StateBinding {
                index: U256::ZERO,
                value: ExprId::fresh().unwrap(),
                numeric: AbstractValue::constant(U256::from(0xaa)).numeric().clone(),
            }],
            limits.expressions().into(),
        )
        .unwrap();
        for (index, expected) in [(0, 0xaa), (1, 0)] {
            let read = ExprId::byte_read(input.clone(), c(index), limits.expressions()).unwrap();
            let candidates = [U256::ZERO, U256::from(0xaa), U256::from(0xff)];
            let answers = batch(&RelationState::default(), &read, &candidates, &limits);
            assert_eq!(
                answers,
                oracle(&RelationState::default(), &read, &candidates, &limits)
            );
            for (candidate, answer) in candidates.into_iter().zip(answers) {
                assert_eq!(
                    answer,
                    if candidate == U256::from(expected) {
                        CheckResult::Sat
                    } else {
                        CheckResult::Unsat
                    }
                );
            }
        }
        let key = ExprId::fresh().unwrap();
        let slots = SlotStateId::zero()
            .store(key.clone(), c(7), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(slots, c(0), limits.expressions()).unwrap();
        for (key_value, expected) in [(0, 7), (1, 0)] {
            let mut path = RelationState::default();
            path.observe(
                &key,
                AbstractValue::constant(U256::from(key_value)).numeric(),
                &limits,
            )
            .unwrap();
            let candidates = [U256::ZERO, U256::from(7), U256::from(9)];
            let answers = batch(&path, &read, &candidates, &limits);
            assert_eq!(answers, oracle(&path, &read, &candidates, &limits));
            assert_eq!(
                candidates
                    .into_iter()
                    .zip(answers)
                    .filter_map(|(candidate, answer)| {
                        (answer == CheckResult::Sat).then_some(candidate)
                    })
                    .collect::<Vec<_>>(),
                [U256::from(expected)]
            );
        }
    }
}

#[test]
fn later_weaker_snapshot_and_separate_fresh_identity_do_not_reuse_candidate_answers() {
    let limits = policy(SmtProvider::Z3);
    let x = ExprId::fresh().unwrap();
    let y = ExprId::fresh().unwrap();
    let at = |value| {
        let mut state = RelationState::default();
        state
            .observe(
                &x,
                AbstractValue::constant(U256::from(value)).numeric(),
                &limits,
            )
            .unwrap();
        state
    };
    let strong = at(1u64);
    let weak = strong
        .join_guarded(&at(0), ExprId::fresh().unwrap(), &limits)
        .unwrap();
    let candidates = [U256::ZERO, U256::ONE];
    for (state, expression, expected) in [
        (&strong, &x, [CheckResult::Unsat, CheckResult::Sat]),
        (&weak, &x, [CheckResult::Sat, CheckResult::Sat]),
        (&strong, &y, [CheckResult::Sat, CheckResult::Sat]),
        (&strong, &x, [CheckResult::Unsat, CheckResult::Sat]),
    ] {
        let answers = batch(state, expression, &candidates, &limits);
        assert_eq!(answers, expected);
        assert_eq!(answers, oracle(state, expression, &candidates, &limits));
    }
}

#[test]
fn encoding_failure_consumes_its_prepaid_query_even_when_the_lease_is_reused() {
    let limits = RelationLimits {
        max_nodes: 1,
        ..policy(SmtProvider::Z3)
    };
    let target = op(opcode::XOR, &[ExprId::fresh().unwrap(), c(0)]);
    let state = RelationState::default();
    let mut ledger = WorkBudget::new(7);
    let mut lease = ledger.query_budget(7);
    assert_eq!(
        check(&state, &target, &[U256::ZERO], &limits, &mut lease),
        Err(CandidateError::Query(QueryReason::ExpressionLimit))
    );
    assert_eq!(
        check(&state, &target, &[U256::ZERO], &limits, &mut lease),
        Err(CandidateError::Work)
    );
    drop(lease);
    assert_eq!(ledger.used(), 7);
    assert!(ledger.exhausted());
}

#[test]
fn empty_and_bottom_states_preserve_original_query_accounting() {
    let limits = RelationLimits {
        max_nodes: 1,
        ..policy(SmtProvider::Z3)
    };
    let target = op(opcode::XOR, &[ExprId::fresh().unwrap(), c(0)]);
    let mut ledger = WorkBudget::new(0);
    assert_eq!(
        check(
            &RelationState::default(),
            &target,
            &[],
            &limits,
            &mut ledger.query_budget(7)
        ),
        Ok(Vec::new())
    );
    assert_eq!(ledger.used(), 0);
    assert!(!ledger.exhausted());
    let state = RelationState {
        bottom: true,
        ..RelationState::default()
    };
    let mut ledger = WorkBudget::new(14);
    assert_eq!(
        check(
            &state,
            &target,
            &[U256::ZERO, U256::ONE],
            &limits,
            &mut ledger.query_budget(7)
        ),
        Ok(vec![CheckResult::Unsat, CheckResult::Unsat])
    );
    assert_eq!(ledger.used(), 14);
    assert!(!ledger.exhausted());
}
