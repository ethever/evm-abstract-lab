//! Mechanism-level soundness checks for local consequences and conservative ITEs.
use super::super::{
    CheckResult, Constraint, QueryReason, RelationLimits, RelationState, ValueQuery,
};
use crate::{
    domain::{
        NumericValue,
        symbolic::{ByteStateId, ByteView, ExprId, SlotStateId, SourceId},
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
fn observed_zero_comparison_retains_the_target_exclusion() {
    let limits = RelationLimits::default();
    let target = ExprId::fresh().unwrap();
    let comparison = op(opcode::EQ, &[target.clone(), c(7)]);
    let mut state = RelationState::default();
    state
        .observe(&comparison, &NumericValue::constant(U256::ZERO), &limits)
        .unwrap();
    assert_eq!(
        state.local_value(&comparison, &limits),
        ValueQuery::Exact(U256::ZERO)
    );
    assert_eq!(
        state.can_equal(&target, U256::from(7), &limits),
        CheckResult::ProvenImpossible
    );
    assert_eq!(
        state.can_equal(&target, U256::from(8), &limits),
        CheckResult::MayBeFeasible
    );
}

#[test]
fn nonzero_symbolic_storage_key_cannot_alias_zero() {
    let limits = RelationLimits::default();
    let key = ExprId::fresh().unwrap();
    let slots = SlotStateId::zero()
        .store(c(0), c(7), limits.expressions().into())
        .unwrap()
        .store(key.clone(), c(9), limits.expressions().into())
        .unwrap();
    let read = ExprId::slot_read(slots, c(0), limits.expressions()).unwrap();
    let guard = op(opcode::ISZERO, &[key]);
    for (zero, expected) in [(false, 7), (true, 9)] {
        let mut state = RelationState::default();
        assert_eq!(
            state.assume(&guard, zero, &limits),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            state.local_value(&read, &limits),
            ValueQuery::Exact(U256::from(expected))
        );
    }
}

#[test]
fn copied_long_view_reuses_the_original_full_word_fact() {
    let limits = RelationLimits::default();
    let length = ExprId::fresh().unwrap();
    let source = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
    let original = ExprId::word_read(
        ByteView::new(source.clone(), c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let copied = ByteStateId::zero()
        .copy(
            c(128),
            ByteView::new(source, c(0), length.clone()),
            length.clone(),
            limits.expressions().into(),
        )
        .unwrap();
    let read = ExprId::word_read(
        ByteView::new(copied, c(128), length.clone()),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.add_unsigned_bounds(&length, U256::from(32), U256::from(64), &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.add_equal(&original, &c(7), &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.local_value(&read, &limits),
        ValueQuery::Exact(U256::from(7))
    );
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
fn optional_local_projection_refusal_cannot_be_returned_as_a_local_proof() {
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
    assert!(!budget.exhausted());
    assert_eq!(budget.used(), 0);
    assert!(budget.can_afford(0));
    assert_eq!(state, before);
}

#[test]
fn rejected_local_step_preserves_paid_work_without_charging_the_rejected_step() {
    let mut budget = WorkBudget::new(100);
    {
        let mut work = super::Work {
            remaining: 3,
            cancellation: analysis_control::current_cancellation(),
            ledger: Some(&mut budget),
        };
        assert!(work.charge(2).is_ok());
        assert_eq!(work.remaining, 1);
        assert_eq!(work.ledger.as_ref().unwrap().used(), 2);
        assert!(matches!(
            work.charge(2),
            Err(super::Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert_eq!(work.remaining, 1);
        assert_eq!(work.ledger.as_ref().unwrap().used(), 2);
        assert!(!work.ledger.as_ref().unwrap().exhausted());
        assert!(work.charge(1).is_ok());
        assert_eq!(work.remaining, 0);
    }
    assert_eq!(budget.used(), 3);
    assert!(!budget.exhausted());
}

#[test]
fn optional_reservation_refusal_keeps_paid_work_and_root_capacity() {
    let mut budget = WorkBudget::new(2);
    {
        let mut work = super::Work {
            remaining: 3,
            cancellation: analysis_control::current_cancellation(),
            ledger: Some(&mut budget),
        };
        assert!(work.charge(1).is_ok());
        assert!(matches!(
            work.charge(2),
            Err(super::Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert_eq!(work.remaining, 2);
        assert_eq!(work.ledger.as_ref().unwrap().used(), 1);
        assert!(!work.ledger.as_ref().unwrap().exhausted());
        assert!(work.charge(1).is_ok());
    }
    assert_eq!(budget.used(), 2);
    assert!(!budget.exhausted());
}

#[test]
fn key_lookup_pays_pointer_checks_separately_from_structural_comparisons() {
    let mut prefix = ExprId::fresh().unwrap();
    for _ in 0..16 {
        prefix = op(opcode::ADD, &[prefix, c(1)]);
    }
    let key = op(opcode::ADD, &[prefix.clone(), c(1)]);
    let equivalent = op(opcode::ADD, &[prefix, c(1)]);
    assert_ne!(key.immutable_node_key(), equivalent.immutable_node_key());
    assert_eq!(key, equivalent);
    let mut keys = super::keys::KeyMap::default();
    let mut work = super::Work {
        remaining: 1024,
        cancellation: analysis_control::current_cancellation(),
        ledger: None,
    };
    assert!(keys.insert(key.clone(), 7, &mut work).is_ok());
    for (query, expected) in [(&key, Some(&7)), (&equivalent, None)] {
        let mut budget = WorkBudget::new(2);
        let answer = keys.get(
            query,
            &mut super::Work {
                remaining: 1024,
                cancellation: analysis_control::current_cancellation(),
                ledger: Some(&mut budget),
            },
        );
        if let Some(value) = expected {
            assert_eq!(answer.unwrap(), Some(value));
            assert_eq!(budget.used(), 2);
        } else {
            assert!(matches!(
                answer,
                Err(super::Stop::Pending(QueryReason::ExpressionLimit))
            ));
            assert_eq!(
                budget.used(),
                1,
                "only the completed integer lookup is paid"
            );
        }
        assert!(!budget.exhausted());
    }
    assert_eq!(keys.get(&equivalent, &mut work).unwrap(), Some(&7));
    assert!(keys.insert(equivalent.clone(), 9, &mut work).is_ok());
    assert_eq!(keys.get(&key, &mut work).unwrap(), Some(&9));
    assert!(keys.remove(&equivalent, &mut work).is_ok());
    assert_eq!(keys.get(&key, &mut work).unwrap(), None);
}

#[test]
fn exact_compound_guarantee_does_not_reconstruct_the_unknown_word() {
    let limits = RelationLimits {
        max_nodes: 128,
        ..RelationLimits::default()
    };
    let source = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::fresh().unwrap());
    let read = ExprId::word_read(
        ByteView::new(source, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    state
        .observe(&read, &NumericValue::constant(U256::from(7)), &limits)
        .unwrap();
    assert_eq!(
        state.local_value(&read, &limits),
        ValueQuery::Exact(U256::from(7))
    );
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
