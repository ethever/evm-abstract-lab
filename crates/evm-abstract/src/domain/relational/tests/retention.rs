//! Default-policy local checks for shared ordered updates above guarded states.

fn product_algebra() -> crate::domain::ValueAlgebra<crate::domain::ProductAlgebra> {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
}
use super::super::{CheckResult, EvmValueQuery, RelationLimits, RelationState};
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
fn policy() -> RelationLimits {
    RelationLimits {
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
    {
        let limits = policy();
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
            RelationState::default().local_value(
                &op(
                    opcode::EQ,
                    &[read(&overlaid, 0, &limits), expected],
                    &limits
                ),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
            "local: byte update above guarded words"
        );
        let expected =
            ExprId::ite(guard.clone(), c(0x1107), c(0x2209), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().local_value(
                &op(opcode::EQ, &[read(&old, 0, &limits), expected], &limits),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
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
            RelationState::default().local_value(
                &op(
                    opcode::EQ,
                    &[read(&later_word, 0, &limits), expected],
                    &limits
                ),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
            "local: common disjoint word update"
        );

        let key = ExprId::fresh().unwrap();
        let initial = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            vec![StateBinding {
                index: U256::from(1),
                value: ExprId::fresh().unwrap(),
                numeric: NumericValue::constant(U256::from(7)).into(),
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
            assert_eq!(
                path.add_equal(&key, &c(index), &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                path.local_value(
                    &op(opcode::EQ, &[latest.clone(), expected], &limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from(1)),
                "local: alias at {index}"
            );
            assert_eq!(
                path.local_value(&observed, &limits, product_algebra().borrowed()),
                EvmValueQuery::Exact(U256::from(if index == 0 { 7 } else { 13 })),
                "local: observed initial slot at alias {index}"
            );
        }
        for (branch, expected) in [(1, 0x11aa), (0, 0x22aa)] {
            let mut path = RelationState::default();
            assert_eq!(
                path.add_equal(&guard, &c(branch), &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                path.local_value(
                    &read(&overlaid, 0, &limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from(expected)),
                "local: selected byte overlay {branch}"
            );
        }
    }
}

#[test]
fn shared_copy_keeps_observed_array_facts_and_captured_history_at_default_limits() {
    {
        let limits = policy();
        let states = limits.expressions().into();
        let observed = ExprId::fresh().unwrap();
        let source = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            c(32),
            vec![StateBinding {
                index: U256::from(31),
                value: observed,
                numeric: NumericValue::constant(U256::from(12)).into(),
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
            RelationState::default().local_value(&low_byte, &limits, product_algebra().borrowed()),
            EvmValueQuery::Exact(U256::from(12)),
            "local: captured initial numeric byte"
        );
        let expected = ExprId::ite(guard, c(7), c(9), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().local_value(
                &op(opcode::EQ, &[read(&copied, 0, &limits), expected], &limits),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
            "local: disjoint historical word"
        );
    }
}

#[test]
fn differing_updates_and_independent_captured_sources_keep_the_selector() {
    {
        let limits = policy();
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
            path.add_equal(
                &length,
                &ExprId::constant(U256::MAX),
                &limits,
                product_algebra().borrowed()
            ),
            CheckResult::MayBeFeasible,
        );
        for result in [U256::ZERO, U256::from(1)] {
            assert_eq!(
                path.can_equal(&equal, result, &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible,
                "local: distinct source identities do not assert hash inequality or equality"
            );
        }
    }
}

#[test]
fn shared_updates_bound_history_and_preserve_rename_max_padding_and_cancellation() {
    {
        let limits = policy();
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
            assert_eq!(
                path.add_equal(symbol, &c(13), &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                path.local_value(
                    &read(state, 32, &limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from(13 * 64)),
                "local: one captured scalar history"
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
        let token = analysis_control::Cancellation::new();
        token.cancel();
        analysis_control::with_cancellation(&token, || {
            assert_eq!(
                current.join_guarded(&current, c(1), states),
                Err(StateError::Cancelled)
            );
            assert_eq!(
                RelationState::default().local_value(
                    &demanded,
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Undetermined(super::super::QueryReason::Cancelled)
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
            RelationState::default().local_value(&word, &limits, product_algebra().borrowed()),
            EvmValueQuery::Exact(U256::from(0xaa) << 248),
            "local: factored byte overlay must not wrap at MAX"
        );
    }
}

#[test]
fn fixed_slot_reads_keep_guarded_history_initial_facts_and_symbolic_aliases() {
    {
        let limits = policy();
        let states = limits.expressions().into();
        let high_key = ExprId::constant(U256::from(1) << 255);
        let full_word = ExprId::constant(U256::MAX - U256::from(17));
        let wide = SlotStateId::zero()
            .store(c(0), full_word.clone(), states)
            .unwrap()
            .store(high_key.clone(), c(13), states)
            .unwrap();
        let read = ExprId::slot_read(wide, c(0), limits.expressions()).unwrap();
        assert_eq!(read, full_word);
        assert_eq!(
            RelationState::default().local_value(&read, &limits, product_algebra().borrowed()),
            EvmValueQuery::Exact(U256::MAX - U256::from(17))
        );
        let wide = SlotStateId::zero()
            .store(high_key.clone(), full_word.clone(), states)
            .unwrap()
            .store(c(0), c(13), states)
            .unwrap();
        assert_eq!(
            ExprId::slot_read(wide, high_key, limits.expressions()).unwrap(),
            full_word
        );
        let x = ExprId::fresh().unwrap();
        let y = ExprId::fresh().unwrap();
        let guard = ExprId::fresh().unwrap();
        let left = SlotStateId::zero()
            .store(c(0), x.clone(), states)
            .unwrap()
            .store(c(1), c(32), states)
            .unwrap();
        let right = SlotStateId::zero()
            .store(c(0), y.clone(), states)
            .unwrap()
            .store(c(1), c(64), states)
            .unwrap();
        let selected = left.join_guarded(&right, guard.clone(), states).unwrap();
        let old = ExprId::slot_read(selected.clone(), c(0), limits.expressions()).unwrap();
        let expected =
            ExprId::ite(guard.clone(), x.clone(), y.clone(), limits.expressions()).unwrap();
        assert_eq!(old, expected);
        let key = ExprId::fresh().unwrap();
        let updated = selected.store(key.clone(), c(13), states).unwrap();
        let latest = ExprId::slot_read(updated, c(0), limits.expressions()).unwrap();
        for alias in [0, 1] {
            let mut path = RelationState::default();
            for (input, value) in [(&x, 7), (&y, 9), (&key, alias)] {
                assert_eq!(
                    path.add_equal(input, &c(value), &limits, product_algebra().borrowed()),
                    CheckResult::MayBeFeasible
                );
            }
            let expected = if alias == 0 { c(13) } else { old.clone() };
            assert_eq!(
                path.local_value(
                    &op(opcode::EQ, &[latest.clone(), expected], &limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from(1)),
                "local: fixed read alias {alias}"
            );
        }
        let initial = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            vec![StateBinding {
                index: U256::ZERO,
                value: x,
                numeric: NumericValue::constant(U256::from(7)).into(),
            }],
            states,
        )
        .unwrap()
        .store(c(1), c(99), states)
        .unwrap();
        assert_eq!(
            RelationState::default().local_value(
                &ExprId::slot_read(initial, c(0), limits.expressions()).unwrap(),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(7)),
            "local: initial observed slot after a disjoint write"
        );
        let renamed = selected
            .rename_fresh(&BTreeMap::from([(y, c(11))]), limits.expressions())
            .unwrap();
        let read = ExprId::slot_read(renamed, c(0), limits.expressions()).unwrap();
        let mut path = RelationState::default();
        assert_eq!(
            path.add_equal(&guard, &c(0), &limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            path.local_value(&read, &limits, product_algebra().borrowed()),
            EvmValueQuery::Exact(U256::from(11))
        );
        assert!(
            ExprId::slot_read(
                selected.clone(),
                c(0),
                crate::domain::symbolic::ExprLimits {
                    max_nodes: selected.nodes(),
                    ..limits.expressions()
                },
            )
            .is_err()
        );
        let token = analysis_control::Cancellation::new();
        token.cancel();
        analysis_control::with_cancellation(&token, || {
            assert_eq!(
                ExprId::slot_read(selected.clone(), c(0), limits.expressions()),
                Err(crate::domain::symbolic::ExprError::Cancelled)
            );
        });
    }
}
