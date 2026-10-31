use super::super::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, ExprLimits, GraphLimits,
    HashSupport, SlotStateId, SourceId, StateError, StateLimits, SymbolicGraph, SymbolicGraphNode,
    state,
};
use crate::domain::NumericValue;
use crate::domain::{EvmValue, ProductAlgebra, identity::Symbol};
use alloy_primitives::U256;
use std::{collections::BTreeMap, sync::Arc};

mod guarded;

fn c(v: u64) -> ExprId {
    ExprId::constant(U256::from(v))
}
fn limits() -> StateLimits {
    StateLimits::default()
}

#[test]
fn copy_leaves_count_selected_occurrences_without_changing_identity_or_export() {
    let a = ByteStateId::exact(Arc::from([0x11]));
    let b = ByteStateId::exact(Arc::from([0x22]));
    let guard = ExprId::fresh().unwrap();
    let pair = a.join_guarded(&b, guard.clone(), limits()).unwrap();
    let nested = pair
        .join_guarded(&ByteStateId::zero(), ExprId::fresh().unwrap(), limits())
        .unwrap();
    // Keep this metric fixture irreducible: the new pair has an independent
    // guard, and the nested branch's complete alternatives are (pair, zero).
    let independent_pair = a
        .join_guarded(&b, ExprId::fresh().unwrap(), limits())
        .unwrap();
    let shared = independent_pair
        .join_guarded(&nested, ExprId::fresh().unwrap(), limits())
        .unwrap();
    for (root, leaves, nodes, depth) in [(&pair, 2, 4, 2), (&nested, 3, 7, 3), (&shared, 5, 13, 4)]
    {
        assert_eq!(root.copy_leaves(), leaves);
        // Retention metrics also sum occurrences, including repeated children.
        assert_eq!((root.nodes(), root.depth()), (nodes, depth));
        assert!(leaves <= nodes);
    }
    let rebuilt = a.join_guarded(&b, guard, limits()).unwrap();
    assert_eq!(pair.id(), rebuilt.id());
    assert_eq!(pair.kind(), rebuilt.kind());
    assert_eq!(state::fingerprint(&pair), state::fingerprint(&pair.id()));
    assert_eq!(shared.clone().id(), shared.id());
    assert_eq!(
        std::mem::size_of::<ByteStateId>(),
        std::mem::size_of::<Arc<()>>()
    );
    let graph = SymbolicGraph::bytes(&shared, GraphLimits::default()).unwrap();
    assert_eq!(graph.nodes.len(), 11); // Three leaves, four guards, four Ite nodes.
    assert_eq!(
        graph
            .nodes
            .iter()
            .filter(|node| matches!(node, SymbolicGraphNode::BytesExact(_)))
            .count(),
        2
    );
    let encoded = serde_json::to_value(&shared).unwrap();
    assert_eq!(encoded, serde_json::to_value(&graph).unwrap());
    assert_eq!(encoded, serde_json::to_value(shared.clone()).unwrap());
    assert!(
        !serde_json::to_string(&shared)
            .unwrap()
            .contains("copy_leaves")
    );
    let map = shared
        .fresh_leaves()
        .into_iter()
        .map(|leaf| (leaf, ExprId::fresh().unwrap()))
        .collect();
    let renamed = shared.rename_fresh(&map, ExprLimits::default()).unwrap();
    assert_eq!(renamed.copy_leaves(), 5);
    assert_eq!((renamed.nodes(), renamed.depth()), (13, 4));
    assert_eq!(pair.join_guarded(&pair, c(0), limits()).unwrap(), pair);
    assert_eq!(a.join_guarded(&b, c(1), limits()).unwrap().copy_leaves(), 1);
}

#[test]
fn copy_leaves_stop_at_every_nonselected_kind_even_over_selected_history() {
    let a = ByteStateId::exact(Arc::from([0x11, 0x22]));
    let selected = a
        .join_guarded(&ByteStateId::zero(), ExprId::fresh().unwrap(), limits())
        .unwrap();
    let source = ByteView::new(selected.clone(), c(0), c(2));
    let byte = selected.write_byte(c(8), c(3), limits()).unwrap();
    let word = selected.write_word(c(32), c(4), limits()).unwrap();
    let copy = selected.copy(c(64), source, c(2), limits()).unwrap();
    let slice = selected.slice(c(1), c(1), limits()).unwrap();
    assert!(
        matches!(byte.kind(), ByteStateKind::WriteByte { previous, .. } if previous == &selected)
    );
    assert!(
        matches!(word.kind(), ByteStateKind::WriteWord { previous, .. } if previous == &selected)
    );
    assert!(matches!(copy.kind(), ByteStateKind::Copy { previous, .. } if previous == &selected));
    assert!(matches!(slice.kind(), ByteStateKind::Slice { source } if source.state == selected));
    for root in [
        ByteStateId::zero(),
        ByteStateId::input(SourceId::fresh().unwrap(), c(2)),
        ByteStateId::observed(SourceId::fresh().unwrap(), c(2), Vec::new(), limits()).unwrap(),
        a,
        byte,
        word,
        copy.clone(),
        slice,
    ] {
        assert_eq!(root.copy_leaves(), 1, "{root:?}");
    }
    assert_eq!(
        copy.copy(c(64), ByteView::new(selected, c(0), c(2)), c(2), limits())
            .unwrap(),
        copy
    );
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
fn alternating_disjoint_slot_writes_reuse_the_immutable_snapshot() {
    let initial = SlotStateId::zero();
    let snapshot = initial
        .store(c(0), c(7), limits())
        .unwrap()
        .store(c(1), c(9), limits())
        .unwrap();
    let mut state = snapshot.clone();
    for _ in 0..8 {
        state = state
            .store(c(0), c(7), limits())
            .unwrap()
            .store(c(1), c(9), limits())
            .unwrap();
        assert_eq!(
            state, snapshot,
            "unchanged alternating slots must stabilize"
        );
        assert_eq!((state.nodes(), state.depth()), (7, 3));
    }
    assert_eq!(initial, SlotStateId::zero());
}
#[test]
fn alternating_disjoint_word_writes_reuse_the_immutable_snapshot() {
    let initial = ByteStateId::zero();
    let snapshot = initial
        .write_word(c(0), c(7), limits())
        .unwrap()
        .write_word(c(32), c(9), limits())
        .unwrap();
    let mut state = snapshot.clone();
    for _ in 0..8 {
        state = state
            .write_word(c(0), c(7), limits())
            .unwrap()
            .write_word(c(32), c(9), limits())
            .unwrap();
        assert_eq!(
            state, snapshot,
            "unchanged alternating words must stabilize"
        );
        assert_eq!((state.nodes(), state.depth()), (7, 3));
    }
    assert_eq!(initial, ByteStateId::zero());
}
#[test]
fn disjoint_byte_and_word_writes_stabilize_at_mathematical_address_boundaries() {
    for (word_at, byte_at) in [
        (U256::ZERO, U256::from(32)),
        (U256::from(32), U256::ZERO),
        (U256::MAX, U256::ZERO),
        (U256::ZERO, U256::MAX),
    ] {
        let word_at = ExprId::constant(word_at);
        let byte_at = ExprId::constant(byte_at);
        let word_then_byte = ByteStateId::zero()
            .write_word(word_at.clone(), c(7), limits())
            .unwrap()
            .write_byte(byte_at.clone(), c(0xaa), limits())
            .unwrap();
        assert_eq!(
            word_then_byte,
            word_then_byte
                .write_word(word_at.clone(), c(7), limits())
                .unwrap()
        );
        let byte_then_word = ByteStateId::zero()
            .write_byte(byte_at.clone(), c(0xaa), limits())
            .unwrap()
            .write_word(word_at, c(7), limits())
            .unwrap();
        assert_eq!(
            byte_then_word,
            byte_then_word
                .write_byte(byte_at, c(0xaa), limits())
                .unwrap()
        );
    }
}
#[test]
fn overlapping_word_and_byte_writes_are_not_skipped_even_at_max_address() {
    let all_ones = ExprId::constant(U256::MAX);
    let snapshot = ByteStateId::zero()
        .write_word(c(0), all_ones.clone(), limits())
        .unwrap()
        .write_word(c(31), c(0), limits())
        .unwrap();
    let restored = snapshot
        .write_word(c(0), all_ones.clone(), limits())
        .unwrap();
    assert_ne!(snapshot, restored);
    assert_eq!(
        ByteView::new(snapshot, c(31), c(1))
            .known_bytes(ExprLimits::default())
            .unwrap(),
        Some(vec![0])
    );
    assert_eq!(
        ByteView::new(restored, c(31), c(1))
            .known_bytes(ExprLimits::default())
            .unwrap(),
        Some(vec![0xff])
    );
    let snapshot = ByteStateId::zero()
        .write_word(all_ones.clone(), all_ones.clone(), limits())
        .unwrap()
        .write_byte(all_ones.clone(), c(0xaa), limits())
        .unwrap();
    let restored = snapshot
        .write_word(all_ones.clone(), all_ones.clone(), limits())
        .unwrap();
    assert_ne!(snapshot, restored);
    assert_eq!(
        ByteView::new(snapshot, all_ones.clone(), c(2))
            .known_bytes(ExprLimits::default())
            .unwrap(),
        Some(vec![0xaa, 0])
    );
    assert_eq!(
        ByteView::new(restored, all_ones, c(2))
            .known_bytes(ExprLimits::default())
            .unwrap(),
        Some(vec![0xff, 0])
    );
}
#[test]
fn disjoint_write_lookup_preserves_limits_and_cancellation() {
    let slots = SlotStateId::zero()
        .store(c(0), c(7), limits())
        .unwrap()
        .store(c(1), c(9), limits())
        .unwrap();
    let words = ByteStateId::zero()
        .write_word(c(0), c(7), limits())
        .unwrap()
        .write_word(c(32), c(9), limits())
        .unwrap();
    let bytes = ByteStateId::zero()
        .write_byte(c(0), c(0xaa), limits())
        .unwrap()
        .write_word(c(32), c(9), limits())
        .unwrap();
    let exact_fit = StateLimits {
        max_nodes: 7,
        max_depth: 3,
    };
    assert_eq!(slots, slots.store(c(0), c(7), exact_fit).unwrap());
    assert_eq!(words, words.write_word(c(0), c(7), exact_fit).unwrap());
    assert_eq!(bytes, bytes.write_byte(c(0), c(0xaa), exact_fit).unwrap());
    for denied in [
        StateLimits {
            max_nodes: 6,
            ..limits()
        },
        StateLimits {
            max_depth: 2,
            ..limits()
        },
    ] {
        assert!(matches!(
            slots.store(c(0), c(7), denied),
            Err(StateError::Limit { .. })
        ));
        assert!(matches!(
            words.write_word(c(0), c(7), denied),
            Err(StateError::Limit { .. })
        ));
        assert!(matches!(
            bytes.write_byte(c(0), c(0xaa), denied),
            Err(StateError::Limit { .. })
        ));
    }
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            slots.store(c(0), c(7), limits()),
            Err(StateError::Cancelled)
        );
        assert_eq!(
            words.write_word(c(0), c(7), limits()),
            Err(StateError::Cancelled)
        );
        assert_eq!(
            bytes.write_byte(c(0), c(0xaa), limits()),
            Err(StateError::Cancelled)
        );
    });
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
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
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
    let value = EvmValue::<NumericValue>::constant(U256::from(128))
        .join_guarded(
            &EvmValue::<NumericValue>::constant(U256::from(192)),
            crate::domain::ValueAlgebra::new(ProductAlgebra::default()).borrowed(),
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
