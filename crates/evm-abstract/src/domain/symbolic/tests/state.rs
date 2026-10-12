use super::super::{
    ByteStateId, ByteView, ExprError, ExprId, ExprKind, ExprLimits, GraphLimits, HashSupport,
    SlotStateId, SourceId, StateError, StateLimits, SymbolicGraph, SymbolicGraphNode,
};
use crate::domain::{AbstractValue, Domain, identity::Symbol};
use alloy_primitives::U256;
use std::{collections::BTreeMap, sync::Arc};
fn c(v: u64) -> ExprId {
    ExprId::constant(U256::from(v))
}
fn limits() -> StateLimits {
    StateLimits::default()
}

#[test]
fn independent_unknown_sources_with_same_length_are_not_interned_together() {
    let len = ExprId::input(2, Symbol::CalldataLength);
    let a = ByteStateId::input(SourceId::fresh().unwrap(), len.clone());
    let b = ByteStateId::input(SourceId::fresh().unwrap(), len.clone());
    assert_ne!(a, b);
    let token = SourceId::input(2, Symbol::CalldataLength);
    assert_eq!(
        ByteStateId::input(token.clone(), len.clone()),
        ByteStateId::input(token, len)
    );
}
#[test]
fn byte_and_slot_updates_intern_and_stabilize_after_an_entry_join() {
    let zero = ByteStateId::zero();
    let first = zero.write_word(c(0), c(7), limits()).unwrap();
    assert_eq!(first, zero.write_word(c(0), c(7), limits()).unwrap());
    assert_eq!(first, first.write_word(c(0), c(7), limits()).unwrap());
    let overlaid = first.write_byte(c(31), c(0xaa), limits()).unwrap();
    let next = overlaid
        .write_word(c(0), c(7), limits())
        .unwrap()
        .write_byte(c(31), c(0xaa), limits())
        .unwrap();
    assert_eq!(overlaid, next);
    let entry = zero
        .join_guarded(&first, ExprId::fresh().unwrap(), limits())
        .unwrap();
    assert_eq!(first, entry.write_word(c(0), c(7), limits()).unwrap());
    assert_eq!(zero, zero.write_word(c(0), c(0), limits()).unwrap());
    let slots = SlotStateId::zero();
    let k = ExprId::fresh().unwrap();
    let value = slots.store(k.clone(), c(7), limits()).unwrap();
    assert_eq!(value, slots.store(k.clone(), c(7), limits()).unwrap());
    assert_eq!(value, value.store(k.clone(), c(7), limits()).unwrap());
    let entry = slots
        .join_guarded(&value, ExprId::fresh().unwrap(), limits())
        .unwrap();
    assert_eq!(value, entry.store(k, c(7), limits()).unwrap());
}
#[test]
fn very_large_copies_are_one_range_node_and_full_copies_canonicalize() {
    let len = ExprId::constant(U256::MAX);
    let source = ByteStateId::input(SourceId::fresh().unwrap(), len.clone());
    let view = ByteView::new(source, c(0), len.clone());
    let copied = ByteStateId::zero()
        .copy(c(0), view.clone(), len.clone(), limits())
        .unwrap();
    assert!(copied.nodes() < 20);
    assert_eq!(ByteView::new(copied, c(0), len).canonicalize(), view);
}
#[test]
fn views_do_not_normalize_overflowing_destination_ranges() {
    let source = ByteView::new(ByteStateId::exact(Arc::from([0xaa, 0xbb])), c(0), c(2));
    let copied = ByteStateId::zero()
        .copy(ExprId::constant(U256::MAX), source.clone(), c(2), limits())
        .unwrap();
    let view = ByteView::new(copied, ExprId::constant(U256::MAX), c(2));
    assert_ne!(view.canonicalize(), source);
    assert_eq!(
        view.known_bytes(ExprLimits::default()).unwrap(),
        Some(vec![0xaa, 0])
    );
    assert!(
        ExprId::keccak(view, ExprLimits::default())
            .unwrap()
            .has_state_semantics()
    );
}
#[test]
fn slicing_retains_the_parent_padding_and_checks_offset_addition() {
    let src = ByteView::new(ByteStateId::exact(Arc::from([1, 2, 3, 4])), c(1), c(1));
    let slice = src.slice(c(0), c(3), limits()).unwrap();
    assert_eq!(
        slice.known_bytes(ExprLimits::default()).unwrap(),
        Some(vec![2, 0, 0])
    );
    let enormous = ByteView::new(
        ByteStateId::exact(Arc::from([9])),
        ExprId::constant(U256::MAX),
        c(2),
    );
    assert_eq!(
        enormous.known_bytes(ExprLimits::default()).unwrap(),
        Some(vec![0, 0])
    );
}
#[test]
fn replay_renames_source_tokens_offsets_lengths_writes_and_branch_guards() {
    let token = SourceId::fresh().unwrap();
    let len = ExprId::fresh().unwrap();
    let offset = ExprId::fresh().unwrap();
    let value = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let source = ByteStateId::input(token.clone(), len.clone());
    let written = source
        .write_word(offset.clone(), value.clone(), limits())
        .unwrap();
    let joined = source
        .join_guarded(&written, guard.clone(), limits())
        .unwrap();
    let read = ExprId::word_read(
        ByteView::new(joined, c(0), len.clone()),
        offset.clone(),
        ExprLimits::default(),
    )
    .unwrap();
    let leaves = read.fresh_leaves();
    assert_eq!(leaves.len(), 5);
    assert!(leaves.contains(token.token()));
    let map: BTreeMap<_, _> = leaves
        .iter()
        .map(|e| (e.clone(), ExprId::fresh().unwrap()))
        .collect();
    let renamed = read.rename_fresh(&map, ExprLimits::default()).unwrap();
    assert_eq!(renamed.fresh_leaves(), map.values().cloned().collect());
    assert_eq!(read.fresh_leaves(), leaves);
    assert!(renamed.has_state_semantics());
}
#[test]
fn normalized_constructions_still_obey_resource_and_cancellation_policy() {
    let zero = ByteStateId::zero();
    let denied = StateLimits {
        max_nodes: 0,
        max_depth: 0,
    };
    assert!(zero.write_word(c(0), c(0), denied).is_err());
    assert!(
        zero.copy(c(0), ByteView::new(zero.clone(), c(0), c(0)), c(0), denied)
            .is_err()
    );
    assert!(zero.join_guarded(&zero, c(1), denied).is_err());
    let cancellation = embedded_smt::Cancellation::new();
    cancellation.cancel();
    embedded_smt::with_cancellation(&cancellation, || {
        assert_eq!(
            zero.write_byte(c(0), c(0), limits()),
            Err(StateError::Cancelled)
        );
    });
}
#[test]
fn flat_export_deduplicates_state_references_and_limits_exact_data() {
    let source = ByteStateId::input(SourceId::fresh().unwrap(), c(32));
    let read = ExprId::word_read(
        ByteView::new(source, c(0), c(32)),
        c(0),
        ExprLimits::default(),
    )
    .unwrap();
    let expression = ExprId::operation(
        revm_bytecode::opcode::ADD,
        &[read.clone(), read],
        ExprLimits::default(),
    )
    .unwrap();
    let graph = SymbolicGraph::expression(&expression, ExprLimits::default()).unwrap();
    assert_eq!(
        graph
            .nodes
            .iter()
            .filter(|n| matches!(n, SymbolicGraphNode::WordRead { .. }))
            .count(),
        1
    );
    let encoded = serde_json::to_value(&expression).unwrap();
    assert!(encoded["nodes"].as_array().unwrap().len() < 12);
    let large = ByteStateId::exact(Arc::from(vec![0u8; 24 * 1024]));
    assert!(SymbolicGraph::bytes(&large, ExprLimits::default()).is_ok());
    assert!(serde_json::to_string(&large).is_ok());
    assert!(matches!(
        SymbolicGraph::bytes(
            &large,
            GraphLimits {
                max_exact_bytes: 1024,
                ..GraphLimits::default()
            }
        ),
        Err(ExprError::ExportBytes { .. })
    ));
}
#[test]
fn guarded_value_join_keeps_numeric_union_and_scalar_pairing() {
    let g = ExprId::fresh().unwrap();
    let value = AbstractValue::constant(U256::from(128))
        .join_guarded(
            &AbstractValue::constant(U256::from(192)),
            Domain::default(),
            g,
            ExprLimits::default(),
        )
        .unwrap();
    assert!(value.contains(U256::from(128)) && value.contains(U256::from(192)));
    assert!(matches!(
        value.expression().unwrap().kind(),
        ExprKind::Ite { .. }
    ));
    let marked = value.clone().with_symbolic_limit();
    assert_eq!(marked.numeric(), value.numeric());
    assert_eq!(marked.expression(), value.expression());
    assert!(marked.symbolic_limit_reached());
}

#[test]
fn arbitrary_lazy_hash_views_report_precision_without_enumerating_lengths() {
    let length = ExprId::fresh().unwrap();
    let source = ByteView::new(
        ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
        c(0),
        length.clone(),
    );
    let copied = ByteStateId::zero()
        .copy(c(128), source.clone(), length.clone(), limits())
        .unwrap();
    let full = ByteView::new(copied.clone(), c(128), length.clone());
    assert_eq!(
        full.hash_support(ExprLimits::default()).unwrap(),
        HashSupport::GuardedCopy
    );
    let partial = ByteView::new(copied, c(129), length);
    assert_eq!(
        partial.hash_support(ExprLimits::default()).unwrap(),
        HashSupport::Uncompared
    );
    assert_eq!(
        source.hash_support(ExprLimits::default()).unwrap(),
        HashSupport::SourceView
    );
}
