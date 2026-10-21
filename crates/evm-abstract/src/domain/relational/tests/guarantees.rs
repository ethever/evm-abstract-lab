//! Local conjunction consequences and guarded/overlap boundaries.

fn product_algebra() -> crate::domain::ValueAlgebra<crate::domain::ProductAlgebra> {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
}
use super::super::{CheckResult, Constraint, EvmValueQuery, RelationLimits, RelationState};
use crate::domain::symbolic::{ByteStateId, ByteView, ExprId, SlotStateId, SourceId, StateBinding};
use crate::domain::{EvmValue, NumericValue, ProductAlgebra};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::sync::Arc;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn policy() -> RelationLimits {
    RelationLimits {
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
    {
        let limits = policy();
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
                    path.assume(
                        &guard,
                        equal ^ (negations % 2 != 0),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible,
                    "local"
                );
                assert_eq!(
                    path.local_value(&read, &limits, product_algebra().borrowed()),
                    EvmValueQuery::Exact(U256::from(if equal { 9 } else { 7 })),
                    "local, equal={equal}, negations={negations}"
                );
                // A fold must never erase the premise supporting it.
                assert_eq!(
                    path.can_equal(
                        &equality,
                        U256::from(u8::from(!equal)),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::ProvenImpossible,
                    "local"
                );
                assert_eq!(
                    path.assume(&equality, !equal, &limits, product_algebra().borrowed()),
                    CheckResult::ProvenImpossible
                );
            }
        }
    }
}

#[test]
fn delayed_input_key_aliases_refine_a_read_with_earlier_numeric_observations() {
    {
        let limits = policy();
        let length = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
        let k = ExprId::word_read(
            ByteView::new(input.clone(), c(0), c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        let q = ExprId::word_read(
            ByteView::new(input, c(32), c(32)),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        let storage = SlotStateId::zero()
            .store(k.clone(), c(7), limits.expressions().into())
            .unwrap()
            .store(q.clone(), c(9), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(storage, k.clone(), limits.expressions()).unwrap();
        let equality = op(opcode::EQ, &[k.clone(), q.clone()], limits);
        for equal in [false, true] {
            let mut path = RelationState::default();
            assert_eq!(
                path.add_unsigned_bounds(
                    &length,
                    U256::from(64),
                    U256::from(1024),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            path.observe(
                &read,
                crate::domain::ValueAlgebra::new(ProductAlgebra::default())
                    .join(
                        &EvmValue::<NumericValue>::constant(U256::from(7)),
                        &EvmValue::<NumericValue>::constant(U256::from(9)),
                    )
                    .numeric(),
                &limits,
            )
            .unwrap();
            let assumption = if equal {
                path.add_equal(&k, &q, &limits, product_algebra().borrowed())
            } else {
                path.assume(&equality, false, &limits, product_algebra().borrowed())
            };
            assert_eq!(assumption, CheckResult::MayBeFeasible, "local");
            assert_eq!(
                path.local_value(&read, &limits, product_algebra().borrowed()),
                EvmValueQuery::Exact(U256::from(if equal { 9 } else { 7 })),
                "local, equal={equal}"
            );
            assert_eq!(
                path.can_equal(
                    &read,
                    U256::from(if equal { 7 } else { 9 }),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::ProvenImpossible,
                "local"
            );
        }
    }
}

#[test]
fn asserted_input_length_bounds_preserve_the_last_byte_padding_boundary() {
    {
        let limits = policy();
        for start in [0, 32] {
            let length = ExprId::fresh().unwrap();
            let input = ByteStateId::observed(
                SourceId::fresh().unwrap(),
                length.clone(),
                (start..start + 32)
                    .map(|index| StateBinding {
                        index: U256::from(index),
                        value: c(1),
                        numeric: NumericValue::constant(U256::from(1)).into(),
                    })
                    .collect(),
                limits.expressions().into(),
            )
            .unwrap();
            let word = ExprId::word_read(
                ByteView::new(input, c(start), c(32)),
                c(0),
                limits.expressions(),
            )
            .unwrap();
            let mut path = RelationState::default();
            assert_eq!(
                path.add_unsigned_bounds(
                    &length,
                    U256::from(start + 31),
                    U256::from(start + 32),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            let full = U256::from_be_bytes([1; 32]);
            for expected in [full, full - U256::from(1)] {
                assert_eq!(
                    path.can_equal(&word, expected, &limits, product_algebra().borrowed()),
                    CheckResult::MayBeFeasible,
                    "local, start={start}"
                );
            }
            let mut covered = RelationState::default();
            assert_eq!(
                covered.add_unsigned_bounds(
                    &length,
                    U256::from(start + 32),
                    U256::from(start + 64),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                covered.local_value(&word, &limits, product_algebra().borrowed()),
                EvmValueQuery::Exact(full),
                "local, start={start}"
            );
        }
    }
}

#[test]
fn retained_inconsistent_equalities_and_ranges_cannot_produce_a_folded_value() {
    {
        let limits = policy();
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let storage = SlotStateId::zero()
            .store(k.clone(), c(7), limits.expressions().into())
            .unwrap()
            .store(q.clone(), c(9), limits.expressions().into())
            .unwrap();
        let read = ExprId::slot_read(storage, k.clone(), limits.expressions()).unwrap();
        // Retained constraints can await checking after imports or joins.
        // Exercise retained local constraints before eager insertion marks bottom.
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
            equal.local_value(&read, &limits, product_algebra().borrowed()),
            EvmValueQuery::ProvenImpossible,
            "local"
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
            range.local_value(&hash, &limits, product_algebra().borrowed()),
            EvmValueQuery::ProvenImpossible,
            "local"
        );
    }
}

#[test]
fn guarded_path_alias_facts_never_become_unconditional_read_rewrites() {
    {
        let limits = policy();
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
        assert_eq!(
            same.assume(&equality, true, &limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            different.assume(&equality, false, &limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        let joined = same
            .join_guarded(&different, selector.clone(), &limits)
            .unwrap();
        super::bounds_include(
            joined.local_value(&read, &limits, product_algebra().borrowed()),
            &[7, 9],
        );
        for (branch, expected) in [(true, 9), (false, 7)] {
            assert_eq!(
                joined.can_equal(
                    &read,
                    U256::from(expected),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible,
                "local, both guarded valuations survive"
            );
            let mut selected = joined.clone();
            assert_eq!(
                selected.assume(&selector, branch, &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                selected.local_value(&read, &limits, product_algebra().borrowed()),
                EvmValueQuery::Exact(U256::from(expected)),
                "local, branch={branch}"
            );
        }
    }
}

#[test]
fn bounded_hash_rewrites_retain_overlap_overflow_and_partial_copy_alternatives() {
    {
        let limits = policy();
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
            path.add_unsigned_bounds(
                &length,
                U256::from(512),
                U256::from(2048),
                &limits,
                product_algebra().borrowed()
            ),
            CheckResult::MayBeFeasible
        );
        // The same admitted length range has both a disjoint boundary and an
        // overlapping copy. A bound that straddles the guard cannot fold it.
        for (sample, disjoint) in [(1024, true), (1025, false)] {
            let mut boundary = path.clone();
            assert_eq!(
                boundary.add_equal(&length, &c(sample), &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            if disjoint {
                assert_eq!(
                    boundary.local_value(&equality, &limits, product_algebra().borrowed()),
                    EvmValueQuery::Exact(U256::from(1))
                );
            } else {
                assert_eq!(
                    boundary.can_equal(
                        &equality,
                        U256::ZERO,
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible
                );
                assert_eq!(
                    boundary.can_equal(
                        &equality,
                        U256::from(1),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible
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
                    path.can_equal(
                        &equality,
                        U256::from(expected),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible,
                    "local, invalid whole range or partial source copy; collisions remain possible"
                );
            }
        }
    }
}
