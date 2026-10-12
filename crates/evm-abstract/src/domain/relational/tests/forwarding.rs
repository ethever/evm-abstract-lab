//! Default-budget proofs and independent boundary checks for demanded reads.

use super::super::{
    CheckResult, QueryReason, RelationLimits, RelationState, SmtProvider, ValueQuery,
};
use crate::domain::{identity::Symbol, numeric::NumericValue, symbolic::*};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;
use std::sync::Arc;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn policy(provider: SmtProvider) -> RelationLimits {
    // Match the actual acceptance policy; retain the provider's default fuel.
    RelationLimits {
        provider,
        max_nodes: 8192,
        max_depth: 128,
        max_constraints: 256,
        ..RelationLimits::default()
    }
}
fn op(code: u8, args: &[ExprId], limits: RelationLimits) -> ExprId {
    ExprId::operation(code, args, limits.expressions()).unwrap()
}
fn read(state: ByteStateId, at: ExprId, limits: RelationLimits) -> ExprId {
    ExprId::word_read(ByteView::new(state, at, c(32)), c(0), limits.expressions()).unwrap()
}

#[test]
fn default_policy_forwards_symbolic_words_and_byte_overlays_with_history() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::input(51, Symbol::CalldataLength);
        let input = ByteStateId::input(SourceId::input(51, Symbol::CalldataLength), length);
        let p = op(
            opcode::AND,
            &[read(input.clone(), c(0), limits), c(511)],
            limits,
        );
        let x = read(input, c(32), limits);
        let memory = ByteStateId::zero()
            .write_word(p.clone(), x.clone(), limits.expressions().into())
            .unwrap();
        let historical = read(memory.clone(), p.clone(), limits);
        let last = op(opcode::ADD, &[p.clone(), c(31)], limits);
        let overlaid = memory
            .write_byte(last, c(0xaa), limits.expressions().into())
            .unwrap();
        let latest = read(overlaid, p, limits);
        let expected = op(
            opcode::OR,
            &[
                op(
                    opcode::AND,
                    &[x.clone(), ExprId::constant(U256::MAX - U256::from(255))],
                    limits,
                ),
                c(0xaa),
            ],
            limits,
        );
        for (overlaid, equality) in [
            (false, op(opcode::EQ, &[historical, x], limits)),
            (true, op(opcode::EQ, &[latest, expected], limits)),
        ] {
            let path = RelationState::default();
            if provider == SmtProvider::Cvc5 && overlaid {
                assert_eq!(limits.rlimit, 100_000);
                // Pin the observed boundary of this input-derived address query
                // at 100k fuel. UNKNOWN cannot claim the contrary path infeasible.
                assert_eq!(
                    path.can_equal(&equality, U256::from(1), &limits),
                    CheckResult::Unknown(QueryReason::ResourceLimit),
                    "Cvc5 overlay satisfiability at 100k fuel"
                );
                assert_eq!(
                    path.can_equal(&equality, U256::ZERO, &limits),
                    CheckResult::Unknown(QueryReason::ResourceLimit),
                    "Cvc5 overlay contradiction proof"
                );
                assert_eq!(
                    path.unique_value(&equality, &limits),
                    ValueQuery::Unknown(QueryReason::ResourceLimit),
                    "Cvc5 overlay uniqueness"
                );
                continue;
            }
            assert_eq!(
                path.unique_value(&equality, &limits),
                ValueQuery::Unique(U256::from(1)),
                "{provider:?}, overlaid={overlaid}"
            );
        }
    }
}

#[test]
fn default_policy_forwarded_words_zero_pad_overflow_and_keep_wrapped_bytes_separate() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for start in [
            U256::ZERO,
            U256::MAX - U256::from(31),
            U256::MAX - U256::from(1),
            U256::MAX,
        ] {
            let p = ExprId::fresh().unwrap();
            let memory = ByteStateId::zero()
                .write_word(
                    p.clone(),
                    ExprId::constant(U256::from_be_bytes([0x55; 32])),
                    limits.expressions().into(),
                )
                .unwrap();
            let historical = read(memory.clone(), p.clone(), limits);
            let last = op(opcode::ADD, &[p.clone(), c(31)], limits);
            let updated = read(
                memory
                    .write_byte(last, c(0xaa), limits.expressions().into())
                    .unwrap(),
                p.clone(),
                limits,
            );
            let mut old = [0; 32];
            let mut new = [0; 32];
            for (j, byte) in old.iter_mut().enumerate() {
                if let Some(index) = start.checked_add(U256::from(j)) {
                    *byte = 0x55;
                    new[j] = if index == start.wrapping_add(U256::from(31)) {
                        0xaa
                    } else {
                        0x55
                    };
                }
            }
            let mut path = RelationState::default();
            assert_eq!(
                path.add_equal(&p, &ExprId::constant(start), &limits),
                CheckResult::Sat
            );
            assert_eq!(
                path.unique_value(&historical, &limits),
                ValueQuery::Unique(U256::from_be_bytes(old)),
                "{provider:?} at {start}"
            );
            assert_eq!(
                path.unique_value(&updated, &limits),
                ValueQuery::Unique(U256::from_be_bytes(new)),
                "{provider:?} at {start}"
            );
        }
    }
}

#[test]
fn default_policy_concrete_read_after_symbolic_store_retains_both_alias_paths() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let input = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::fresh().unwrap());
        let key = read(input, c(0), limits);
        let state = SlotStateId::zero()
            .store(c(0), c(7), limits.expressions().into())
            .unwrap()
            .store(key.clone(), c(9), limits.expressions().into())
            .unwrap();
        let value = ExprId::slot_read(state, c(0), limits.expressions()).unwrap();
        let guard = op(opcode::ISZERO, &[key], limits);
        for (alias, expected) in [(true, 9), (false, 7)] {
            let mut path = RelationState::default();
            assert_eq!(
                path.assume(&guard, alias, &limits),
                CheckResult::Sat,
                "{provider:?}, alias={alias} assumption"
            );
            assert_eq!(
                path.unique_value(&value, &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}"
            );
        }
    }
}

#[test]
fn input_word_prefix_padding_matches_observed_bytes_and_view_boundaries() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for (start, lengths) in [
            (
                U256::ZERO,
                vec![0, 1, 7, 31, 32, 33]
                    .into_iter()
                    .map(U256::from)
                    .collect::<Vec<_>>(),
            ),
            (
                U256::from(32),
                vec![0, 32, 33, 63, 64, 65]
                    .into_iter()
                    .map(U256::from)
                    .collect(),
            ),
            (
                U256::MAX - U256::from(31),
                vec![
                    U256::ZERO,
                    U256::MAX - U256::from(31),
                    U256::MAX - U256::from(1),
                    U256::MAX,
                ],
            ),
            (U256::MAX, vec![U256::ZERO, U256::MAX]),
        ] {
            let length = ExprId::fresh().unwrap();
            let bindings = (0..32)
                .filter_map(|j| {
                    start.checked_add(U256::from(j)).map(|index| StateBinding {
                        index,
                        // A symbolic initial observation must retain its numeric
                        // fact. Other cells use concrete observations so this
                        // padding test does not add 32 unrelated scalar leaves.
                        value: if j == 0 {
                            ExprId::fresh().unwrap()
                        } else {
                            c(j + 1)
                        },
                        numeric: NumericValue::constant(U256::from(j + 1)),
                    })
                })
                .collect();
            let source = ByteStateId::observed(
                SourceId::fresh().unwrap(),
                length.clone(),
                bindings,
                limits.expressions().into(),
            )
            .unwrap();
            for boundary in lengths {
                let mut path = RelationState::default();
                assert_eq!(
                    path.add_equal(&length, &ExprId::constant(boundary), &limits),
                    CheckResult::Sat
                );
                for requested in [length.clone(), c(32), c(7)] {
                    let word = ExprId::word_read(
                        ByteView::new(source.clone(), ExprId::constant(start), requested.clone()),
                        c(0),
                        limits.expressions(),
                    )
                    .unwrap();
                    let requested = requested.as_constant().unwrap_or(boundary);
                    let expected: [u8; 32] = std::array::from_fn(|j| {
                        if U256::from(j) < requested
                            && start
                                .checked_add(U256::from(j))
                                .is_some_and(|index| index < boundary)
                        {
                            (j + 1) as u8
                        } else {
                            0
                        }
                    });
                    assert_eq!(
                        path.unique_value(&word, &limits),
                        ValueQuery::Unique(U256::from_be_bytes(expected)),
                        "{provider:?}, start={start}, source length={boundary}, view length={requested}"
                    );
                }
            }
        }
    }
}

#[test]
fn default_policy_guarded_echo_hashes_keep_empty_data_and_disjoint_copy_relations() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
        let original = ByteView::new(input.clone(), c(0), length.clone());
        let guard = ExprId::fresh().unwrap();
        let returned_length =
            ExprId::ite(guard.clone(), c(0), length.clone(), limits.expressions()).unwrap();
        let returned = ByteStateId::zero()
            .join_guarded(&input, guard.clone(), limits.expressions().into())
            .unwrap();
        let returned_view = ByteView::new(returned, c(0), returned_length.clone());
        assert_eq!(
            returned_view.hash_support(limits.expressions()).unwrap(),
            HashSupport::GuardedState
        );
        let memory = ByteStateId::zero()
            .copy(
                c(0),
                original.clone(),
                length.clone(),
                limits.expressions().into(),
            )
            .unwrap()
            .copy(
                c(1024),
                returned_view,
                returned_length.clone(),
                limits.expressions().into(),
            )
            .unwrap();
        let old_hash = ExprId::keccak(
            ByteView::new(memory.clone(), c(0), length.clone()),
            limits.expressions(),
        )
        .unwrap();
        let new_hash = ExprId::keccak(
            ByteView::new(memory, c(1024), returned_length),
            limits.expressions(),
        )
        .unwrap();
        let source_hash = ExprId::keccak(original, limits.expressions()).unwrap();
        let mut path = RelationState::default();
        assert_eq!(
            path.add_unsigned_bounds(&length, U256::from(32), U256::from(512), &limits),
            CheckResult::Sat
        );
        assert_eq!(path.assume(&guard, false, &limits), CheckResult::Sat);
        for (label, hash) in [("old_hash", old_hash), ("new_hash", new_hash.clone())] {
            assert_eq!(
                path.unique_value(
                    &op(opcode::EQ, &[hash, source_hash.clone()], limits),
                    &limits
                ),
                ValueQuery::Unique(U256::from(1)),
                "{provider:?}, hash={label}"
            );
        }
        let mut empty = RelationState::default();
        assert_eq!(empty.assume(&guard, true, &limits), CheckResult::Sat);
        assert_eq!(
            empty.unique_value(&new_hash, &limits),
            ValueQuery::Unique(U256::from_be_bytes(keccak256([]).0)),
            "{provider:?}"
        );
    }
}

#[test]
fn overlapping_copy_hashes_do_not_claim_preservation_from_wrapped_range_ends() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for at in [U256::ZERO, U256::MAX] {
            let length = ExprId::fresh().unwrap();
            let input = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::MAX));
            let view = ByteView::new(input.clone(), ExprId::constant(at), length.clone());
            let before = ExprId::keccak(view, limits.expressions()).unwrap();
            let copied = input
                .copy(
                    ExprId::constant(at),
                    ByteView::new(ByteStateId::exact(Arc::from([0xaa, 0xbb])), c(0), c(2)),
                    c(2),
                    limits.expressions().into(),
                )
                .unwrap();
            let after = ExprId::keccak(
                ByteView::new(copied, ExprId::constant(at), length.clone()),
                limits.expressions(),
            )
            .unwrap();
            let equality = op(opcode::EQ, &[before, after], limits);
            let mut path = RelationState::default();
            assert_eq!(path.add_equal(&length, &c(128), &limits), CheckResult::Sat);
            assert_eq!(
                path.can_equal(&equality, U256::ZERO, &limits),
                CheckResult::Sat,
                "{provider:?} at {at}"
            );
            assert_eq!(
                path.can_equal(&equality, U256::from(1), &limits),
                CheckResult::Sat,
                "hash collisions remain possible"
            );
        }
    }
}
