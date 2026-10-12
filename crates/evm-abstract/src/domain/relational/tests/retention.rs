//! Default-policy native checks for shared ordered updates above guarded states.

use super::super::{CheckResult, RelationLimits, RelationState, SmtProvider, ValueQuery};
use crate::domain::{
    NumericValue,
    symbolic::{
        ByteStateId, ByteStateKind, ByteView, ExprId, SlotStateId, SlotStateKind, SourceId,
        StateBinding, StateError, StateLimits,
    },
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        ..RelationLimits::default()
    }
}
fn op(code: u8, args: &[ExprId], limits: &RelationLimits) -> ExprId {
    ExprId::operation(code, args, limits.expressions()).unwrap()
}
fn read(state: &ByteStateId, at: u64, limits: &RelationLimits) -> ExprId {
    ExprId::word_read(
        ByteView::new(state.clone(), c(at), c(64)),
        c(0),
        limits.expressions(),
    )
    .unwrap()
}

#[test]
fn shared_updates_preserve_guarded_words_byte_overlays_and_slot_aliases_at_default_limits() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let states = limits.expressions().into();
        let guard = ExprId::fresh().unwrap();
        let yes = ByteStateId::zero()
            .write_word(c(0), c(0x1107), states)
            .unwrap();
        let no = ByteStateId::zero()
            .write_word(c(0), c(0x2209), states)
            .unwrap();
        let old = yes.join_guarded(&no, guard.clone(), states).unwrap();
        let overlaid = old.write_byte(c(31), c(0xaa), states).unwrap();
        assert_eq!(
            overlaid,
            yes.write_byte(c(31), c(0xaa), states)
                .unwrap()
                .join_guarded(
                    &no.write_byte(c(31), c(0xaa), states).unwrap(),
                    guard.clone(),
                    states,
                )
                .unwrap()
        );
        assert!(
            matches!(overlaid.kind(), ByteStateKind::WriteByte { previous, .. } if *previous == old)
        );
        let expected =
            ExprId::ite(guard.clone(), c(0x11aa), c(0x22aa), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().unique_value(
                &op(
                    opcode::EQ,
                    &[read(&overlaid, 0, &limits), expected],
                    &limits
                ),
                &limits,
            ),
            ValueQuery::Unique(U256::from(1)),
            "{provider:?}: byte update above guarded words"
        );
        let expected =
            ExprId::ite(guard.clone(), c(0x1107), c(0x2209), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().unique_value(
                &op(opcode::EQ, &[read(&old, 0, &limits), expected], &limits),
                &limits,
            ),
            ValueQuery::Unique(U256::from(1)),
        );

        let later_word = old.write_word(c(32), c(0x33), states).unwrap();
        assert_eq!(
            later_word,
            yes.write_word(c(32), c(0x33), states)
                .unwrap()
                .join_guarded(
                    &no.write_word(c(32), c(0x33), states).unwrap(),
                    guard.clone(),
                    states,
                )
                .unwrap()
        );
        assert!(
            matches!(later_word.kind(), ByteStateKind::WriteWord { previous, .. } if *previous == old)
        );
        let expected =
            ExprId::ite(guard.clone(), c(0x1107), c(0x2209), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().unique_value(
                &op(
                    opcode::EQ,
                    &[read(&later_word, 0, &limits), expected],
                    &limits
                ),
                &limits,
            ),
            ValueQuery::Unique(U256::from(1)),
            "{provider:?}: common disjoint word update"
        );

        let key = ExprId::fresh().unwrap();
        let initial = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            vec![StateBinding {
                index: U256::from(1),
                value: ExprId::fresh().unwrap(),
                numeric: NumericValue::constant(U256::from(7)),
            }],
            states,
        )
        .unwrap();
        let left = initial.store(c(0), c(7), states).unwrap();
        let right = initial.store(c(0), c(9), states).unwrap();
        let old = left.join_guarded(&right, guard.clone(), states).unwrap();
        let stored = old.store(key.clone(), c(13), states).unwrap();
        assert_eq!(
            stored,
            left.store(key.clone(), c(13), states)
                .unwrap()
                .join_guarded(
                    &right.store(key.clone(), c(13), states).unwrap(),
                    guard.clone(),
                    states,
                )
                .unwrap()
        );
        assert!(matches!(stored.kind(), SlotStateKind::Store { previous, .. } if *previous == old));
        let observed = ExprId::slot_read(stored.clone(), c(1), limits.expressions()).unwrap();
        let latest = ExprId::slot_read(stored, c(0), limits.expressions()).unwrap();
        let historical = ExprId::slot_read(old, c(0), limits.expressions()).unwrap();
        for (index, expected) in [(0, c(13)), (1, historical)] {
            let mut path = RelationState::default();
            assert_eq!(path.add_equal(&key, &c(index), &limits), CheckResult::Sat);
            assert_eq!(
                path.unique_value(
                    &op(opcode::EQ, &[latest.clone(), expected], &limits),
                    &limits
                ),
                ValueQuery::Unique(U256::from(1)),
                "{provider:?}: alias at {index}"
            );
            assert_eq!(
                path.unique_value(&observed, &limits),
                ValueQuery::Unique(U256::from(if index == 0 { 7 } else { 13 })),
                "{provider:?}: observed initial slot at alias {index}"
            );
        }
        for (branch, expected) in [(1, 0x11aa), (0, 0x22aa)] {
            let mut path = RelationState::default();
            assert_eq!(
                path.add_equal(&guard, &c(branch), &limits),
                CheckResult::Sat
            );
            assert_eq!(
                path.unique_value(&read(&overlaid, 0, &limits), &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}: selected byte overlay {branch}"
            );
        }
    }
}

#[test]
fn shared_copy_keeps_observed_array_facts_and_captured_history_at_default_limits() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let states = limits.expressions().into();
        let observed = ExprId::fresh().unwrap();
        let source = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            c(32),
            vec![StateBinding {
                index: U256::from(31),
                value: observed,
                numeric: NumericValue::constant(U256::from(12)),
            }],
            states,
        )
        .unwrap();
        let view = ByteView::new(source, c(0), c(32));
        let guard = ExprId::fresh().unwrap();
        let left = ByteStateId::zero().write_word(c(0), c(7), states).unwrap();
        let right = ByteStateId::zero().write_word(c(0), c(9), states).unwrap();
        let old = left.join_guarded(&right, guard.clone(), states).unwrap();
        let copied = old.copy(c(32), view.clone(), c(32), states).unwrap();
        assert_eq!(
            copied,
            left.copy(c(32), view.clone(), c(32), states)
                .unwrap()
                .join_guarded(
                    &right.copy(c(32), view.clone(), c(32), states).unwrap(),
                    guard.clone(),
                    states,
                )
                .unwrap()
        );
        assert!(
            matches!(copied.kind(), ByteStateKind::Copy { previous, source, .. } if *previous == old && *source == view)
        );
        assert_eq!(
            ByteView::new(copied.clone(), c(32), c(32)).canonicalize(),
            view
        );
        let low_byte = op(opcode::AND, &[read(&copied, 32, &limits), c(255)], &limits);
        assert_eq!(
            RelationState::default().unique_value(&low_byte, &limits),
            ValueQuery::Unique(U256::from(12)),
            "{provider:?}: captured initial numeric byte"
        );
        let expected = ExprId::ite(guard, c(7), c(9), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().unique_value(
                &op(opcode::EQ, &[read(&copied, 0, &limits), expected], &limits),
                &limits,
            ),
            ValueQuery::Unique(U256::from(1)),
            "{provider:?}: disjoint historical word"
        );
    }
}

#[test]
fn differing_updates_and_independent_captured_sources_keep_the_selector() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let states = limits.expressions().into();
        let guard = ExprId::fresh().unwrap();
        let zero = ByteStateId::zero();
        let left = zero.write_word(c(0), c(7), states).unwrap();
        for other in [
            zero.write_word(c(0), c(9), states).unwrap(),
            zero.write_word(c(1), c(7), states).unwrap(),
            zero.write_byte(c(0), c(7), states).unwrap(),
        ] {
            assert!(matches!(
                left.join_guarded(&other, guard.clone(), states)
                    .unwrap()
                    .kind(),
                ByteStateKind::Ite { .. }
            ));
        }
        let slots = SlotStateId::zero();
        let left = slots.store(c(0), c(7), states).unwrap();
        for other in [
            slots.store(c(0), c(9), states).unwrap(),
            slots.store(c(1), c(7), states).unwrap(),
        ] {
            assert!(matches!(
                left.join_guarded(&other, guard.clone(), states)
                    .unwrap()
                    .kind(),
                SlotStateKind::Ite { .. }
            ));
        }

        let length = ExprId::fresh().unwrap();
        let a = ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            c(0),
            length.clone(),
        );
        let b = ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            c(0),
            length.clone(),
        );
        let left = zero.copy(c(0), a.clone(), length.clone(), states).unwrap();
        let right = zero.copy(c(0), b.clone(), length.clone(), states).unwrap();
        assert!(matches!(
            left.join_guarded(&right, guard, states).unwrap().kind(),
            ByteStateKind::Ite { .. }
        ));
        let equal = op(
            opcode::EQ,
            &[
                ExprId::keccak(a, limits.expressions()).unwrap(),
                ExprId::keccak(b, limits.expressions()).unwrap(),
            ],
            &limits,
        );
        let mut path = RelationState::default();
        assert_eq!(
            path.add_equal(&length, &ExprId::constant(U256::MAX), &limits),
            CheckResult::Sat,
        );
        for result in [U256::ZERO, U256::from(1)] {
            assert_eq!(
                path.can_equal(&equal, result, &limits),
                CheckResult::Sat,
                "{provider:?}: distinct source identities do not assert hash inequality or equality"
            );
        }
    }
}

#[test]
fn shared_updates_bound_history_and_preserve_rename_max_padding_and_cancellation() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let states = limits.expressions().into();
        let input = ExprId::fresh().unwrap();
        let mut value = input.clone();
        for _ in 0..6 {
            value = op(opcode::ADD, &[value.clone(), value], &limits);
        }
        let source = ByteView::new(
            ByteStateId::zero().write_word(c(0), value, states).unwrap(),
            c(0),
            c(32),
        );
        let mut current = ByteStateId::zero()
            .copy(c(32), source.clone(), c(32), states)
            .unwrap();
        let mut unfactored_nodes = current.nodes();
        for value in 1..=20 {
            let alternative = ByteStateId::zero()
                .write_word(c(0), c(value), states)
                .unwrap()
                .copy(c(32), source.clone(), c(32), states)
                .unwrap();
            unfactored_nodes += alternative.nodes() + 2;
            current = current
                .join_guarded(&alternative, ExprId::fresh().unwrap(), states)
                .unwrap();
        }
        assert!(unfactored_nodes > limits.max_nodes);
        assert!(current.nodes() <= limits.max_nodes);
        assert!(current.depth() <= limits.max_depth);
        let replacement = ExprId::fresh().unwrap();
        let map = BTreeMap::from([(input.clone(), replacement.clone())]);
        let renamed = current.rename_fresh(&map, limits.expressions()).unwrap();
        assert_ne!(renamed, current);
        assert!(current.fresh_leaves().contains(&input));
        assert!(!renamed.fresh_leaves().contains(&input));
        for (state, symbol) in [(&current, &input), (&renamed, &replacement)] {
            let mut path = RelationState::default();
            assert_eq!(path.add_equal(symbol, &c(13), &limits), CheckResult::Sat);
            assert_eq!(
                path.unique_value(&read(state, 32, &limits), &limits),
                ValueQuery::Unique(U256::from(13 * 64)),
                "{provider:?}: one captured scalar history"
            );
        }
        let denied = StateLimits {
            max_nodes: current.nodes() - 1,
            ..states
        };
        assert!(matches!(
            current.join_guarded(&current, c(1), denied),
            Err(StateError::Limit { .. })
        ));
        let demanded = read(&current, 32, &limits);
        let token = embedded_smt::Cancellation::new();
        token.cancel();
        embedded_smt::with_cancellation(&token, || {
            assert_eq!(
                current.join_guarded(&current, c(1), states),
                Err(StateError::Cancelled)
            );
            assert_eq!(
                RelationState::default().unique_value(&demanded, &limits),
                ValueQuery::Unknown(super::super::QueryReason::Cancelled)
            );
        });

        let at = ExprId::constant(U256::MAX);
        let all = ExprId::constant(U256::MAX);
        let yes = ByteStateId::zero()
            .write_word(at.clone(), all, states)
            .unwrap();
        let selected = yes
            .join_guarded(&ByteStateId::zero(), ExprId::fresh().unwrap(), states)
            .unwrap()
            .write_byte(at.clone(), c(0xaa), states)
            .unwrap();
        let word = ExprId::word_read(
            ByteView::new(selected, at, c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        assert_eq!(
            RelationState::default().unique_value(&word, &limits),
            ValueQuery::Unique(U256::from(0xaa) << 248),
            "{provider:?}: factored byte overlay must not wrap at MAX"
        );
    }
}
