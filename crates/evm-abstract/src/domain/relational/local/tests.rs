//! Mechanism-level soundness checks for local consequences and conservative ITEs.
use super::super::{
    CheckResult, Constraint, QueryReason, RelationLimits, RelationState, ValueQuery,
};
use crate::{
    domain::{
        NumericValue,
        symbolic::{ByteStateId, ByteView, ExprId, SlotStateId, SourceId, StateBinding},
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
fn finite_exclusion_preserves_ite_identity_and_every_unexcluded_candidate() {
    let limits = RelationLimits::default();
    let selector = ExprId::fresh().unwrap();
    let value = ExprId::ite(selector.clone(), c(128), c(192), limits.expressions()).unwrap();
    for (excluded, expected) in [(192, Some(128u64)), (128, Some(192u64)), (7, None)] {
        let mut state = RelationState::default();
        state
            .observe(
                &value,
                &NumericValue::unsigned_range(U256::from(128), U256::from(192)).unwrap(),
                &limits,
            )
            .unwrap();
        assert_eq!(
            state.assume(
                &op(opcode::EQ, &[value.clone(), c(excluded)]),
                false,
                &limits
            ),
            CheckResult::MayBeFeasible
        );
        let before = state.clone();
        let answer = state.local_value(&value, &limits);
        if let Some(expected) = expected {
            assert_eq!(answer, ValueQuery::Exact(U256::from(expected)));
            let mut budget = WorkBudget::new(20_000_000);
            assert_eq!(
                state.local_value_budgeted(&value, &limits, &mut budget),
                answer
            );
            assert!(!budget.exhausted());
        } else {
            for candidate in [128, 192] {
                assert!(includes(&answer, U256::from(candidate)));
            }
        }
        for candidate in [0, 1] {
            assert!(includes(
                &state.local_value(&selector, &limits),
                U256::from(candidate)
            ));
        }
        assert_eq!(state, before);
    }
}

#[test]
fn retained_ite_bounds_select_disjoint_branches_and_keep_paired_roots_and_guarantees() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    for (lower, upper, nonzero) in [(7u64, 9u64, false), (512, 514, true)] {
        let selector = ExprId::fresh().unwrap();
        let chosen = ExprId::fresh().unwrap();
        let excluded = if nonzero {
            op(opcode::AND, &[ExprId::fresh().unwrap(), c(255)])
        } else {
            c(512)
        };
        let (yes, no) = if nonzero {
            (chosen.clone(), excluded)
        } else {
            (excluded, chosen.clone())
        };
        let selected = ExprId::ite(selector.clone(), yes, no, limits.expressions()).unwrap();
        let predicate = ExprId::fresh().unwrap();
        let mut yes = RelationState::default();
        let mut no = RelationState::default();
        assert_eq!(
            yes.assume(&predicate, true, &limits),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            no.assume(&predicate, false, &limits),
            CheckResult::MayBeFeasible
        );
        let mut state = yes.join_guarded(&no, selector.clone(), &limits).unwrap();
        state
            .observe(
                &selected,
                &NumericValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap(),
                &limits,
            )
            .unwrap();
        let before = state.clone();
        let answer = state.local_value(&chosen, &limits);
        let ValueQuery::Bounds(bounds) = answer else {
            panic!("chosen branch retains the result range: {answer:?}");
        };
        assert_eq!(
            bounds.interval().unsigned_bounds(),
            (U256::from(lower), U256::from(upper))
        );
        assert_eq!(
            state.can_equal(&selector, U256::ZERO, &limits),
            if nonzero {
                CheckResult::ProvenImpossible
            } else {
                CheckResult::MayBeFeasible
            }
        );
        assert_eq!(
            state.can_equal(&selector, U256::ONE, &limits),
            if nonzero {
                CheckResult::MayBeFeasible
            } else {
                CheckResult::ProvenImpossible
            }
        );
        assert_eq!(
            state.can_equal(&predicate, U256::ZERO, &limits),
            if nonzero {
                CheckResult::ProvenImpossible
            } else {
                CheckResult::MayBeFeasible
            }
        );
        let paired = ExprId::ite(selector.clone(), c(11), c(22), limits.expressions()).unwrap();
        let mut budget = WorkBudget::new(20_000_000);
        assert_eq!(
            state.local_value_budgeted(&paired, &limits, &mut budget),
            ValueQuery::Exact(U256::from(if nonzero { 11 } else { 22 }))
        );
        assert!(budget.used() > 0);
        assert!(!budget.exhausted());
        let yes = SlotStateId::zero().store(c(0), c(11), states).unwrap();
        let no = SlotStateId::zero().store(c(0), c(22), states).unwrap();
        let slots = yes.join_guarded(&no, selector.clone(), states).unwrap();
        let read = ExprId::slot_read(slots, c(0), limits.expressions()).unwrap();
        assert_eq!(
            state.local_value(&read, &limits),
            ValueQuery::Exact(U256::from(if nonzero { 11 } else { 22 }))
        );
        let yes = ByteStateId::zero().write_word(c(0), c(7), states).unwrap();
        let no = ByteStateId::zero().write_word(c(0), c(9), states).unwrap();
        let bytes = yes.join_guarded(&no, selector, states).unwrap();
        let read = ExprId::word_read(
            ByteView::new(bytes, c(0), c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        assert_eq!(
            state.local_value(&read, &limits),
            ValueQuery::Exact(U256::from(if nonzero { 7 } else { 9 }))
        );
        assert_eq!(state, before);
    }
}

#[test]
fn nonzero_ite_result_excludes_exact_zero_without_booleanizing_the_selector() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let selector = ExprId::fresh().unwrap();
    let chosen = ExprId::fresh().unwrap();
    let selected =
        ExprId::ite(selector.clone(), chosen.clone(), c(0), limits.expressions()).unwrap();
    let nonzero = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    assert_eq!(
        nonzero.interval().unsigned_bounds(),
        (U256::ZERO, U256::MAX)
    );
    let mut unknown = RelationState::default();
    assert_eq!(
        unknown.assume(&chosen, true, &limits),
        CheckResult::MayBeFeasible
    );
    let mut state = unknown.clone();
    state.observe(&selected, &nonzero, &limits).unwrap();
    let before = state.clone();
    for expression in [&selector, &chosen] {
        let answer = state.local_value(expression, &limits);
        let ValueQuery::Bounds(bounds) = answer else {
            panic!("nonzero words retain their full range: {answer:?}");
        };
        assert!(!bounds.contains(U256::ZERO));
        for candidate in [U256::ONE, U256::from(2), U256::MAX] {
            assert!(bounds.contains(candidate));
        }
    }
    let yes = ByteStateId::zero().write_word(c(0), c(7), states).unwrap();
    let no = ByteStateId::zero().write_word(c(0), c(9), states).unwrap();
    let bytes = yes.join_guarded(&no, selector.clone(), states).unwrap();
    let read = ExprId::word_read(
        ByteView::new(bytes, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    assert_eq!(
        state.local_value(&read, &limits),
        ValueQuery::Exact(U256::from(7))
    );
    let ValueQuery::Bounds(bounds) = unknown.local_value(&selector, &limits) else {
        panic!("an unknown result leaves the selector unknown");
    };
    assert!(bounds.contains(U256::ZERO));
    assert!(bounds.contains(U256::ONE));
    for (truth, expected) in [(true, 7), (false, 9)] {
        let mut direct = unknown.clone();
        assert_eq!(
            direct.assume(&selector, truth, &limits),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            direct.local_value(&read, &limits),
            ValueQuery::Exact(U256::from(expected))
        );
    }
    assert_eq!(state, before);
}

#[test]
fn overlapping_ite_branch_bounds_do_not_select_or_narrow_either_branch() {
    let limits = RelationLimits::default();
    let selector = ExprId::fresh().unwrap();
    let yes = op(opcode::AND, &[ExprId::fresh().unwrap(), c(255)]);
    let no = op(opcode::AND, &[ExprId::fresh().unwrap(), c(511)]);
    let selected = ExprId::ite(
        selector.clone(),
        yes.clone(),
        no.clone(),
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    state
        .observe(
            &selected,
            &NumericValue::unsigned_range(U256::from(7), U256::from(9)).unwrap(),
            &limits,
        )
        .unwrap();
    let before = state.clone();
    let answer = state.local_value(&selector, &limits);
    let ValueQuery::Bounds(bounds) = answer else {
        panic!("overlapping upper bounds leave the selector unknown: {answer:?}");
    };
    assert!(bounds.contains(U256::ZERO));
    assert!(bounds.contains(U256::ONE));
    for (branch, maximum) in [(yes, 255), (no, 511)] {
        let answer = state.local_value(&branch, &limits);
        let ValueQuery::Bounds(bounds) = answer else {
            panic!("neither branch inherits the result without a determined selector: {answer:?}");
        };
        assert!(bounds.contains(U256::ZERO));
        assert!(bounds.contains(U256::from(maximum)));
    }
    assert_eq!(
        state.can_equal(&selected, U256::from(7), &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.can_equal(&selected, U256::from(10), &limits),
        CheckResult::ProvenImpossible
    );
    assert_eq!(state, before);
}

#[test]
fn determined_copy_lengths_preserve_hash_identity_without_fixing_unknown_selectors() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let length = ExprId::fresh().unwrap();
    let selector = ExprId::fresh().unwrap();
    let source = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
    let selected = ByteStateId::zero()
        .join_guarded(&source, selector.clone(), states)
        .unwrap();
    let selected_length =
        ExprId::ite(selector.clone(), c(0), length.clone(), limits.expressions()).unwrap();
    let copied = ByteStateId::zero()
        .copy(
            c(1024),
            ByteView::new(selected, c(0), selected_length.clone()),
            selected_length.clone(),
            states,
        )
        .unwrap();
    let copied_hash = ExprId::keccak(
        ByteView::new(copied, c(1024), selected_length),
        limits.expressions(),
    )
    .unwrap();
    let source_hash = ExprId::keccak(
        ByteView::new(source, c(0), length.clone()),
        limits.expressions(),
    )
    .unwrap();
    let equality = op(opcode::EQ, &[copied_hash.clone(), source_hash]);
    let mut state = RelationState::default();
    assert_eq!(
        state.add_unsigned_bounds(&length, U256::from(32), U256::from(512), &limits),
        CheckResult::MayBeFeasible
    );
    let unknown = state.local_value(&equality, &limits);
    assert!(matches!(unknown, ValueQuery::Bounds(_)));
    assert!(includes(&unknown, U256::ZERO));
    assert!(includes(&unknown, U256::ONE));
    let mut empty = state.clone();
    assert_eq!(
        empty.assume(&selector, true, &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        empty.local_value(&copied_hash, &limits),
        ValueQuery::Exact(U256::from_be_bytes(alloy_primitives::keccak256([]).0))
    );
    assert_eq!(
        state.assume(&selector, false, &limits),
        CheckResult::MayBeFeasible
    );
    let before = state.clone();
    assert_eq!(
        state.local_value(&equality, &limits),
        ValueQuery::Exact(U256::ONE)
    );
    assert_eq!(state, before);
}

#[test]
fn initial_source_observations_keep_namespace_length_writes_and_guards() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let token = ExprId::fresh().unwrap();
    let source = SourceId::scoped(token.clone(), U256::ONE);
    let initial = ByteStateId::input(source.clone(), c(2));
    let observed = ByteStateId::observed(
        source.clone(),
        c(2),
        [0xaau8, 0xbbu8]
            .into_iter()
            .enumerate()
            .map(|(index, byte)| StateBinding {
                index: U256::from(index),
                value: ExprId::fresh().unwrap(),
                numeric: NumericValue::constant(U256::from(byte)),
            })
            .collect(),
        states,
    )
    .unwrap();
    let hash = |state, length| {
        ExprId::keccak(ByteView::new(state, c(0), c(length)), limits.expressions()).unwrap()
    };
    let digest = U256::from_be_bytes(alloy_primitives::keccak256([0xaa, 0xbb]).0);
    let mut state = RelationState::default();
    assert_eq!(
        state.add_equal(
            &hash(observed.clone(), 2),
            &ExprId::constant(digest),
            &limits
        ),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.local_value(&hash(initial.clone(), 2), &limits),
        ValueQuery::Exact(digest)
    );
    for unrelated in [
        ByteStateId::input(SourceId::scoped(token, U256::from(2)), c(2)),
        ByteStateId::input(source, c(1)),
    ] {
        assert!(matches!(
            state.local_value(&hash(unrelated, 2), &limits),
            ValueQuery::Bounds(_)
        ));
    }
    assert_eq!(
        state.local_value(&hash(initial.clone(), 3), &limits),
        ValueQuery::Exact(U256::from_be_bytes(
            alloy_primitives::keccak256([0xaa, 0xbb, 0]).0
        ))
    );
    let written = initial.clone().write_byte(c(0), c(0x11), states).unwrap();
    assert_eq!(
        state.local_value(&hash(written, 2), &limits),
        ValueQuery::Exact(U256::from_be_bytes(
            alloy_primitives::keccak256([0x11, 0xbb]).0
        ))
    );
    let selector = ExprId::fresh().unwrap();
    let alternative = ByteStateId::input(SourceId::fresh().unwrap(), c(2));
    let guarded = ExprId::ite(
        selector.clone(),
        hash(observed, 2),
        hash(alternative, 2),
        limits.expressions(),
    )
    .unwrap();
    let mut guarded_state = RelationState::default();
    assert_eq!(
        guarded_state.add_equal(&guarded, &ExprId::constant(digest), &limits),
        CheckResult::MayBeFeasible
    );
    let old_hash = hash(initial, 2);
    assert!(matches!(
        guarded_state.local_value(&old_hash, &limits),
        ValueQuery::Bounds(_)
    ));
    let mut unselected = guarded_state.clone();
    assert_eq!(
        unselected.assume(&selector, false, &limits),
        CheckResult::MayBeFeasible
    );
    assert!(matches!(
        unselected.local_value(&old_hash, &limits),
        ValueQuery::Bounds(_)
    ));
    assert_eq!(
        guarded_state.assume(&selector, true, &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        guarded_state.local_value(&old_hash, &limits),
        ValueQuery::Exact(digest)
    );
}

#[test]
fn concrete_slot_alias_reuses_guarded_history_without_choosing_its_selector() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let selector = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let left = SlotStateId::zero().store(c(0), c(7), states).unwrap();
    let right = SlotStateId::zero().store(c(0), c(9), states).unwrap();
    let history = left.join_guarded(&right, selector.clone(), states).unwrap();
    let old = ExprId::slot_read(history.clone(), c(0), limits.expressions()).unwrap();
    let updated = history.store(key.clone(), c(13), states).unwrap();
    let latest = ExprId::slot_read(updated, c(0), limits.expressions()).unwrap();
    for alias in [0, 1] {
        let mut state = RelationState::default();
        assert_eq!(
            state.add_equal(&key, &c(alias), &limits),
            CheckResult::MayBeFeasible
        );
        let expected = if alias == 0 { c(13) } else { old.clone() };
        assert_eq!(
            state.local_value(&op(opcode::EQ, &[latest.clone(), expected]), &limits),
            ValueQuery::Exact(U256::ONE)
        );
        for candidate in [U256::ZERO, U256::ONE] {
            assert!(includes(&state.local_value(&selector, &limits), candidate));
        }
    }
    let answer = RelationState::default().local_value(&op(opcode::EQ, &[latest, old]), &limits);
    assert!(matches!(answer, ValueQuery::Bounds(_)));
    assert!(includes(&answer, U256::ZERO));
    assert!(includes(&answer, U256::ONE));
}

#[test]
fn shared_byte_overlay_preserves_the_original_word_selection() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let selector = ExprId::fresh().unwrap();
    let left = ByteStateId::zero()
        .write_word(c(0), c(0x1107), states)
        .unwrap();
    let right = ByteStateId::zero()
        .write_word(c(0), c(0x2209), states)
        .unwrap();
    let history = left.join_guarded(&right, selector.clone(), states).unwrap();
    let overlaid = history.write_byte(c(31), c(0xaa), states).unwrap();
    let word = ExprId::word_read(
        ByteView::new(overlaid, c(0), c(64)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    for (guard, exact) in [(selector.clone(), true), (ExprId::fresh().unwrap(), false)] {
        let expected = ExprId::ite(guard, c(0x11aa), c(0x22aa), limits.expressions()).unwrap();
        let answer = RelationState::default()
            .local_value(&op(opcode::EQ, &[word.clone(), expected]), &limits);
        if exact {
            assert_eq!(answer, ValueQuery::Exact(U256::ONE));
        } else {
            assert!(matches!(answer, ValueQuery::Bounds(_)));
            assert!(includes(&answer, U256::ZERO));
            assert!(includes(&answer, U256::ONE));
        }
    }
    for (branch, expected) in [(true, 0x11aau64), (false, 0x22aau64)] {
        let mut state = RelationState::default();
        assert_eq!(
            state.assume(&selector, branch, &limits),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            state.local_value(&word, &limits),
            ValueQuery::Exact(U256::from(expected))
        );
    }
}

#[test]
fn known_source_word_registration_leaves_byte_facts_unmaterialized() {
    let limits = RelationLimits::default();
    let initial = ByteStateId::input(SourceId::fresh().unwrap(), c(64));
    let read = ExprId::word_read(
        ByteView::new(initial.clone(), c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let byte = ExprId::byte_read(initial, c(31), limits.expressions()).unwrap();
    let mut work = super::Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::current_cancellation(),
        ledger: None,
    };
    let mut engine = super::Engine::new(&mut work, &limits, crate::domain::Domain::default());
    engine
        .atom(&Constraint::Equal {
            left: read.clone(),
            right: c(7),
        })
        .unwrap();
    engine.source_facts(&read).unwrap();
    assert_eq!(engine.source_words.len(), 1);
    assert!(!engine.has_source_bytes);
    assert!(engine.known(&byte).unwrap().is_top());
    assert_eq!(engine.eval(&read).unwrap().singleton(), Some(U256::from(7)));
    let before_demand = engine.work.remaining;
    assert_eq!(engine.eval(&byte).unwrap().singleton(), Some(U256::from(7)));
    assert!(engine.work.remaining < before_demand);
    assert!(engine.known(&byte).unwrap().is_top());
}

#[test]
fn demanded_source_word_bytes_preserve_identity_guards_and_nonwrapping_windows() {
    let limits = RelationLimits::default();
    let token = ExprId::fresh().unwrap();
    let source = SourceId::scoped(token.clone(), U256::ONE);
    let initial = ByteStateId::input(source.clone(), c(64));
    let word = |state, start| {
        ExprId::word_read(
            ByteView::new(state, start, c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap()
    };
    let byte = |state, index| ExprId::byte_read(state, index, limits.expressions()).unwrap();
    let original = word(initial.clone(), c(0));
    let mut state = RelationState::default();
    state
        .observe(&original, &NumericValue::constant(U256::from(7)), &limits)
        .unwrap();
    assert_eq!(
        state.local_value(&byte(initial.clone(), c(31)), &limits),
        ValueQuery::Exact(U256::from(7))
    );
    for other in [
        ByteStateId::input(SourceId::scoped(token, U256::from(2)), c(64)),
        ByteStateId::input(source.clone(), c(63)),
        ByteStateId::input(SourceId::fresh().unwrap(), c(64)),
    ] {
        let answer = state.local_value(&byte(other, c(31)), &limits);
        let ValueQuery::Bounds(bounds) = answer else {
            panic!("a different source or length has no word guarantee: {answer:?}");
        };
        assert!(bounds.contains(U256::from(255)));
    }

    let selector = ExprId::fresh().unwrap();
    let selected = ExprId::ite(selector, original, c(7), limits.expressions()).unwrap();
    let mut guarded = RelationState::default();
    guarded
        .observe(&selected, &NumericValue::constant(U256::from(7)), &limits)
        .unwrap();
    let answer = guarded.local_value(&byte(initial, c(31)), &limits);
    assert!(matches!(answer, ValueQuery::Bounds(bounds) if bounds.contains(U256::from(255))));

    let initial = ByteStateId::input(source, ExprId::constant(U256::MAX));
    let edge = word(initial.clone(), ExprId::constant(U256::MAX));
    let mut edge_state = RelationState::default();
    edge_state
        .observe(&edge, &NumericValue::constant(U256::ZERO), &limits)
        .unwrap();
    let answer = edge_state.local_value(&byte(initial, c(0)), &limits);
    assert!(matches!(answer, ValueQuery::Bounds(bounds) if bounds.contains(U256::from(255))));
}

#[test]
fn overlapping_source_words_reuse_only_their_guaranteed_initial_bytes() {
    let limits = RelationLimits::default();
    let states = limits.expressions().into();
    let length = ExprId::fresh().unwrap();
    let source = SourceId::fresh().unwrap();
    let initial = ByteStateId::input(source, length.clone());
    let original = ExprId::word_read(
        ByteView::new(initial.clone(), c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let copied = ByteStateId::zero()
        .copy(
            c(128),
            ByteView::new(initial.clone(), c(0), length.clone()),
            length.clone(),
            states,
        )
        .unwrap();
    let shifted = ExprId::word_read(
        ByteView::new(copied, c(129), length.clone()),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.add_unsigned_bounds(&length, U256::from(33), U256::from(128), &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.add_equal(&original, &c(7), &limits),
        CheckResult::MayBeFeasible
    );
    let before = state.clone();
    let answer = state.local_value(&shifted, &limits);
    let ValueQuery::Bounds(bounds) = answer else {
        panic!("shifted byte remains unknown: {answer:?}");
    };
    assert_eq!(
        bounds.interval().unsigned_bounds(),
        (U256::from(0x700), U256::from(0x7ff))
    );
    assert_eq!(
        state.can_equal(&shifted, U256::from(7), &limits),
        CheckResult::ProvenImpossible
    );
    assert_eq!(
        state.can_equal(&shifted, U256::from(0x7aa), &limits),
        CheckResult::MayBeFeasible
    );
    let independent = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
    let independent = ExprId::word_read(
        ByteView::new(independent, c(1), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    assert!(includes(
        &state.local_value(&independent, &limits),
        U256::from(7)
    ));
    assert_eq!(state, before);

    let short = ExprId::word_read(
        ByteView::new(initial.clone(), c(0), c(31)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let last = ExprId::byte_read(initial.clone(), c(31), limits.expressions()).unwrap();
    let mut short_state = RelationState::default();
    assert_eq!(
        short_state.add_equal(&short, &c(0), &limits),
        CheckResult::MayBeFeasible
    );
    let answer = short_state.local_value(&last, &limits);
    assert!(matches!(answer, ValueQuery::Bounds(_)));
    assert!(includes(&answer, U256::from(255)));
    let written = initial.write_word(c(0), c(7), states).unwrap();
    let written = ExprId::word_read(
        ByteView::new(written, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut written_state = RelationState::default();
    assert_eq!(
        written_state.add_equal(&written, &c(7), &limits),
        CheckResult::MayBeFeasible
    );
    assert!(includes(
        &written_state.local_value(&original, &limits),
        U256::from(42)
    ));
}

#[test]
fn candidate_equality_cannot_supply_its_own_compound_value() {
    let limits = RelationLimits::default();
    let base = ExprId::fresh().unwrap();
    let square = op(opcode::MUL, &[base.clone(), base.clone()]);
    let slots = SlotStateId::zero()
        .store(c(0), c(9), limits.expressions().into())
        .unwrap();
    let read = ExprId::slot_read(slots, c(0), limits.expressions()).unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.add_equal(&base, &c(3), &limits),
        CheckResult::MayBeFeasible
    );
    let before = state.clone();
    for expression in [square, read] {
        assert_eq!(
            state.local_value(&expression, &limits),
            ValueQuery::Exact(U256::from(9))
        );
        for (candidate, expected) in [
            (8, CheckResult::ProvenImpossible),
            (9, CheckResult::MayBeFeasible),
        ] {
            assert_eq!(
                state.can_equal(&expression, U256::from(candidate), &limits),
                expected
            );
            let mut budget = WorkBudget::new(20_000_000);
            assert_eq!(
                state.can_equal_budgeted(&expression, U256::from(candidate), &limits, &mut budget),
                expected
            );
            assert!(!budget.exhausted());
        }
    }
    assert_eq!(state, before);
}

#[test]
fn unknown_candidate_queries_keep_typed_interruption_without_a_witness() {
    let limits = RelationLimits::default();
    let expression = op(opcode::MUL, &[ExprId::fresh().unwrap(), c(3)]);
    let state = RelationState::default();
    assert_eq!(
        state.can_equal(&expression, U256::from(7), &limits),
        CheckResult::MayBeFeasible
    );
    let mut budget = WorkBudget::new(0);
    assert_eq!(
        state.can_equal_budgeted(&expression, U256::from(7), &limits, &mut budget),
        CheckResult::Undetermined(QueryReason::ExpressionLimit)
    );
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
    let token = analysis_control::Cancellation::new();
    token.cancel();
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            state.can_equal(&expression, U256::from(7), &limits),
            CheckResult::Undetermined(QueryReason::Cancelled)
        );
    });
}

#[test]
fn assumptions_check_definitions_instead_of_their_own_imported_truth() {
    let limits = RelationLimits::default();
    let base = ExprId::fresh().unwrap();
    let square = op(opcode::EXP, &[base.clone(), c(2)]);
    let product = op(opcode::MUL, &[base.clone(), base.clone()]);
    let slots = SlotStateId::zero()
        .store(c(0), c(9), limits.expressions().into())
        .unwrap();
    let read = ExprId::slot_read(slots, c(0), limits.expressions()).unwrap();
    let bytes = ByteStateId::exact(std::sync::Arc::from([0u8; 32]));
    let word = ExprId::word_read(
        ByteView::new(bytes, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    for (condition, truth) in [
        (op(opcode::EQ, &[square.clone(), product]), false),
        (op(opcode::EQ, &[square, c(10)]), true),
        (op(opcode::EQ, &[read, c(10)]), true),
        (op(opcode::EQ, &[word.clone(), c(1)]), true),
        (word, true),
    ] {
        let mut state = RelationState::default();
        assert_eq!(
            state.add_equal(&base, &c(3), &limits),
            CheckResult::MayBeFeasible
        );
        let mut budgeted = state.clone();
        let mut budget = WorkBudget::new(20_000_000);
        assert_eq!(
            state.assume_checked(&condition, truth, &limits),
            Ok(CheckResult::ProvenImpossible)
        );
        assert_eq!(
            budgeted.assume_checked_budgeted(&condition, truth, &limits, &mut budget),
            Ok(CheckResult::ProvenImpossible)
        );
        assert!(!budget.exhausted());
    }
}

#[test]
fn check_rejects_an_observed_compound_result_only_from_its_definition() {
    let limits = RelationLimits::default();
    let base = ExprId::fresh().unwrap();
    let product = op(opcode::MUL, &[base.clone(), base.clone()]);
    for (result, expected) in [
        (9, CheckResult::MayBeFeasible),
        (10, CheckResult::ProvenImpossible),
    ] {
        let mut state = RelationState::default();
        state
            .observe(&base, &NumericValue::constant(U256::from(3)), &limits)
            .unwrap();
        state
            .observe(
                &product,
                &NumericValue::constant(U256::from(result)),
                &limits,
            )
            .unwrap();
        // Ordinary projection may trust an explicit result guarantee. Only a
        // feasibility check needs to compare that guarantee to its definition.
        assert_eq!(
            state.local_value(&product, &limits),
            ValueQuery::Exact(U256::from(result))
        );
        assert_eq!(state.check(&limits), expected);
    }
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
    let long_original = ExprId::word_read(
        ByteView::new(source.clone(), c(0), length.clone()),
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
    for original in [original, long_original] {
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
        assert_eq!(
            state.local_value(&original, &limits),
            ValueQuery::Exact(U256::from(7))
        );
    }
}

#[test]
fn unconstrained_word_and_concrete_padding_need_no_generic_numeric_operations() {
    let limits = RelationLimits::default();
    let state = RelationState::default();
    for (source, expected) in [
        (ByteStateId::input(SourceId::fresh().unwrap(), c(32)), None),
        (
            ByteStateId::exact(std::sync::Arc::from([0xabu8])),
            Some(U256::from(0xab) << 248usize),
        ),
        (ByteStateId::zero(), Some(U256::ZERO)),
    ] {
        let read = ExprId::word_read(
            ByteView::new(source, c(0), c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        // Less than one generic operation reservation in the measured trace.
        let mut budget = WorkBudget::new(30_000);
        let answer = state.local_value_budgeted(&read, &limits, &mut budget);
        match expected {
            Some(value) => assert_eq!(answer, ValueQuery::Exact(value)),
            None => {
                let ValueQuery::Bounds(bounds) = answer else {
                    panic!("unknown bytes should complete as ordinary bounds");
                };
                assert!(bounds.is_top());
            }
        }
        assert!(!budget.exhausted());
    }
}

#[test]
fn unknown_byte_with_padding_keeps_its_word_bits_and_both_signs() {
    let limits = RelationLimits::default();
    let source = ByteStateId::input(SourceId::fresh().unwrap(), c(1));
    let read = ExprId::word_read(
        ByteView::new(source, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut budget = WorkBudget::new(30_000);
    let ValueQuery::Bounds(bounds) =
        RelationState::default().local_value_budgeted(&read, &limits, &mut budget)
    else {
        panic!("one unknown byte should keep its complete padded-word bounds");
    };
    for byte in 0u64..=255 {
        assert!(bounds.contains(U256::from(byte) << 248usize));
    }
    assert!(!bounds.contains(U256::ONE));
    assert_eq!(bounds.known_bits().zero(), U256::MAX >> 8usize);
    assert_eq!(
        bounds.interval().unsigned_bounds(),
        (U256::ZERO, U256::from(255) << 248usize)
    );
    assert!(!budget.exhausted());
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
fn statically_impossible_alternative_implies_its_guard_without_search() {
    let limits = RelationLimits::default();
    let guard = ExprId::fresh().unwrap();
    let value = ExprId::fresh().unwrap();
    let impossible = RelationState {
        bottom: true,
        ..Default::default()
    };
    let mut possible = RelationState::default();
    assert_eq!(
        possible.add_equal(&value, &c(9), &limits),
        CheckResult::MayBeFeasible
    );
    for (then_state, else_state, expected) in [
        (&impossible, &possible, U256::ZERO),
        (&possible, &impossible, U256::ONE),
    ] {
        let joined = then_state
            .join_guarded(else_state, guard.clone(), &limits)
            .unwrap();
        assert_eq!(
            joined.local_value(&value, &limits),
            ValueQuery::Exact(U256::from(9))
        );
        if expected == U256::ZERO {
            assert_eq!(
                joined.local_value(&guard, &limits),
                ValueQuery::Exact(U256::ZERO)
            );
        } else {
            let ValueQuery::Bounds(bounds) = joined.local_value(&guard, &limits) else {
                panic!("nonzero guard should preserve every unproved nonzero word");
            };
            assert!(!bounds.contains(U256::ZERO));
            assert!(bounds.contains(U256::ONE));
            assert!(bounds.contains(U256::MAX));
        }
        assert_eq!(
            joined.can_equal(&guard, U256::ONE - expected, &limits),
            CheckResult::ProvenImpossible
        );
    }
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
