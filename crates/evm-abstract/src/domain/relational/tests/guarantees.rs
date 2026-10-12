//! Native checks for conjunction reduction and its guarded/overlap boundaries.

use super::super::{
    CheckResult, Constraint, RelationLimits, RelationState, SmtProvider, ValueQuery,
};
use crate::domain::symbolic::{ByteStateId, ByteView, ExprId, SlotStateId, SourceId};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::sync::Arc;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn policy(provider: SmtProvider) -> RelationLimits {
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

#[test]
fn default_fuel_conjunction_aliases_keep_their_asserted_predicates() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let storage = SlotStateId::zero()
            .store(k.clone(), c(7), limits.expressions().into())
            .unwrap()
            .store(q.clone(), c(9), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(storage, k.clone(), limits.expressions()).unwrap();
        let equality = op(opcode::EQ, &[k, q], limits);
        // ISZERO chains must retain EVM zero/nonzero meaning and key order.
        for equal in [false, true] {
            for negations in 0..3 {
                let mut guard = equality.clone();
                for _ in 0..negations {
                    guard = op(opcode::ISZERO, &[guard], limits);
                }
                let mut path = RelationState::default();
                assert_eq!(
                    path.assume(&guard, equal ^ (negations % 2 != 0), &limits),
                    CheckResult::Sat,
                    "{provider:?}"
                );
                assert_eq!(
                    path.unique_value(&read, &limits),
                    ValueQuery::Unique(U256::from(if equal { 9 } else { 7 })),
                    "{provider:?}, equal={equal}, negations={negations}"
                );
                // A fold must never erase the premise supporting it.
                assert_eq!(
                    path.can_equal(&equality, U256::from(u8::from(!equal)), &limits),
                    CheckResult::Unsat,
                    "{provider:?}"
                );
                assert_eq!(path.assume(&equality, !equal, &limits), CheckResult::Unsat);
            }
        }
    }
}

#[test]
fn retained_inconsistent_equalities_and_ranges_cannot_produce_a_folded_value() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let storage = SlotStateId::zero()
            .store(k.clone(), c(7), limits.expressions().into())
            .unwrap()
            .store(q.clone(), c(9), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(storage, k.clone(), limits.expressions()).unwrap();
        // Retained constraints can await checking after imports or joins.
        // Exercise the native assertions without eager insertion setting bottom.
        let equal = RelationState {
            constraints: [
                Constraint::Equal {
                    left: k.clone(),
                    right: q.clone(),
                },
                Constraint::Truth {
                    expression: op(opcode::EQ, &[k, q], limits),
                    nonzero: false,
                },
            ]
            .into_iter()
            .collect(),
            ..RelationState::default()
        };
        assert!(!equal.bottom);
        assert_eq!(
            equal.unique_value(&read, &limits),
            ValueQuery::Infeasible,
            "{provider:?}"
        );

        let length = ExprId::fresh().unwrap();
        let source = ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            c(0),
            length.clone(),
        );
        let at = ExprId::constant(U256::MAX - U256::from(31));
        let memory = ByteStateId::zero()
            .copy(
                at.clone(),
                source,
                length.clone(),
                limits.expressions().into(),
            )
            .unwrap();
        let hash = ExprId::keccak(
            ByteView::new(memory, at, length.clone()),
            limits.expressions(),
        )
        .unwrap();
        let range = RelationState {
            constraints: [(1, 16), (33, 64)]
                .map(|(lower, upper)| Constraint::Range {
                    expression: length.clone(),
                    lower: U256::from(lower),
                    upper: U256::from(upper),
                })
                .into_iter()
                .collect(),
            ..RelationState::default()
        };
        assert!(!range.bottom);
        assert_eq!(
            range.unique_value(&hash, &limits),
            ValueQuery::Infeasible,
            "{provider:?}"
        );
    }
}

#[test]
fn guarded_path_alias_facts_never_become_unconditional_read_rewrites() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let selector = ExprId::fresh().unwrap();
        let storage = SlotStateId::zero()
            .store(k.clone(), c(7), limits.expressions().into())
            .unwrap()
            .store(q.clone(), c(9), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(storage, k.clone(), limits.expressions()).unwrap();
        let equality = op(opcode::EQ, &[k, q], limits);
        let mut same = RelationState::default();
        let mut different = RelationState::default();
        assert_eq!(same.assume(&equality, true, &limits), CheckResult::Sat);
        assert_eq!(
            different.assume(&equality, false, &limits),
            CheckResult::Sat
        );
        let joined = same
            .join_guarded(&different, selector.clone(), &limits)
            .unwrap();
        assert_eq!(joined.unique_value(&read, &limits), ValueQuery::Multiple);
        for (branch, expected) in [(true, 9), (false, 7)] {
            assert_eq!(
                joined.can_equal(&read, U256::from(expected), &limits),
                CheckResult::Sat,
                "{provider:?}, both guarded valuations survive"
            );
            let mut selected = joined.clone();
            assert_eq!(
                selected.assume(&selector, branch, &limits),
                CheckResult::Sat
            );
            assert_eq!(
                selected.unique_value(&read, &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}, branch={branch}"
            );
        }
    }
}

#[test]
fn bounded_hash_rewrites_retain_overlap_overflow_and_partial_copy_alternatives() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
        let original = ByteView::new(input, c(0), length.clone());
        let source_hash = ExprId::keccak(original.clone(), limits.expressions()).unwrap();
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
                ByteView::new(ByteStateId::exact(Arc::from([0xaa, 0xbb])), c(0), c(2)),
                c(2),
                limits.expressions().into(),
            )
            .unwrap();
        let old_hash = ExprId::keccak(
            ByteView::new(memory, c(0), length.clone()),
            limits.expressions(),
        )
        .unwrap();
        let equality = op(opcode::EQ, &[old_hash, source_hash.clone()], limits);
        let mut path = RelationState::default();
        assert_eq!(
            path.add_unsigned_bounds(&length, U256::from(512), U256::from(2048), &limits),
            CheckResult::Sat
        );
        // The same admitted length range has both a disjoint boundary and an
        // overlapping copy. A bound that straddles the guard cannot fold it.
        for (sample, disjoint) in [(1024, true), (1025, false)] {
            let mut boundary = path.clone();
            assert_eq!(
                boundary.add_equal(&length, &c(sample), &limits),
                CheckResult::Sat
            );
            if disjoint {
                assert_eq!(
                    boundary.unique_value(&equality, &limits),
                    ValueQuery::Unique(U256::from(1))
                );
            } else {
                assert_eq!(
                    boundary.can_equal(&equality, U256::ZERO, &limits),
                    CheckResult::Sat
                );
                assert_eq!(
                    boundary.can_equal(&equality, U256::from(1), &limits),
                    CheckResult::Sat
                );
            }
        }
        let overflow_at = ExprId::constant(U256::MAX - U256::from(31));
        for (destination, copied) in [(overflow_at, length.clone()), (c(128), c(96))] {
            let memory = ByteStateId::zero()
                .copy(
                    destination.clone(),
                    original.clone(),
                    copied.clone(),
                    limits.expressions().into(),
                )
                .unwrap();
            let hash = ExprId::keccak(
                ByteView::new(memory, destination, copied),
                limits.expressions(),
            )
            .unwrap();
            let equality = op(opcode::EQ, &[hash, source_hash.clone()], limits);
            for expected in [0, 1] {
                assert_eq!(
                    path.can_equal(&equality, U256::from(expected), &limits),
                    CheckResult::Sat,
                    "{provider:?}, invalid whole range or partial source copy; collisions remain possible"
                );
            }
        }
    }
}
