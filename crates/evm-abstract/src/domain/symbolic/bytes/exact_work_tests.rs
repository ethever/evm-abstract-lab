//! Retained payload units are independent of logical ranges and cache warmth.

use super::{
    ByteStateId, ByteStateKind, ByteView, ExprId, ExprLimits, SourceId, StateBinding, StateLimits,
    state,
};
use crate::{
    U256,
    domain::{
        EvmValue, NumericValue, ProductAlgebra, ValueAlgebra, WordObservation,
        symbolic::{ExprError, ExprKind, GraphLimits, SymbolicGraph},
    },
    resource::WorkBudget,
    world::EvmByteArray,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn exact(bytes: &[u8]) -> ByteStateId {
    ByteStateId::exact(Arc::from(bytes))
}

fn limits() -> StateLimits {
    StateLimits::default()
}

#[test]
fn every_byte_edge_kind_keeps_retained_payload_units() {
    let input = ByteStateId::input(SourceId::fresh().unwrap(), c(4));
    let data = exact(&[1, 0, 2, 0]);
    let byte = data.write_byte(c(10), c(7), limits()).unwrap();
    let word = byte.write_word(c(32), c(9), limits()).unwrap();
    let copied = word
        .copy(
            c(64),
            ByteView::new(exact(&[3, 4, 5]), c(0), c(3)),
            c(1),
            limits(),
        )
        .unwrap();
    let slice = copied.slice(c(1), c(1), limits()).unwrap();
    let selected = slice
        .join_guarded(&input, ExprId::fresh().unwrap(), limits())
        .unwrap();
    assert!(matches!(byte.kind(), ByteStateKind::WriteByte { .. }));
    assert!(matches!(word.kind(), ByteStateKind::WriteWord { .. }));
    assert!(matches!(copied.kind(), ByteStateKind::Copy { .. }));
    assert!(matches!(slice.kind(), ByteStateKind::Slice { .. }));
    assert!(matches!(selected.kind(), ByteStateKind::Ite { .. }));
    for (root, bytes) in [
        (ByteStateId::zero(), 0),
        (exact(&[]), 0),
        (input, 0),
        (data, 4),
        (byte, 4),
        (word, 4),
        (copied, 7),
        (slice, 7),
        (selected, 7),
    ] {
        assert_eq!(root.exact_payload_bytes(), bytes, "{root:?}");
        assert_eq!(root.exact_payload_bytes(), bytes, "warm {root:?}");
    }
}

#[test]
fn overlapping_diamonds_deduplicate_warm_children_and_distinct_payloads() {
    let data = exact(&[1, 2, 3, 4]);
    let distinct = exact(&[5, 6, 7, 8]);
    assert_ne!(data, distinct);
    assert_eq!(data.id(), exact(&[1, 2, 3, 4]).id());
    let left = data.write_byte(c(10), c(1), limits()).unwrap();
    let right = data.write_word(c(32), c(2), limits()).unwrap();
    assert_eq!(left.exact_payload_bytes(), 4);
    assert_eq!(right.exact_payload_bytes(), 4);
    let diamond = left
        .copy(
            c(64),
            ByteView::new(right.clone(), c(0), c(1)),
            c(1),
            limits(),
        )
        .unwrap();
    assert!(matches!(diamond.kind(), ByteStateKind::Copy { .. }));
    assert_eq!(diamond.exact_payload_bytes(), 4);
    let selected = left
        .join_guarded(&right, ExprId::fresh().unwrap(), limits())
        .unwrap();
    assert!(matches!(selected.kind(), ByteStateKind::Ite { .. }));
    assert_eq!(selected.exact_payload_bytes(), 4);
    let aliased = data
        .copy(
            c(64),
            ByteView::new(data.clone(), c(0), c(1)),
            c(1),
            limits(),
        )
        .unwrap();
    assert_eq!(aliased.exact_payload_bytes(), 4);
    let both = right
        .copy(c(80), ByteView::new(distinct, c(0), c(1)), c(1), limits())
        .unwrap();
    assert_eq!(both.exact_payload_bytes(), 8);
    let overlap = diamond
        .join_guarded(&both, ExprId::fresh().unwrap(), limits())
        .unwrap();
    assert!(matches!(overlap.kind(), ByteStateKind::Ite { .. }));
    assert_eq!(overlap.exact_payload_bytes(), 8);
}

#[test]
fn slices_and_copy_normalization_count_buffers_in_the_resulting_snapshot() {
    let data = exact(&[1, 2, 3, 4]);
    let slice = data.slice(c(1), c(1), limits()).unwrap();
    assert!(matches!(slice.kind(), ByteStateKind::Slice { .. }));
    assert_eq!(slice.exact_payload_bytes(), 4);
    assert_eq!(data.slice(c(0), c(4), limits()).unwrap(), data);
    let maximum = ExprId::constant(U256::MAX);
    assert_eq!(
        data.slice(maximum.clone(), c(0), limits())
            .unwrap()
            .exact_payload_bytes(),
        0
    );
    let huge = ByteStateId::zero()
        .copy(
            c(0),
            ByteView::new(data.clone(), c(0), maximum.clone()),
            maximum.clone(),
            limits(),
        )
        .unwrap();
    assert_eq!(huge.exact_payload_bytes(), 4);
    assert_eq!(
        data.copy(maximum, ByteView::new(huge, c(0), c(4)), c(0), limits())
            .unwrap(),
        data
    );
    let copied = ByteStateId::zero()
        .copy(c(0), ByteView::new(data, c(0), c(4)), c(4), limits())
        .unwrap();
    assert_eq!(copied.exact_payload_bytes(), 4);
    let replacement = copied
        .copy(
            c(0),
            ByteView::new(exact(&[5, 6, 7, 8, 9]), c(0), c(4)),
            c(4),
            limits(),
        )
        .unwrap();
    assert_eq!(replacement.exact_payload_bytes(), 5);
    assert_eq!(copied.exact_payload_bytes(), 4);
}

#[test]
fn scalar_dependencies_are_excluded_even_when_export_reaches_their_bytes() {
    let hidden = ExprId::byte_read(
        exact(&[1, 2, 3, 4, 5, 6]),
        ExprId::fresh().unwrap(),
        ExprLimits::default(),
    )
    .unwrap();
    assert!(matches!(hidden.kind(), ExprKind::ByteRead { .. }));
    let input = ByteStateId::observed(
        SourceId::scoped(hidden.clone(), U256::from(17)),
        hidden.clone(),
        vec![StateBinding {
            index: U256::ZERO,
            value: hidden.clone(),
            numeric: WordObservation::constant(U256::from(7)),
        }],
        limits(),
    )
    .unwrap();
    let zero = ByteStateId::zero();
    let byte = zero
        .write_byte(hidden.clone(), hidden.clone(), limits())
        .unwrap();
    let word = zero
        .write_word(hidden.clone(), hidden.clone(), limits())
        .unwrap();
    let copied = zero
        .copy(
            hidden.clone(),
            ByteView::new(input.clone(), hidden.clone(), hidden.clone()),
            hidden.clone(),
            limits(),
        )
        .unwrap();
    let slice = input
        .slice(hidden.clone(), hidden.clone(), limits())
        .unwrap();
    let selected = zero.join_guarded(&input, hidden, limits()).unwrap();
    assert!(matches!(copied.kind(), ByteStateKind::Copy { .. }));
    assert!(matches!(slice.kind(), ByteStateKind::Slice { .. }));
    for root in [&input, &byte, &word, &copied, &slice, &selected] {
        assert_eq!(root.exact_payload_bytes(), 0);
    }
    assert_eq!(
        SymbolicGraph::bytes(
            &input,
            GraphLimits {
                max_exact_bytes: 5,
                ..GraphLimits::default()
            }
        )
        .unwrap_err(),
        ExprError::ExportBytes { bytes: 6, limit: 5 }
    );
}

#[test]
fn clones_and_intern_hits_share_one_cache_without_changing_identity_or_retention() {
    let weak = {
        let input = ByteStateId::input(SourceId::fresh().unwrap(), c(8));
        let view = ByteView::new(exact(&[9, 8, 7]), c(0), c(3));
        let root = input.copy(c(1), view.clone(), c(3), limits()).unwrap();
        let cloned = root.clone();
        let rebuilt = input.copy(c(1), view.clone(), c(3), limits()).unwrap();
        assert!(Arc::ptr_eq(&root.0, &cloned.0));
        assert!(Arc::ptr_eq(&root.0, &rebuilt.0));
        assert!(root.0.kind.exact_payload_bytes.get().is_none());
        let before = (
            root.id(),
            root.nodes(),
            root.depth(),
            root.copy_leaves(),
            state::fingerprint(&root),
        );
        let ordering = root.cmp(&input);
        let keys = BTreeSet::from([root.clone(), input.clone()]);
        let encoded = serde_json::to_value(&root).unwrap();
        assert_eq!(cloned.exact_payload_bytes(), 3);
        assert_eq!(rebuilt.0.kind.exact_payload_bytes.get(), Some(&3));
        let reinterned = input.copy(c(1), view, c(3), limits()).unwrap();
        assert!(Arc::ptr_eq(&root.0, &reinterned.0));
        assert_eq!(reinterned.0.kind.exact_payload_bytes.get(), Some(&3));
        assert!(input.0.kind.exact_payload_bytes.get().is_none());
        assert_eq!(
            before,
            (
                root.id(),
                root.nodes(),
                root.depth(),
                root.copy_leaves(),
                state::fingerprint(&root)
            )
        );
        assert_eq!(ordering, root.cmp(&input));
        assert!(keys.contains(&rebuilt));
        assert_eq!(encoded, serde_json::to_value(&root).unwrap());
        let changed = root.write_byte(c(20), c(7), limits()).unwrap();
        assert_ne!(changed, root);
        assert!(changed.0.kind.exact_payload_bytes.get().is_none());
        assert_eq!(changed.exact_payload_bytes(), 3);
        assert_eq!(root.0.kind.exact_payload_bytes.get(), Some(&3));
        Arc::downgrade(&root.0)
    };
    assert!(weak.upgrade().is_none());
}

#[test]
fn warming_preserves_hidden_source_renaming_and_export_limits() {
    let source = SourceId::scoped(ExprId::fresh().unwrap(), U256::from(17));
    let length = ExprId::fresh().unwrap();
    let binding = StateBinding {
        index: U256::ZERO,
        value: ExprId::fresh().unwrap(),
        numeric: WordObservation::constant(U256::from(7)),
    };
    let input = ByteStateId::observed(
        source.clone(),
        length.clone(),
        vec![binding.clone()],
        limits(),
    )
    .unwrap();
    let independent = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
    assert_eq!(input.exact_payload_bytes(), 0);
    assert_eq!(independent.exact_payload_bytes(), 0);
    assert_ne!(input, independent);
    let root = input
        .copy(
            c(64),
            ByteView::new(exact(&[3, 2, 1]), c(0), c(3)),
            c(2),
            limits(),
        )
        .unwrap();
    let leaves = root.fresh_leaves();
    for leaf in [source.token(), &length, &binding.value] {
        assert!(leaves.contains(leaf));
    }
    let encoded = serde_json::to_value(&root).unwrap();
    let export_limits = GraphLimits {
        max_exact_bytes: 2,
        ..GraphLimits::default()
    };
    let cold_error = SymbolicGraph::bytes(&root, export_limits).unwrap_err();
    assert!(root.0.kind.exact_payload_bytes.get().is_none());
    assert_eq!(root.exact_payload_bytes(), 3);
    assert_eq!(encoded, serde_json::to_value(&root).unwrap());
    assert_eq!(cold_error, ExprError::ExportBytes { bytes: 3, limit: 2 });
    assert_eq!(
        SymbolicGraph::bytes(&root, export_limits).unwrap_err(),
        cold_error
    );
    let replacements: BTreeMap<_, _> = leaves
        .iter()
        .map(|leaf| (leaf.clone(), ExprId::fresh().unwrap()))
        .collect();
    let renamed = root
        .rename_fresh(&replacements, ExprLimits::default())
        .unwrap();
    assert_ne!(root, renamed);
    assert_eq!(renamed.exact_payload_bytes(), 3);
    assert_eq!(root.fresh_leaves(), leaves);
    assert_eq!(
        renamed.fresh_leaves(),
        replacements.values().cloned().collect()
    );
    let ByteStateKind::Copy { previous, .. } = renamed.kind() else {
        panic!("renamed copy lost its source history")
    };
    let ByteStateKind::Input {
        source: renamed_source,
        length: renamed_length,
        bindings,
    } = previous.kind()
    else {
        panic!("renamed copy lost its hidden input")
    };
    assert_eq!(renamed_source.token(), &replacements[source.token()]);
    assert_eq!(renamed_source.namespace(), source.namespace());
    assert_eq!(renamed_length, &replacements[&length]);
    assert_eq!(&bindings[0].value, &replacements[&binding.value]);
    assert_eq!(bindings[0].numeric, binding.numeric);
    assert!(
        !serde_json::to_string(&renamed)
            .unwrap()
            .contains("exact_payload_bytes")
    );
}

#[test]
fn cold_and_warm_world_quotes_keep_the_same_payment_and_mutable_metadata() {
    type Bytes = EvmByteArray<NumericValue>;
    let value = |n: u64| EvmValue::<NumericValue>::constant(U256::from(n));
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let mut memory = Bytes::memory();
    memory
        .copy_from(
            &value(0),
            &Bytes::unknown(),
            &value(0),
            &value(4),
            domain.borrowed(),
        )
        .unwrap();
    memory
        .copy_from(
            &value(8),
            &Bytes::exact(&[1, 0, 2, 0]),
            &value(0),
            &value(4),
            domain.borrowed(),
        )
        .unwrap();
    let root = memory.state_root().clone();
    let cloned = memory.clone();
    assert!(root.0.kind.exact_payload_bytes.get().is_none());
    let encoded = serde_json::to_value(&memory).unwrap();
    let cold = memory.work_size();
    assert_eq!(root.0.kind.exact_payload_bytes.get(), Some(&4));
    for array in [&memory, &cloned] {
        assert_eq!(array.work_size(), cold);
        let mut exact_budget = WorkBudget::new(cold);
        assert!(exact_budget.charge(array.work_size()));
        assert_eq!(exact_budget.used(), cold);
        let mut insufficient = WorkBudget::new(cold - 1);
        assert!(!insufficient.charge(array.work_size()));
        assert_eq!(insufficient.used(), 0);
    }
    assert_eq!(encoded, serde_json::to_value(&memory).unwrap());
    let mut expanded = cloned;
    expanded
        .expand(&value(64), &value(32), domain.borrowed())
        .unwrap();
    assert_eq!(expanded.state_root(), &root);
    assert_eq!(root.exact_payload_bytes(), 4);
    assert!(expanded.work_size() > cold);
}

#[test]
fn saturated_occurrence_metrics_do_not_multiply_shared_payload_bytes() {
    let mut root = exact(&[8, 9, 10]);
    let condition = ExprId::fresh().unwrap();
    // Like the existing copy-metric fixture, private construction bypasses
    // simplification/limits to reach overflow with a small physical DAG.
    for _ in 0..=usize::BITS {
        let (nodes, depth) = state::metrics([
            (root.nodes(), root.depth()),
            (root.nodes(), root.depth()),
            (condition.nodes(), condition.depth()),
        ]);
        root = ByteStateId::intern(
            ByteStateKind::Ite {
                condition: condition.clone(),
                then_state: root.clone(),
                else_state: root,
            },
            nodes,
            depth,
        );
    }
    assert_eq!(root.nodes(), usize::MAX);
    assert_eq!(root.copy_leaves(), usize::MAX);
    assert_eq!(root.exact_payload_bytes(), 3);
    assert_eq!(root.exact_payload_bytes(), 3);
}
