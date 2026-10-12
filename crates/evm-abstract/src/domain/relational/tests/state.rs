//! These are real native-solver proofs, beyond constructor-only normalization.
use super::super::{
    CheckResult, RelationLimits, RelationState, ScalarQuery, SmtProvider, ValueQuery,
};
use crate::domain::{AbstractValue, Domain, NumericValue, identity::Symbol, symbolic::*};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::sync::Arc;
fn c(v: u64) -> ExprId {
    ExprId::constant(U256::from(v))
}
fn expr_limits() -> ExprLimits {
    ExprLimits {
        max_nodes: 32_768,
        max_depth: 128,
    }
}
fn state_limits() -> StateLimits {
    expr_limits().into()
}
fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        max_nodes: 32_768,
        max_depth: 128,
        rlimit: 10_000_000,
        ..RelationLimits::default()
    }
}
fn op(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, expr_limits()).unwrap()
}
fn word(state: ByteStateId, at: ExprId) -> ExprId {
    ExprId::word_read(ByteView::new(state, at, c(32)), c(0), expr_limits()).unwrap()
}

#[test]
fn native_slot_arrays_preserve_aliasing_mixed_write_order_and_history() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let initial = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            vec![StateBinding {
                index: U256::from(3),
                value: c(11),
                numeric: NumericValue::constant(U256::from(11)),
            }],
            state_limits(),
        )
        .unwrap();
        let a = initial.store(k.clone(), c(7), state_limits()).unwrap();
        let historical = ExprId::slot_read(a.clone(), k.clone(), expr_limits()).unwrap();
        let latest = a
            .store(c(3), c(5), state_limits())
            .unwrap()
            .store(q.clone(), c(9), state_limits())
            .unwrap();
        let read = ExprId::slot_read(latest, k.clone(), expr_limits()).unwrap();
        let eq = op(opcode::EQ, &[q.clone(), k.clone()]);
        let mut same = RelationState::default();
        assert_eq!(same.assume(&eq, true, &limits), CheckResult::Sat);
        assert_eq!(
            same.unique_value(&read, &limits),
            ValueQuery::Unique(U256::from(9)),
            "{provider:?}"
        );
        let mut different = RelationState::default();
        assert_eq!(different.assume(&eq, false, &limits), CheckResult::Sat);
        assert_eq!(
            different.assume(&op(opcode::EQ, &[k.clone(), c(3)]), false, &limits),
            CheckResult::Sat
        );
        assert_eq!(
            different.unique_value(&read, &limits),
            ValueQuery::Unique(U256::from(7)),
            "{provider:?}"
        );
        assert_eq!(
            same.unique_value(&historical, &limits),
            ValueQuery::Unique(U256::from(7))
        );
        assert_eq!(
            RelationState::default().unique_value(&read, &limits),
            ValueQuery::Multiple
        );
    }
}
#[test]
fn repeated_writes_preserve_intervening_symbolic_aliases_and_snapshots() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let k = ExprId::fresh().unwrap();
        let q = ExprId::fresh().unwrap();
        let snapshot = SlotStateId::zero()
            .store(k.clone(), c(7), state_limits())
            .unwrap()
            .store(q.clone(), c(9), state_limits())
            .unwrap();
        let latest = snapshot.store(k.clone(), c(7), state_limits()).unwrap();
        assert_ne!(latest, snapshot);
        let read = ExprId::slot_read(latest, q.clone(), expr_limits()).unwrap();
        let old = ExprId::slot_read(snapshot, q.clone(), expr_limits()).unwrap();
        for (alias, expected) in [(true, 7), (false, 9)] {
            let mut path = RelationState::default();
            assert_eq!(
                path.assume(&op(opcode::EQ, &[k.clone(), q.clone()]), alias, &limits),
                CheckResult::Sat
            );
            assert_eq!(
                path.unique_value(&read, &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}"
            );
            assert_eq!(
                path.unique_value(&old, &limits),
                ValueQuery::Unique(U256::from(9))
            );
        }
        let p = ExprId::fresh().unwrap();
        let snapshot = ByteStateId::zero()
            .write_word(c(0), c(7), state_limits())
            .unwrap()
            .write_word(p.clone(), c(9), state_limits())
            .unwrap();
        let latest = snapshot.write_word(c(0), c(7), state_limits()).unwrap();
        assert_ne!(latest, snapshot);
        let read = word(latest, p.clone());
        let old = word(snapshot, p.clone());
        for (at, expected) in [(0, 7), (32, 9)] {
            let mut path = RelationState::default();
            assert_eq!(path.add_equal(&p, &c(at), &limits), CheckResult::Sat);
            assert_eq!(
                path.unique_value(&read, &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}"
            );
            assert_eq!(
                path.unique_value(&old, &limits),
                ValueQuery::Unique(U256::from(9))
            );
        }
    }
}
#[test]
fn changed_disjoint_writes_preserve_other_cells_and_historical_values() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let slots = SlotStateId::zero()
            .store(c(0), c(7), state_limits())
            .unwrap()
            .store(c(1), c(9), state_limits())
            .unwrap();
        let changed_slots = slots.store(c(0), c(11), state_limits()).unwrap();
        let words = ByteStateId::zero()
            .write_word(c(0), c(7), state_limits())
            .unwrap()
            .write_word(c(32), c(9), state_limits())
            .unwrap();
        let changed_words = words.write_word(c(0), c(11), state_limits()).unwrap();
        for (read, expected) in [
            (ExprId::slot_read(slots, c(0), expr_limits()).unwrap(), 7),
            (
                ExprId::slot_read(changed_slots.clone(), c(0), expr_limits()).unwrap(),
                11,
            ),
            (
                ExprId::slot_read(changed_slots, c(1), expr_limits()).unwrap(),
                9,
            ),
            (word(words, c(0)), 7),
            (word(changed_words.clone(), c(0)), 11),
            (word(changed_words, c(32)), 9),
        ] {
            assert_eq!(
                RelationState::default().unique_value(&read, &limits),
                ValueQuery::Unique(U256::from(expected)),
                "{provider:?}"
            );
        }
    }
}
#[test]
fn native_byte_reads_keep_big_endian_partial_overlay_and_symbolic_aliases() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let p = ExprId::fresh().unwrap();
        let x = ExprId::fresh().unwrap();
        let memory = ByteStateId::zero()
            .write_word(p.clone(), x.clone(), state_limits())
            .unwrap();
        let old = word(memory.clone(), p.clone());
        let last = op(opcode::ADD, &[p.clone(), c(31)]);
        let memory = memory.write_byte(last, c(0xaa), state_limits()).unwrap();
        let latest = word(memory, p.clone());
        let mut path = RelationState::default();
        assert_eq!(
            path.add_unsigned_bounds(&p, U256::ZERO, U256::from(4096), &limits),
            CheckResult::Sat
        );
        let expected = op(
            opcode::OR,
            &[
                op(
                    opcode::AND,
                    &[x.clone(), ExprId::constant(U256::MAX - U256::from(255))],
                ),
                c(0xaa),
            ],
        );
        assert_eq!(
            path.unique_value(&op(opcode::EQ, &[latest, expected]), &limits),
            ValueQuery::Unique(U256::from(1)),
            "{provider:?}"
        );
        assert_eq!(
            path.unique_value(&op(opcode::EQ, &[old, x]), &limits),
            ValueQuery::Unique(U256::from(1)),
            "{provider:?}"
        );
    }
}
#[test]
fn conditional_pointer_and_state_are_paired_by_one_guard() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let g = ExprId::fresh().unwrap();
        let left = ByteStateId::zero()
            .write_word(c(128), c(7), state_limits())
            .unwrap();
        let right = ByteStateId::zero()
            .write_word(c(192), c(7), state_limits())
            .unwrap();
        let state = left
            .join_guarded(&right, g.clone(), state_limits())
            .unwrap();
        let p = ExprId::ite(g, c(128), c(192), expr_limits()).unwrap();
        let value = word(state, p);
        assert!(value.has_state_semantics());
        assert_eq!(
            RelationState::default().unique_value(&value, &limits),
            ValueQuery::Unique(U256::from(7)),
            "{provider:?}"
        );
    }
}
#[test]
fn guarded_path_join_retains_two_formulas_and_summary_dependencies() {
    let limits = policy(SmtProvider::Z3);
    let x = ExprId::input(8, Symbol::CallValue);
    let g = ExprId::fresh().unwrap();
    let mut a = RelationState::default();
    let mut b = RelationState::default();
    assert_eq!(a.add_equal(&x, &c(7), &limits), CheckResult::Sat);
    assert_eq!(b.add_equal(&x, &c(9), &limits), CheckResult::Sat);
    let path = a.join_guarded(&b, g.clone(), &limits).unwrap();
    assert!(!path.is_empty());
    assert!(path.relevant(&x));
    assert!(path.fresh_leaves().contains(&g));
    assert_eq!(path.unique_value(&x, &limits), ValueQuery::Multiple);
    let expected = ExprId::ite(g.clone(), c(7), c(9), expr_limits()).unwrap();
    assert_eq!(
        path.unique_value(&op(opcode::EQ, &[x.clone(), expected]), &limits),
        ValueQuery::Unique(U256::from(1))
    );
    let mut left = path.clone();
    assert_eq!(left.assume(&g, true, &limits), CheckResult::Sat);
    assert_eq!(
        left.unique_value(&x, &limits),
        ValueQuery::Unique(U256::from(7))
    );
    assert_eq!(
        path,
        path.join_guarded(&path, ExprId::fresh().unwrap(), &limits)
            .unwrap()
    );
    assert!(matches!(
        path.refine_numeric(&x, &NumericValue::top(), Domain::default(), &limits),
        ScalarQuery::Unchanged
    ));
}
#[test]
fn copy_word_constraints_forward_without_byte_expansion_or_source_rebinding() {
    let limits = policy(SmtProvider::Z3);
    let length = ExprId::input(12, Symbol::CalldataLength);
    let source = ByteStateId::input(SourceId::input(12, Symbol::CalldataLength), length.clone());
    let src = ByteView::new(source.clone(), c(0), length.clone());
    let memory = ByteStateId::zero()
        .copy(c(128), src, length.clone(), state_limits())
        .unwrap();
    let child = ByteView::new(memory, c(128), length.clone());
    let parent_read = word(source, c(0));
    let child_read = ExprId::word_read(child, c(0), expr_limits()).unwrap();
    let mut path = RelationState::default();
    assert_eq!(
        path.add_unsigned_bounds(&length, U256::from(32), U256::from(1024), &limits),
        CheckResult::Sat
    );
    assert_eq!(
        path.add_equal(&parent_read, &c(7), &limits),
        CheckResult::Sat
    );
    assert_eq!(
        path.unique_value(&child_read, &limits),
        ValueQuery::Unique(U256::from(7))
    );
}
#[test]
fn full_symbolic_length_copies_hash_equally_and_alias_storage_under_validity_guards() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::input(19, Symbol::CalldataLength);
        let source =
            ByteStateId::input(SourceId::input(19, Symbol::CalldataLength), length.clone());
        let view = ByteView::new(source, c(0), length.clone());
        let a = ByteStateId::zero()
            .copy(c(128), view.clone(), length.clone(), state_limits())
            .unwrap();
        let b = ByteStateId::zero()
            .copy(c(192), view, length.clone(), state_limits())
            .unwrap();
        let ha = ExprId::keccak(ByteView::new(a, c(128), length.clone()), expr_limits()).unwrap();
        let hb = ExprId::keccak(ByteView::new(b, c(192), length.clone()), expr_limits()).unwrap();
        let storage = SlotStateId::input(SourceId::fresh().unwrap())
            .store(ha, c(7), state_limits())
            .unwrap();
        let read = ExprId::slot_read(storage, hb, expr_limits()).unwrap();
        let mut path = RelationState::default();
        assert_eq!(
            path.add_unsigned_bounds(&length, U256::ZERO, U256::from(1_000_000), &limits),
            CheckResult::Sat
        );
        assert_eq!(
            path.unique_value(&read, &limits),
            ValueQuery::Unique(U256::from(7)),
            "{provider:?}"
        );
    }
}
#[test]
fn different_sources_and_hash_inputs_do_not_assume_equality_or_collision_freedom() {
    let limits = policy(SmtProvider::Z3);
    let a = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), c(100)),
            c(0),
            c(100),
        ),
        expr_limits(),
    )
    .unwrap();
    let b = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), c(100)),
            c(0),
            c(100),
        ),
        expr_limits(),
    )
    .unwrap();
    let eq = op(opcode::EQ, &[a, b]);
    let mut same = RelationState::default();
    let mut different = RelationState::default();
    assert_eq!(same.assume(&eq, true, &limits), CheckResult::Sat);
    assert_eq!(different.assume(&eq, false, &limits), CheckResult::Sat);
}
#[test]
fn concrete_initial_slots_and_byte_numeric_facts_constrain_actual_arrays() {
    let limits = policy(SmtProvider::Z3);
    let numeric = NumericValue::unsigned_range(U256::from(10), U256::from(20)).unwrap();
    let v = ExprId::fresh().unwrap();
    let slots = SlotStateId::observed(
        SourceId::fresh().unwrap(),
        vec![StateBinding {
            index: U256::from(3),
            value: v,
            numeric,
        }],
        state_limits(),
    )
    .unwrap();
    let read = ExprId::slot_read(slots, c(3), expr_limits()).unwrap();
    let path = RelationState::default();
    assert_eq!(
        path.can_equal(&read, U256::from(9), &limits),
        CheckResult::Unsat
    );
    assert_eq!(
        path.can_equal(&read, U256::from(15), &limits),
        CheckResult::Sat
    );
    let bytes = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        c(1),
        vec![StateBinding {
            index: U256::ZERO,
            value: ExprId::fresh().unwrap(),
            numeric: NumericValue::constant(U256::from(0xaa)),
        }],
        state_limits(),
    )
    .unwrap();
    let read = ExprId::byte_read(bytes.clone(), c(0), expr_limits()).unwrap();
    assert_eq!(
        path.unique_value(&read, &limits),
        ValueQuery::Unique(U256::from(0xaa))
    );
    let hash = ExprId::keccak(ByteView::new(bytes, c(0), c(1)), expr_limits()).unwrap();
    assert_eq!(
        path.unique_value(&hash, &limits),
        ValueQuery::Unique(U256::from_be_bytes(alloy_primitives::keccak256([0xaa]).0))
    );
}
#[test]
fn demand_reads_zero_pad_without_word256_index_wrap_and_keep_mcopy_snapshot() {
    let limits = policy(SmtProvider::Z3);
    let state = ByteStateId::zero()
        .write_byte(c(0), c(0xee), state_limits())
        .unwrap()
        .write_byte(ExprId::constant(U256::MAX), c(0xaa), state_limits())
        .unwrap();
    let value = word(state.clone(), ExprId::constant(U256::MAX));
    assert_eq!(
        RelationState::default().unique_value(&value, &limits),
        ValueQuery::Unique(U256::from(0xaa) << 248)
    );
    let source = ByteStateId::exact(Arc::from([1, 2, 3, 4]));
    let view = ByteView::new(source.clone(), c(0), c(3));
    let copied = source.copy(c(1), view, c(3), state_limits()).unwrap();
    let read = word(copied, c(0));
    let expected = U256::from_be_slice(&[
        1, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0,
    ]);
    assert_eq!(
        RelationState::default().unique_value(&read, &limits),
        ValueQuery::Unique(expected)
    );
}
#[test]
fn scalar_reduction_of_state_reads_is_not_reported_complete() {
    let limits = policy(SmtProvider::Z3);
    let k = ExprId::fresh().unwrap();
    let q = ExprId::fresh().unwrap();
    let slots = SlotStateId::zero().store(k, c(7), state_limits()).unwrap();
    let read = ExprId::slot_read(slots, q, expr_limits()).unwrap();
    let mut path = RelationState::default();
    path.observe(
        &read,
        AbstractValue::unsigned_range(U256::ZERO, U256::from(10))
            .unwrap()
            .numeric(),
        &limits,
    )
    .unwrap();
    assert!(matches!(
        path.refine_numeric(&read, &NumericValue::top(), Domain::default(), &limits),
        ScalarQuery::Refined {
            complete: false,
            ..
        }
    ));
}

#[test]
fn invalid_whole_copy_hash_reads_only_valid_destination_bytes() {
    let limits = policy(SmtProvider::Z3);
    let source = ByteView::new(ByteStateId::exact(Arc::from([0xaa, 0xbb])), c(0), c(2));
    let state = ByteStateId::zero()
        .copy(ExprId::constant(U256::MAX), source, c(2), state_limits())
        .unwrap();
    let hash = ExprId::keccak(
        ByteView::new(state, ExprId::constant(U256::MAX), c(2)),
        expr_limits(),
    )
    .unwrap();
    assert_eq!(
        RelationState::default().unique_value(&hash, &limits),
        ValueQuery::Unique(U256::from_be_bytes(
            alloy_primitives::keccak256([0xaa, 0]).0
        ))
    );
}
#[test]
fn newly_observed_source_content_binds_an_older_hash_application() {
    let limits = policy(SmtProvider::Z3);
    let source = SourceId::fresh().unwrap();
    let old = ByteStateId::input(source.clone(), c(2));
    let observed = ByteStateId::observed(
        source,
        c(2),
        vec![
            StateBinding {
                index: U256::ZERO,
                value: c(0xaa),
                numeric: NumericValue::constant(U256::from(0xaa)),
            },
            StateBinding {
                index: U256::from(1),
                value: c(0xbb),
                numeric: NumericValue::constant(U256::from(0xbb)),
            },
        ],
        state_limits(),
    )
    .unwrap();
    let oldhash = ExprId::keccak(ByteView::new(old, c(0), c(2)), expr_limits()).unwrap();
    let observedhash = ExprId::keccak(ByteView::new(observed, c(0), c(2)), expr_limits()).unwrap();
    let mut path = RelationState::default();
    assert_eq!(
        path.add_equal(
            &observedhash,
            &ExprId::constant(U256::from_be_bytes(
                alloy_primitives::keccak256([0xaa, 0xbb]).0
            )),
            &limits
        ),
        CheckResult::Sat
    );
    assert_eq!(
        path.unique_value(&oldhash, &limits),
        ValueQuery::Unique(U256::from_be_bytes(
            alloy_primitives::keccak256([0xaa, 0xbb]).0
        ))
    );
}

#[test]
fn scalar_ite_retains_solver_semantics_after_direct_numeric_projection() {
    let limits = policy(SmtProvider::Z3);
    let value = ExprId::ite(ExprId::fresh().unwrap(), c(128), c(192), expr_limits()).unwrap();
    let mut path = RelationState::default();
    path.observe(
        &value,
        &NumericValue::unsigned_range(U256::from(128), U256::from(192)).unwrap(),
        &limits,
    )
    .unwrap();
    assert!(matches!(
        path.refine_numeric(&value, &NumericValue::top(), Domain::default(), &limits),
        ScalarQuery::Refined {
            complete: false,
            ..
        }
    ));
    assert_eq!(
        path.assume(&op(opcode::EQ, &[value.clone(), c(192)]), false, &limits),
        CheckResult::Sat
    );
    assert_eq!(
        path.unique_value(&value, &limits),
        ValueQuery::Unique(U256::from(128))
    );
}

#[test]
fn normal_default_budget_proves_forwarding_and_query_limits_preserve_reachability() {
    let limits = RelationLimits::default();
    let length = ExprId::input(88, Symbol::CalldataLength);
    let source = ByteStateId::input(SourceId::input(88, Symbol::CalldataLength), length.clone());
    let memory = ByteStateId::zero()
        .copy(
            c(128),
            ByteView::new(source.clone(), c(0), length.clone()),
            length.clone(),
            StateLimits::default(),
        )
        .unwrap();
    let read = ExprId::word_read(
        ByteView::new(memory, c(128), length.clone()),
        c(0),
        ExprLimits::default(),
    )
    .unwrap();
    let original = ExprId::word_read(
        ByteView::new(source, c(0), length.clone()),
        c(0),
        ExprLimits::default(),
    )
    .unwrap();
    let mut path = RelationState::default();
    assert_eq!(
        path.add_unsigned_bounds(&length, U256::from(32), U256::from(1024), &limits),
        CheckResult::Sat
    );
    assert_eq!(path.add_equal(&original, &c(7), &limits), CheckResult::Sat);
    assert_eq!(
        path.unique_value(&read, &limits),
        ValueQuery::Unique(U256::from(7))
    );
    let denied = RelationLimits {
        max_nodes: 1,
        ..limits
    };
    assert_eq!(
        path.unique_value(&read, &denied),
        ValueQuery::Unknown(crate::domain::relational::QueryReason::ExpressionLimit)
    );
    assert!(!path.is_bottom());
    let token = embedded_smt::Cancellation::new();
    token.cancel();
    embedded_smt::with_cancellation(&token, || {
        assert_eq!(
            path.unique_value(&read, &limits),
            ValueQuery::Unknown(crate::domain::relational::QueryReason::Cancelled)
        );
        assert_eq!(
            path.join_guarded(&path, c(1), &limits),
            Err(crate::domain::relational::QueryReason::Cancelled)
        );
    });
    assert!(!path.is_bottom());
}

#[test]
fn changing_a_forwarded_input_offset_does_not_inherit_the_original_word() {
    let limits = policy(SmtProvider::Z3);
    let len = ExprId::input(93, Symbol::CalldataLength);
    let source = ByteStateId::input(SourceId::input(93, Symbol::CalldataLength), len.clone());
    let memory = ByteStateId::zero()
        .copy(
            c(128),
            ByteView::new(source.clone(), c(0), len.clone()),
            len.clone(),
            state_limits(),
        )
        .unwrap();
    let original = word(source, c(0));
    let shifted = ExprId::word_read(
        ByteView::new(memory, c(129), len.clone()),
        c(0),
        expr_limits(),
    )
    .unwrap();
    let mut path = RelationState::default();
    assert_eq!(
        path.add_unsigned_bounds(&len, U256::from(33), U256::from(128), &limits),
        CheckResult::Sat
    );
    assert_eq!(path.add_equal(&original, &c(7), &limits), CheckResult::Sat);
    assert_eq!(path.unique_value(&shifted, &limits), ValueQuery::Multiple);
    assert_eq!(
        path.can_equal(&shifted, U256::from(7), &limits),
        CheckResult::Unsat
    );
    assert_eq!(
        path.can_equal(&shifted, U256::from(0x7aa), &limits),
        CheckResult::Sat
    );
}
