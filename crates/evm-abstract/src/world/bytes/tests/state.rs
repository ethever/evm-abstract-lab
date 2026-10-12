//! Byte-state wrapper regressions: content, policy, and summary identity.

use super::super::*;
use crate::domain::{
    relational::{RelationLimits, RelationState, ValueQuery},
    symbolic::{ExprKind, StateLimits},
};

fn v(n: usize) -> AbstractValue {
    constant(n)
}
fn symbolic(lo: usize, hi: usize) -> AbstractValue {
    AbstractValue::unsigned_range(U256::from(lo), U256::from(hi))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap())
}
fn prove(value: &AbstractValue, expected: U256) {
    if let Some(v) = value.singleton() {
        assert_eq!(v, expected);
        return;
    }
    assert_eq!(
        RelationState::default().unique_value(
            value.expression().unwrap(),
            &RelationLimits {
                max_nodes: 100_000,
                max_depth: 256,
                rlimit: 1_000_000,
                ..RelationLimits::default()
            }
        ),
        ValueQuery::Unique(expected)
    );
}
#[test]
fn unenumerated_copy_retains_input_length_and_complete_copy_hash() {
    let domain = Domain::default();
    let mut input = ByteArray::unknown();
    input.bind_input(410).unwrap();
    let size = input.len().clone();
    assert!(size.constants().is_none());
    let hash = input.keccak(domain, 64).unwrap();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(&v(0), &input, &v(0), &size, 1024 * 1024, domain)
        .unwrap();
    assert!(
        matches!(memory.state_root().kind(),ByteStateKind::Copy {length,..} if Some(length)==size.expression())
    );
    assert!(memory.len().expression().is_some());
    assert!(memory.work_size() < 50_000);
    let copied = memory.slice(&v(0), &size, 1024 * 1024, domain).unwrap();
    assert_eq!(copied.len().expression(), size.expression());
    assert_eq!(
        copied.keccak(domain, 64).unwrap().expression(),
        hash.expression()
    );
}
#[test]
fn concrete_byte_order_partial_unaligned_and_history() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let value = AbstractValue::constant(U256::from_be_bytes([0x12; 32]));
    memory.write_word(&v(3), &value, 64, domain).unwrap();
    let old = memory.read_word(&v(3), domain);
    memory.write_byte(&v(34), &v(0xaa), 64, domain).unwrap();
    assert_eq!(old.singleton(), value.singleton());
    assert_eq!(
        memory.read_word(&v(3), domain).singleton(),
        Some((value.singleton().unwrap() & !U256::from(255)) | U256::from(0xaa))
    );
    assert_eq!(memory.byte_at(2, domain).singleton(), Some(U256::ZERO));
    assert_eq!(
        memory.read_word(&v(4), domain).singleton(),
        Some((U256::from_be_bytes([0x12; 32]) << 8 & !U256::from(0xffff)) | U256::from(0xaa00))
    );
}
#[test]
fn mcopy_reads_old_source_snapshot() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &ByteArray::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            32,
            domain,
        )
        .unwrap();
    let old = memory.clone();
    memory
        .copy_from(&v(1), &old, &v(0), &v(3), 32, domain)
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), 32, domain)
            .unwrap()
            .exact_bytes(),
        Some(vec![1, 1, 2, 3])
    );
}
#[test]
fn source_overflow_pads_without_wrapping_or_target_cap() {
    let domain = Domain::default();
    let input = ByteArray::exact(&[0xaa, 0xbb]);
    let sliced = input
        .slice(&AbstractValue::constant(U256::MAX), &v(2), 2, domain)
        .unwrap();
    assert_eq!(sliced.exact_bytes(), Some(vec![0, 0]));
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &input,
            &AbstractValue::constant(U256::MAX),
            &v(2),
            32,
            domain,
        )
        .unwrap();
    assert_eq!(
        memory.read_word(&v(0), domain).singleton(),
        Some(U256::ZERO)
    );
}
#[test]
fn symbolic_msize_and_zero_length_huge_offset() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let original = memory.clone();
    memory
        .expand(&AbstractValue::constant(U256::MAX), &v(0), 64, domain)
        .unwrap();
    assert_eq!(memory, original);
    let size = symbolic(0, 33);
    memory
        .copy_from(&v(1), &ByteArray::exact(&[0xaa]), &v(0), &size, 64, domain)
        .unwrap();
    let expression = memory.len().expression().unwrap();
    let mut relations = RelationState::default();
    let policy = RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    };
    relations
        .observe(size.expression().unwrap(), v(33).numeric(), &policy)
        .unwrap();
    assert_eq!(
        relations.unique_value(expression, &policy),
        ValueQuery::Unique(U256::from(64))
    );
}
#[test]
fn return_copy_preserves_suffix_and_symbolic_minimum() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &ByteArray::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            64,
            domain,
        )
        .unwrap();
    memory
        .copy_return_data(&v(1), &ByteArray::exact(&[9]), &v(3), 64, domain)
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), 64, domain)
            .unwrap()
            .exact_bytes(),
        Some(vec![1, 9, 3, 4])
    );
    let ret = ByteArray::unknown()
        .slice(&v(0), &symbolic(0, 1), 64, domain)
        .unwrap();
    memory
        .copy_return_data(&v(1), &ret, &v(3), 64, domain)
        .unwrap();
    assert_eq!(memory.byte_at(2, domain).singleton(), Some(U256::from(3)));
}
#[test]
fn public_default_covers_symbolic_copy_and_slice() {
    let domain = Domain::default();
    let source = ByteArray::exact(&[0xaa]);
    let offset = symbolic(0, 31);
    let mut memory = ByteArray::memory();
    memory
        .copy_from(&offset, &source, &v(0), &v(1), 32, domain)
        .unwrap();
    assert!(memory.default_byte().contains(U256::from(0xaa)));
    assert!(memory.stored_bytes().next().is_none());
    let sliced = source.slice(&offset, &v(1), 32, domain).unwrap();
    assert!(sliced.default_byte().contains(U256::from(0xaa)));
    assert!(
        memory
            .project(domain)
            .default_byte()
            .contains(U256::from(0xaa))
    );
}
#[test]
fn join_same_root_different_operand_bounds_covers_both() {
    let domain = Domain::default();
    let token = ExprId::fresh().unwrap();
    let a = symbolic(7, 8).with_expression(token.clone());
    let b = symbolic(9, 10).with_expression(token);
    let mut left = ByteArray::memory();
    let mut right = ByteArray::memory();
    left.write_word(&v(0), &a, 32, domain).unwrap();
    right.write_word(&v(0), &b, 32, domain).unwrap();
    assert_eq!(left.root, right.root);
    let joined = left.join(&right, domain);
    for n in 7..=10 {
        assert!(joined.read_word(&v(0), domain).contains(U256::from(n)));
    }
}
#[test]
fn guarded_join_keeps_pointer_state_pairing() {
    let domain = Domain::default();
    let selector = ExprId::fresh().unwrap();
    let mut left = ByteArray::memory();
    let mut right = ByteArray::memory();
    left.write_word(&v(32), &v(7), 128, domain).unwrap();
    right.write_word(&v(64), &v(7), 128, domain).unwrap();
    let joined = left
        .join_guarded(&right, domain, selector.clone(), ExprLimits::default())
        .unwrap();
    let pointer = v(32)
        .join_guarded(&v(64), domain, selector, ExprLimits::default())
        .unwrap();
    prove(&joined.read_word(&pointer, domain), U256::from(7));
}
#[test]
fn fresh_rename_includes_sources_and_preserves_history() {
    let domain = Domain::default();
    let source = ByteArray::unknown();
    let old = source.read_word(&v(0), domain);
    let mut copied = source.slice(&v(1), &symbolic(1, 33), 64, domain).unwrap();
    let map = copied
        .fresh_leaves()
        .into_iter()
        .map(|e| (e, ExprId::fresh().unwrap()))
        .collect::<BTreeMap<_, _>>();
    assert!(map.len() >= 3);
    copied.rename_fresh(&map, ExprLimits::default()).unwrap();
    assert!(
        copied
            .fresh_leaves()
            .is_disjoint(&map.keys().cloned().collect())
    );
    assert_eq!(
        source.read_word(&v(0), domain).expression(),
        old.expression()
    );
}
#[test]
fn repeated_overwrite_and_zero_copy_are_idempotent() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory.write_word(&v(0), &v(7), 32, domain).unwrap();
    let old = memory.clone();
    for _ in 0..20 {
        memory.write_word(&v(0), &v(7), 32, domain).unwrap();
    }
    assert_eq!(memory, old);
    assert_eq!(memory.join(&memory, domain), memory);
    memory
        .copy_from(
            &AbstractValue::constant(U256::MAX),
            &ByteArray::unknown(),
            &v(0),
            &v(0),
            32,
            domain,
        )
        .unwrap();
    assert_eq!(memory, old);
}
#[test]
fn materialization_policy_is_separate_from_logical_copy_size() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &ByteArray::exact(&[1]),
            &v(0),
            &v(1024 * 1024),
            1024 * 1024,
            domain,
        )
        .unwrap();
    assert!(memory.work_size() < 50_000);
    assert!(memory.exact_bytes_bounded(32).is_none());
    assert_eq!(
        memory.byte_at(1024 * 1024 - 1, domain).singleton(),
        Some(U256::ZERO)
    );
}
#[test]
fn project_widen_keep_length_view_and_hash() {
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        enabled: false,
        ..RelationLimits::default()
    }));
    let old = ByteArray::unknown();
    let mut array = old
        .slice(&v(0), &symbolic(1, 33), 64, Domain::default())
        .unwrap();
    array.widen(&old, domain);
    let projected = array.project(domain);
    assert!(projected.len().expression().is_some());
    let _ = projected.view();
    assert!(projected.keccak(domain, 1).is_ok());
    assert!(projected.slice(&v(0), &v(1), 64, domain).is_ok());
}
#[test]
fn checked_budget_failure_is_atomic_and_visible() {
    let domain = Domain::default();
    let left = ByteArray::exact(&[1]);
    let right = ByteArray::exact(&[2]);
    assert!(matches!(
        left.join_guarded(
            &right,
            domain,
            ExprId::fresh().unwrap(),
            ExprLimits {
                max_nodes: 1,
                max_depth: 1
            }
        ),
        Err(RangeError::State(StateError::Limit { .. }))
            | Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    assert_eq!(left.exact_bytes(), Some(vec![1]));
    assert_eq!(
        StateLimits::default().max_nodes,
        ExprLimits::default().max_nodes
    );
}
#[test]
fn unbounded_symbolic_word_numeric_respects_nonwrapping_reads() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let offset = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    memory.write_word(&offset, &v(7), 64, domain).unwrap();
    let value = memory.read_word(&offset, domain);
    assert!(value.contains(U256::ZERO));
    assert!(value.contains(U256::from(7)));
    assert!(matches!(
        value.expression().unwrap().kind(),
        ExprKind::WordRead { .. }
    ));
}

#[test]
fn existing_composed_or_scoped_input_binding_keeps_source_identity() {
    let mut input = ByteArray::unknown();
    input.bind_input(700).unwrap();
    let root = input.root.clone();
    input.bind_input(701).unwrap();
    assert_eq!(input.root, root);
    let mut view = input.slice(&v(1), &v(32), 64, Domain::default()).unwrap();
    let root = view.root.clone();
    view.bind_input(702).unwrap();
    assert_eq!(view.root, root);
}
#[test]
fn copy_large_exact_source_does_not_duplicate_all_sparse_facts() {
    let source = ByteArray::exact(&vec![0xaa; 4096]);
    let mut memory = ByteArray::memory();
    memory
        .copy_from(&v(0), &source, &v(0), &v(4096), 4096, Domain::default())
        .unwrap();
    assert!(memory.stored_bytes().count() <= COPY_FACT_LIMIT);
    assert!(memory.default_byte().contains(U256::from(0xaa)));
    assert_eq!(
        memory.byte_at(4095, Domain::default()).singleton(),
        Some(U256::from(0xaa))
    );
}

#[test]
fn checked_self_join_respects_budget_and_cancellation() {
    let array = ByteArray::exact(&[1]);
    let guard = ExprId::constant(U256::ZERO);
    assert!(
        array
            .join_guarded(
                &array,
                Domain::default(),
                guard.clone(),
                ExprLimits {
                    max_nodes: 0,
                    max_depth: 0
                }
            )
            .is_err()
    );
    let cancel = embedded_smt::Cancellation::new();
    cancel.cancel();
    embedded_smt::with_cancellation(&cancel, || {
        assert!(matches!(
            array.join_guarded(&array, Domain::default(), guard, ExprLimits::default()),
            Err(RangeError::State(StateError::Cancelled))
        ));
    });
}

#[test]
fn ordinary_union_does_not_absorb_branch_under_an_incompatible_old_guard() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let mut left = ByteArray::memory();
    let mut right = ByteArray::memory();
    left.write_word(&v(0), &v(7), 32, domain).unwrap();
    right.write_word(&v(0), &v(9), 32, domain).unwrap();
    let paired = left
        .join_guarded(&right, domain, guard.clone(), ExprLimits::default())
        .unwrap();
    let union = paired.join(&left, domain);
    let value = union.read_word(&v(0), domain);
    let p = RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    };
    let mut relations = RelationState::default();
    relations.observe(&guard, v(0).numeric(), &p).unwrap();
    let eq = ExprId::operation(
        opcode::EQ,
        &[
            value.expression().unwrap().clone(),
            ExprId::constant(U256::from(7)),
        ],
        p.expressions(),
    )
    .unwrap();
    assert_eq!(
        relations.assume(&eq, true, &p),
        crate::domain::relational::CheckResult::Sat
    );
}

#[test]
fn repeated_symbolic_range_writes_and_copy_keep_msize_stable() {
    let domain = Domain::default();
    let offset = symbolic(0, 64);
    let mut memory = ByteArray::memory();
    memory.write_word(&offset, &v(7), 96, domain).unwrap();
    let old = memory.clone();
    for _ in 0..40 {
        memory.write_word(&offset, &v(7), 96, domain).unwrap();
    }
    assert_eq!(memory, old);
    let source = ByteArray::unknown();
    let size = symbolic(0, 64);
    let mut memory = ByteArray::memory();
    memory
        .copy_from(&v(0), &source, &v(0), &size, 64, domain)
        .unwrap();
    let old = memory.clone();
    for _ in 0..40 {
        memory
            .copy_from(&v(0), &source, &v(0), &size, 64, domain)
            .unwrap();
    }
    assert_eq!(memory, old);
}
#[test]
fn allocation_witnesses_are_must_facts_across_join_and_rename() {
    let domain = Domain::default();
    let range = symbolic(0, 32);
    let mut a = ByteArray::memory();
    a.expand(&v(0), &range, 32, domain).unwrap();
    let b = ByteArray::memory();
    let joined = a
        .join_guarded(&b, domain, ExprId::fresh().unwrap(), ExprLimits::default())
        .unwrap();
    assert!(joined.allocations.is_empty());
    let map = a
        .fresh_leaves()
        .into_iter()
        .map(|e| (e, ExprId::fresh().unwrap()))
        .collect::<BTreeMap<_, _>>();
    a.rename_fresh(&map, ExprLimits::default()).unwrap();
    assert!(!a.allocations.iter().any(|(_, len)| map.contains_key(len)));
}

#[test]
fn numeric_only_write_offsets_constrain_the_state_read_expression() {
    let domain = Domain::default();
    let offset = domain.join(&v(0), &v(1));
    let mut memory = ByteArray::memory();
    memory.write_byte(&offset, &v(7), 32, domain).unwrap();
    let read = memory.read_byte(&v(2), domain);
    assert_eq!(read.singleton(), Some(U256::ZERO));
    assert_eq!(
        RelationState::default().unique_value(
            read.expression().unwrap(),
            &RelationLimits {
                max_nodes: 100_000,
                max_depth: 256,
                rlimit: 1_000_000,
                ..RelationLimits::default()
            }
        ),
        ValueQuery::Unique(U256::ZERO)
    );
}

#[test]
fn copies_at_distinct_addresses_hash_to_the_same_storage_key_under_range_guard() {
    use crate::world::{Account, Store, World};
    let domain = Domain::default();
    let mut input = ByteArray::unknown();
    input.bind_input(810).unwrap();
    let size = input.len().clone().with_numeric(
        crate::domain::NumericValue::unsigned_range(U256::ZERO, U256::from(64)).unwrap(),
    );
    let mut a = ByteArray::memory();
    let mut b = ByteArray::memory();
    a.copy_from(&v(32), &input, &v(0), &size, 128, domain)
        .unwrap();
    b.copy_from(&v(64), &input, &v(0), &size, 128, domain)
        .unwrap();
    let ka = a
        .slice(&v(32), &size, 128, domain)
        .unwrap()
        .keccak(domain, 0)
        .unwrap();
    let kb = b
        .slice(&v(64), &size, 128, domain)
        .unwrap()
        .keccak(domain, 0)
        .unwrap();
    let address = alloy_primitives::Address::repeat_byte(0x83);
    let mut world = World::new(crate::Fork::Osaka, "copy hash slot");
    world.insert(address, Account::empty()).unwrap();
    let mut store = Store::new(&world);
    store.write_checked(address, &ka, &v(7), domain).unwrap();
    let read = store.read(address, &kb, domain);
    let p = RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    };
    let mut relations = RelationState::default();
    relations
        .observe(size.expression().unwrap(), size.numeric(), &p)
        .unwrap();
    assert_eq!(
        relations.unique_value(read.expression().unwrap(), &p),
        ValueQuery::Unique(U256::from(7))
    );
}
