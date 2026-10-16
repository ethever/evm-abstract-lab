//! Mechanism-level soundness checks for local consequences and conservative ITEs.
use super::super::{
    CheckResult, Constraint, QueryReason, RelationLimits, RelationState, ValueQuery,
};
use crate::{
    domain::{
        NumericValue,
        symbolic::{ByteStateId, ByteView, ExprId, SlotStateId},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn op(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, RelationLimits::default().expressions()).unwrap()
}
fn includes(answer: &ValueQuery, expected: U256) -> bool {
    match answer {
        ValueQuery::Exact(value) => *value == expected,
        ValueQuery::Bounds(bounds) => bounds.contains(expected),
        ValueQuery::Undetermined(_) => true,
        ValueQuery::ProvenImpossible => false,
    }
}

#[test]
fn equality_chains_and_bijective_arithmetic_prove_exact_words() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let y = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    assert_eq!(state.add_equal(&x, &y, &limits), CheckResult::MayBeFeasible);
    assert_eq!(
        state.assume(
            &op(opcode::EQ, &[op(opcode::ADD, &[y, c(1)]), c(0)]),
            true,
            &limits
        ),
        CheckResult::MayBeFeasible
    );
    assert_eq!(state.local_value(&x, &limits), ValueQuery::Exact(U256::MAX));
    assert_eq!(
        state.can_equal(&x, U256::ZERO, &limits),
        CheckResult::ProvenImpossible
    );
}

#[test]
fn masks_exclude_impossible_bits_and_keep_every_unproved_high_bit() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    state.assume(
        &op(opcode::EQ, &[op(opcode::AND, &[x.clone(), c(255)]), c(7)]),
        true,
        &limits,
    );
    assert_eq!(
        state.can_equal(&x, U256::from(8), &limits),
        CheckResult::ProvenImpossible
    );
    for value in [
        U256::from(7),
        (U256::MAX & !U256::from(255)) | U256::from(7),
    ] {
        assert!(includes(&state.local_value(&x, &limits), value));
    }
}

#[test]
fn symbolic_slot_alias_and_guarded_history_keep_the_original_key_identity() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let y = ExprId::fresh().unwrap();
    let slots = SlotStateId::zero()
        .store(x.clone(), c(7), limits.expressions().into())
        .unwrap()
        .store(y.clone(), c(9), limits.expressions().into())
        .unwrap();
    let read = ExprId::slot_read(slots, x.clone(), limits.expressions()).unwrap();
    let equal = op(opcode::EQ, &[x, y]);
    for (alias, expected) in [(true, 9u64), (false, 7)] {
        let mut state = RelationState::default();
        state.assume(&equal, alias, &limits);
        assert_eq!(
            state.local_value(&read, &limits),
            ValueQuery::Exact(U256::from(expected))
        );
    }
}

#[test]
fn unknown_selector_never_leaks_an_alternatives_facts() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let selector = ExprId::fresh().unwrap();
    let mut yes = RelationState::default();
    let mut no = RelationState::default();
    yes.add_equal(&x, &c(1), &limits);
    no.add_equal(&x, &c(2), &limits);
    let joined = yes.join_guarded(&no, selector.clone(), &limits).unwrap();
    for candidate in [1u64, 2] {
        assert!(includes(
            &joined.local_value(&x, &limits),
            U256::from(candidate)
        ));
    }
    let mut selected = joined;
    selected.assume(&selector, true, &limits);
    assert_eq!(
        selected.local_value(&x, &limits),
        ValueQuery::Exact(U256::ONE)
    );
}

#[test]
fn local_infeasibility_is_distinct_from_an_interrupted_retained_assumption() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let mut impossible = RelationState::default();
    impossible.assume(&x, true, &limits);
    assert_eq!(
        impossible.assume(&x, false, &limits),
        CheckResult::ProvenImpossible
    );
    let token = analysis_control::Cancellation::new();
    token.cancel();
    let mut pending = RelationState::default();
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            pending.assume_checked(&x, false, &limits),
            Ok(CheckResult::Undetermined(QueryReason::Cancelled))
        )
    });
    assert!(!pending.is_bottom());
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending.local_value(&x, &limits),
        ValueQuery::Exact(U256::ZERO)
    );
}

#[test]
fn independent_unknowns_and_hashes_never_claim_satisfiability_or_distinct_models() {
    let limits = RelationLimits::default();
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    for truth in [true, false] {
        let mut state = RelationState::default();
        assert_eq!(
            state.assume(&op(opcode::EQ, &[a.clone(), b.clone()]), truth, &limits),
            CheckResult::MayBeFeasible
        );
    }
    assert!(matches!(
        RelationState::default().local_value(&a, &limits),
        ValueQuery::Bounds(_)
    ));
}

#[test]
fn demanded_word_reads_do_not_wrap_mathematical_addresses() {
    let limits = RelationLimits::default();
    let at = ExprId::fresh().unwrap();
    let state = ByteStateId::zero()
        .write_word(
            at.clone(),
            ExprId::constant(U256::MAX),
            limits.expressions().into(),
        )
        .unwrap();
    let read = ExprId::word_read(
        ByteView::new(state, at.clone(), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut relations = RelationState::default();
    relations.add_equal(&at, &ExprId::constant(U256::MAX), &limits);
    assert_eq!(
        relations.local_value(&read, &limits),
        ValueQuery::Exact(U256::from(255) << 248)
    );
}

#[test]
fn root_ledger_refusal_cannot_be_returned_as_a_local_proof() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    state
        .observe(&x, &NumericValue::constant(U256::ONE), &limits)
        .unwrap();
    let before = state.clone();
    let mut budget = WorkBudget::new(0);
    assert!(matches!(
        state.local_value_budgeted(&x, &limits, &mut budget),
        ValueQuery::Undetermined(_)
    ));
    assert!(budget.exhausted());
    assert_eq!(budget.used(), 0);
    assert_eq!(state, before);
}

#[test]
fn retained_inconsistent_bits_are_a_real_local_contradiction() {
    let x = ExprId::fresh().unwrap();
    let state = RelationState {
        constraints: [Constraint::Bits {
            expression: x,
            zero: U256::ONE,
            one: U256::ONE,
        }]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    assert_eq!(
        state.check(&RelationLimits::default()),
        CheckResult::ProvenImpossible
    );
}
