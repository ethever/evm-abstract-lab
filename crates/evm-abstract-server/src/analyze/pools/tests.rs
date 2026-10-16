//! Regress native cache reuse across complete metadata and typed export bounds.
use super::Pools;
use evm_abstract::{
    U256,
    analysis::control::Control,
    domain::{
        AbstractValue, Domain,
        identity::Symbol,
        provenance::Origin,
        relational::RelationLimits,
        symbolic::{ByteStateId, ByteView, ExprId, ExprKind, ExprLimits, SourceId},
    },
    world::ByteArray,
};
use evm_abstract_protocol as api;

fn deep_expression() -> ExprId {
    let mut expression = ExprId::input(77, Symbol::CallValue);
    for _ in 0..14 {
        expression = ExprId::operation(
            0x01,
            &[expression.clone(), expression],
            ExprLimits {
                max_nodes: 65_536,
                max_depth: 256,
            },
        )
        .unwrap();
    }
    expression
}

fn deep_buffer() -> ByteArray {
    deep_buffer_with(deep_expression())
}

fn deep_buffer_with(expression: ExprId) -> ByteArray {
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 65_536,
        max_depth: 256,
        ..RelationLimits::default()
    }));
    let mut source = ByteArray::memory();
    source
        .write_word(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::top().with_expression(expression),
            1024,
            domain,
        )
        .unwrap();
    source
}

fn byte_buffer(value: &AbstractValue) -> ByteArray {
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 65_536,
        max_depth: 256,
        ..RelationLimits::default()
    }));
    let mut source = ByteArray::memory();
    source
        .write_byte(&AbstractValue::constant(U256::ZERO), value, 1024, domain)
        .unwrap();
    source
}

fn failure(error: api::ApiError, kind: api::ExportFailureKind) -> api::ExportFailure {
    let code = if kind == api::ExportFailureKind::Cancelled {
        api::ApiErrorCode::Cancelled
    } else {
        api::ApiErrorCode::Export
    };
    assert_eq!(error.code, code);
    let decoded: api::ApiError =
        serde_json::from_slice(&serde_json::to_vec(&error).unwrap()).unwrap();
    assert_eq!(decoded, error);
    let api::ErrorDetails::Export(detail) = error.details else {
        panic!("typed export failure required");
    };
    assert_eq!(detail.kind, kind);
    assert_eq!(detail.written, None);
    *detail
}

fn assert_buffer_roundtrip(pools: &Pools<'_>, id: usize) {
    pools.symbolic.graph.validate().unwrap();
    let snapshot = (
        &pools.buffers_dto[id],
        &pools.symbolic.graph,
        pools.symbolic.usage,
    );
    let decoded: (api::ByteArraySnapshot, api::SymbolicGraph, api::ExportUsage) =
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    assert_eq!(
        decoded,
        (
            pools.buffers_dto[id].clone(),
            pools.symbolic.graph.clone(),
            pools.symbolic.usage,
        )
    );
}

#[test]
fn complete_buffer_clones_reuse_one_snapshot_without_repaying_expanded_cells() {
    let source = deep_buffer();
    let original = source.clone();
    let expanded = source.work_size();
    assert!(expanded > 1_000_000);
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let graph_nodes = pools.symbolic.graph.nodes.len();
    let snapshot = pools.buffers_dto[id].clone();
    for _ in 0..64 {
        assert_eq!(pools.buffer(&source.clone()).unwrap(), id);
    }
    assert_eq!(pools.buffers.len(), 1);
    assert_eq!(pools.buffers_dto, vec![snapshot.clone()]);
    assert_eq!(pools.symbolic.graph.nodes.len(), graph_nodes);
    assert!(pools.symbolic.usage.work < expanded as u64);
    assert_eq!(snapshot.cells.len(), 32);
    pools.symbolic.graph.validate().unwrap();
    let bytes = serde_json::to_vec(&(&snapshot, &pools.symbolic.graph)).unwrap();
    let decoded: (api::ByteArraySnapshot, api::SymbolicGraph) =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, (snapshot, pools.symbolic.graph));
    assert!(source == original);
}

#[test]
fn same_root_numeric_provenance_and_operand_expression_facts_remain_distinct() {
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let expression = ExprId::input(99, Symbol::CallValue);
    let broad = byte_buffer(&AbstractValue::unknown_byte().with_expression(expression.clone()));
    let narrow = byte_buffer(
        &AbstractValue::unsigned_range(U256::ZERO, U256::from(127))
            .unwrap()
            .with_expression(expression),
    );
    assert_eq!(broad.state_root(), narrow.state_root());
    let broad_id = pools.buffer(&broad).unwrap();
    let narrow_id = pools.buffer(&narrow).unwrap();
    assert_ne!(broad_id, narrow_id);
    assert_ne!(pools.buffers_dto[broad_id], pools.buffers_dto[narrow_id]);

    let constant = AbstractValue::constant(U256::from(7));
    let memory = byte_buffer(&constant.clone().with_origin(Origin::Memory));
    let calldata = byte_buffer(&constant.clone().with_origin(Origin::Calldata));
    assert_eq!(memory.state_root(), calldata.state_root());
    let memory_id = pools.buffer(&memory).unwrap();
    let calldata_id = pools.buffer(&calldata).unwrap();
    assert_ne!(memory_id, calldata_id);
    assert_ne!(pools.buffers_dto[memory_id], pools.buffers_dto[calldata_id]);

    let domain = Domain::default();
    let mut first = ByteArray::memory();
    first
        .expand(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(1024)),
            2048,
            domain,
        )
        .unwrap();
    let mut second = first.clone();
    let offset = ExprId::input(123, Symbol::CallValue);
    first
        .write_byte(
            &AbstractValue::unsigned_range(U256::ZERO, U256::from(16))
                .unwrap()
                .with_expression(offset.clone()),
            &constant,
            2048,
            domain,
        )
        .unwrap();
    second
        .write_byte(
            &AbstractValue::unsigned_range(U256::ZERO, U256::from(32))
                .unwrap()
                .with_expression(offset),
            &constant,
            2048,
            domain,
        )
        .unwrap();
    assert_eq!(first.state_root(), second.state_root());
    assert_eq!(first.len(), second.len());
    assert_eq!(first.stored_bytes().count(), second.stored_bytes().count());
    assert_eq!(first.default_byte(), second.default_byte());
    let first_id = pools.buffer(&first).unwrap();
    let second_id = pools.buffer(&second).unwrap();
    assert_ne!(first_id, second_id);
    assert_ne!(
        pools.buffers_dto[first_id].symbolic_facts,
        pools.buffers_dto[second_id].symbolic_facts
    );
    assert_eq!(pools.buffer(&first.clone()).unwrap(), first_id);
    assert_eq!(pools.buffer(&second.clone()).unwrap(), second_id);
}

#[test]
fn same_root_length_and_hidden_allocation_metadata_still_require_native_equality() {
    let domain = Domain::default();
    let zero = AbstractValue::constant(U256::ZERO);
    let mut source = ByteArray::memory();
    source
        .expand(
            &zero,
            &AbstractValue::constant(U256::from(128)),
            1024,
            domain,
        )
        .unwrap();
    let mut allocated = source.clone();
    allocated
        .expand(
            &AbstractValue::constant(U256::from(32)),
            &AbstractValue::constant(U256::from(32)),
            1024,
            domain,
        )
        .unwrap();
    let mut longer = source.clone();
    longer
        .expand(
            &zero,
            &AbstractValue::constant(U256::from(256)),
            1024,
            domain,
        )
        .unwrap();
    assert_eq!(source.state_root(), allocated.state_root());
    assert_eq!(source.len(), allocated.len());
    assert_ne!(source, allocated);
    assert_eq!(source.state_root(), longer.state_root());
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let allocated_id = pools.buffer(&allocated).unwrap();
    let longer_id = pools.buffer(&longer).unwrap();
    assert_ne!(id, allocated_id);
    // Even equal wire snapshots cannot replace the stronger native equality.
    assert_eq!(pools.buffers_dto[id], pools.buffers_dto[allocated_id]);
    assert_ne!(id, longer_id);
    assert_ne!(
        pools.buffers_dto[id].length,
        pools.buffers_dto[longer_id].length
    );
    assert_eq!(pools.buffer(&allocated.clone()).unwrap(), allocated_id);
}

#[test]
fn forced_candidate_collision_does_not_merge_independent_input_sources() {
    let first = ByteArray::unknown();
    let second = ByteArray::unknown();
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let first_id = pools.buffer(&first).unwrap();
    pools
        .buffer_roots
        .insert(second.state_root().clone(), vec![first_id]);
    let second_id = pools.buffer(&second).unwrap();
    assert_ne!(first_id, second_id);
    assert_ne!(
        pools.buffers_dto[first_id].symbolic,
        pools.buffers_dto[second_id].symbolic
    );
    assert_eq!(pools.buffer(&second.clone()).unwrap(), second_id);
}

#[test]
fn independently_allocated_equal_expression_trees_keep_full_comparison_accounting() {
    let left = deep_expression();
    let right = deep_expression();
    assert!(left == right);
    assert!(!std::ptr::eq(left.kind(), right.kind()));
    let source = byte_buffer(&AbstractValue::constant(U256::from(7)).with_expression(left));
    let incoming = byte_buffer(&AbstractValue::constant(U256::from(7)).with_expression(right));
    assert!(source == incoming);
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let snapshot = pools.buffers_dto[id].clone();
    let before = pools.symbolic.usage.work;
    assert_eq!(pools.buffer(&incoming).unwrap(), id);
    assert!(pools.symbolic.usage.work - before > 1024);
    assert_eq!(pools.buffers.len(), 1);
    assert_eq!(pools.buffers_dto[id], snapshot);
    assert_buffer_roundtrip(&pools, id);
}

#[test]
fn independently_allocated_equal_reads_do_not_compare_their_backing_history() {
    let value = deep_expression();
    let source = deep_buffer_with(value.clone());
    let incoming = deep_buffer_with(value);
    assert!(source == incoming);
    assert_eq!(source.state_root(), incoming.state_root());
    assert!(
        source
            .stored_bytes()
            .zip(incoming.stored_bytes())
            .any(|((_, left), (_, right))| {
                let (Some(left), Some(right)) = (left.expression(), right.expression()) else {
                    return false;
                };
                matches!(
                    left.kind(),
                    ExprKind::ByteRead { .. } | ExprKind::WordRead { .. }
                ) && left.nodes() > 32_000
                    && !std::ptr::eq(left.kind(), right.kind())
            })
    );
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let nodes = pools.symbolic.graph.nodes.len();
    let snapshot = pools.buffers_dto[id].clone();
    let before = pools.symbolic.usage.work;
    for _ in 0..64 {
        assert_eq!(pools.buffer(&incoming).unwrap(), id);
    }
    assert_eq!(pools.buffers_dto.len(), 1);
    assert_eq!(pools.symbolic.graph.nodes.len(), nodes);
    let marginal = pools.symbolic.usage.work - before;
    let expanded = source.work_size();
    assert!(marginal < expanded as u64);
    let backing = source.state_root().nodes();
    let entries = source.metadata_len();
    let total = pools.symbolic.usage.work;
    eprintln!(
        "buffer read reuse: expanded={expanded} backing={backing} entries={entries} hits=64 marginal={marginal} total={total}"
    );
    assert_eq!(pools.buffers_dto[id], snapshot);
    pools.symbolic.graph.validate().unwrap();
    let decoded: (api::ByteArraySnapshot, api::SymbolicGraph) =
        serde_json::from_slice(&serde_json::to_vec(&(&snapshot, &pools.symbolic.graph)).unwrap())
            .unwrap();
    assert_eq!(decoded, (snapshot, pools.symbolic.graph.clone()));
}

fn word_read(state: &ByteStateId, offset: ExprId, length: ExprId) -> ExprId {
    let expression = ExprId::word_read(
        ByteView::new(state.clone(), offset, length),
        ExprId::constant(U256::ZERO),
        ExprLimits {
            max_nodes: 65_536,
            max_depth: 256,
        },
    )
    .unwrap();
    assert!(matches!(expression.kind(), ExprKind::WordRead { .. }));
    expression
}

#[test]
fn equal_word_reads_reuse_large_history_and_retain_full_view_scalars() {
    let backing = deep_buffer();
    let first = word_read(
        backing.state_root(),
        ExprId::constant(U256::ONE),
        ExprId::constant(U256::from(17)),
    );
    let second = word_read(
        backing.state_root(),
        ExprId::constant(U256::ONE),
        ExprId::constant(U256::from(17)),
    );
    assert!(first == second);
    assert!(!std::ptr::eq(first.kind(), second.kind()));
    assert!(first.nodes() > 32_000);
    let source = byte_buffer(&AbstractValue::top().with_expression(first.clone()));
    let incoming = byte_buffer(&AbstractValue::top().with_expression(second));
    assert!(source == incoming);
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let before = pools.symbolic.usage.work;
    let nodes = pools.symbolic.graph.nodes.len();
    for _ in 0..64 {
        assert_eq!(pools.buffer(&incoming).unwrap(), id);
    }
    assert!(pools.symbolic.usage.work - before < 64 * 1024);
    assert_eq!(pools.symbolic.graph.nodes.len(), nodes);
    assert_eq!(pools.buffers_dto.len(), 1);
    let root = pools.symbolic.expression(&first).unwrap();
    let api::Expression::WordRead(read) = &pools.symbolic.graph.nodes[root.node] else {
        panic!("typed WordRead required");
    };
    let api::Expression::View(view) = &pools.symbolic.graph.nodes[read.view.node] else {
        panic!("complete view required");
    };
    assert_eq!(
        pools.symbolic.graph.nodes[view.offset.node],
        api::Expression::Constant("0x1".into())
    );
    assert_eq!(
        pools.symbolic.graph.nodes[view.length.node],
        api::Expression::Constant("0x11".into())
    );
    pools.symbolic.graph.validate().unwrap();
    let snapshot = (&pools.buffers_dto[id], &pools.symbolic.graph);
    let decoded: (api::ByteArraySnapshot, api::SymbolicGraph) =
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    assert_eq!(
        decoded,
        (pools.buffers_dto[id].clone(), pools.symbolic.graph.clone())
    );
    for (offset, length) in [(2, 17), (1, 18)] {
        let expression = word_read(
            backing.state_root(),
            ExprId::constant(U256::from(offset)),
            ExprId::constant(U256::from(length)),
        );
        let other = byte_buffer(&AbstractValue::top().with_expression(expression));
        assert_ne!(pools.buffer(&other).unwrap(), id);
    }
}

#[test]
fn nonshared_word_view_offset_and_length_keep_recursive_work_accounting() {
    let state = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::fresh().unwrap());
    for offset in [true, false] {
        let left = deep_expression();
        let right = deep_expression();
        let (left_at, right_at, left_len, right_len) = if offset {
            (
                left,
                right,
                ExprId::constant(U256::from(32)),
                ExprId::constant(U256::from(32)),
            )
        } else {
            (
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::ZERO),
                left,
                right,
            )
        };
        let first = word_read(&state, left_at, left_len);
        let second = word_read(&state, right_at, right_len);
        assert!(first == second);
        assert!(!std::ptr::eq(first.kind(), second.kind()));
        let source = byte_buffer(&AbstractValue::top().with_expression(first));
        let incoming = byte_buffer(&AbstractValue::top().with_expression(second));
        assert!(source == incoming);
        let control = Control::default();
        let mut pools = Pools::new(&control);
        let id = pools.buffer(&source).unwrap();
        let snapshot = pools.buffers_dto[id].clone();
        let before = pools.symbolic.usage.work;
        assert_eq!(pools.buffer(&incoming).unwrap(), id);
        assert!(pools.symbolic.usage.work - before > 1024);
        assert_eq!(pools.buffers.len(), 1);
        assert_eq!(pools.buffers_dto[id], snapshot);
        assert_buffer_roundtrip(&pools, id);
    }
}

#[test]
fn buffer_projection_past_former_work_cap_remains_lossless_and_cancellable() {
    let bytes = vec![1; 4096];
    let source = ByteArray::exact(&bytes);
    let retained = source.clone();
    let control = Control::new(Default::default());
    let mut pools = Pools::new(&control);
    let id = pools.buffer(&source).unwrap();
    let snapshot = pools.buffers_dto[id].clone();
    let before = pools.symbolic.usage;
    assert!(before.work > source.metadata_len() as u64);
    assert!(before.work < 64_000_000);
    assert_eq!(snapshot.cells.len(), bytes.len());
    for (offset, cell) in snapshot.cells.iter().enumerate() {
        assert_eq!(cell.offset, offset);
        assert_eq!(
            snapshot.values[cell.value].constants,
            Some(vec!["0x1".into()])
        );
    }
    // Each hit meters the real metadata scan and complete native comparison;
    // no synthetic charges or seeded usage bypass the former boundary.
    for _ in 0..1024 {
        assert_eq!(pools.buffer(&source).unwrap(), id);
    }
    assert!(pools.symbolic.usage.work > 64_000_000);
    assert_eq!(pools.symbolic.usage.nodes, before.nodes);
    assert_eq!(pools.symbolic.usage.exact_bytes, bytes.len());
    assert_eq!(pools.buffers_dto, vec![snapshot]);
    let api::Expression::View(view) =
        &pools.symbolic.graph.nodes[pools.buffers_dto[id].symbolic.node]
    else {
        panic!("complete byte view required");
    };
    assert_eq!(
        pools.symbolic.graph.nodes[view.state.node],
        api::Expression::BytesExact(format!("0x{}", "01".repeat(bytes.len())))
    );
    let next = ByteArray::exact(&[2, 3, 4, 5]);
    let after_hits = pools.symbolic.usage.work;
    let next_id = pools.buffer(&next).unwrap();
    assert_ne!(next_id, id);
    assert!(pools.symbolic.usage.work > after_hits);
    assert_eq!(pools.buffers_dto[next_id].cells.len(), 4);
    assert_buffer_roundtrip(&pools, id);
    assert_buffer_roundtrip(&pools, next_id);
    let usage = pools.symbolic.usage;
    control.cancellation().cancel();
    let detail = failure(
        pools.buffer(&source).unwrap_err(),
        api::ExportFailureKind::Cancelled,
    );
    assert_eq!(detail.required, usage.work);
    assert_eq!(detail.usage, usage);
    assert_eq!(detail.limits, pools.symbolic.limits);
    assert_eq!(source, retained);
}

#[test]
fn saturated_work_still_allows_projection_and_preserves_payload_bounds_and_cancellation() {
    let source = ByteArray::exact(&[1, 2, 3, 4]);
    let retained = source.clone();
    let control = Control::new(Default::default());
    let mut pools = Pools::new(&control);
    pools.symbolic.usage.work = u64::MAX - 1;
    let id = pools.buffer(&source).unwrap();
    assert_eq!(pools.symbolic.usage.work, u64::MAX);
    assert_eq!(pools.buffer(&source).unwrap(), id);
    assert_eq!(pools.symbolic.usage.work, u64::MAX);
    assert_eq!(pools.buffers_dto[id].cells.len(), 4);
    assert_buffer_roundtrip(&pools, id);
    pools.symbolic.limits.max_exact_bytes = pools.symbolic.usage.exact_bytes;
    let detail = failure(
        pools.buffer(&ByteArray::exact(&[5, 6, 7, 8])).unwrap_err(),
        api::ExportFailureKind::ExactBytes,
    );
    assert_eq!(detail.usage.work, u64::MAX);
    assert!(detail.required > detail.limits.max_exact_bytes as u64);
    control.cancellation().cancel();
    let usage = pools.symbolic.usage;
    let detail = failure(
        pools.buffer(&source).unwrap_err(),
        api::ExportFailureKind::Cancelled,
    );
    assert_eq!(detail.required, u64::MAX);
    assert_eq!(detail.usage, usage);
    assert_eq!(source, retained);
}

#[test]
fn buffer_export_bounds_and_cache_hit_cancellation_remain_typed_and_lossless() {
    for (kind, limit) in [
        (api::ExportFailureKind::Nodes, 0),
        (api::ExportFailureKind::Depth, 1),
        (api::ExportFailureKind::ExactBytes, 3),
    ] {
        let source = if kind == api::ExportFailureKind::Depth {
            byte_buffer(&AbstractValue::unknown_byte().with_expression(ExprId::fresh().unwrap()))
        } else {
            ByteArray::exact(&[1, 2, 3, 4])
        };
        let retained = source.clone();
        let control = Control::default();
        let mut pools = Pools::new(&control);
        match kind {
            api::ExportFailureKind::Nodes => pools.symbolic.limits.max_nodes = limit,
            api::ExportFailureKind::Depth => pools.symbolic.limits.max_depth = limit,
            api::ExportFailureKind::ExactBytes => pools.symbolic.limits.max_exact_bytes = limit,
            _ => unreachable!(),
        }
        let detail = failure(pools.buffer(&source).unwrap_err(), kind);
        assert_eq!(detail.limits, pools.symbolic.limits);
        assert_eq!(detail.usage, pools.symbolic.usage);
        assert!(detail.required > limit as u64);
        assert_eq!(source, retained);
    }
    let source = ByteArray::memory();
    let retained = source.clone();
    let control = Control::new(Default::default());
    let mut pools = Pools::new(&control);
    pools.buffer(&source).unwrap();
    let usage = pools.symbolic.usage;
    control.cancellation().cancel();
    let detail = failure(
        pools.buffer(&source).unwrap_err(),
        api::ExportFailureKind::Cancelled,
    );
    assert_eq!(detail.usage, usage);
    assert_eq!(detail.limits, pools.symbolic.limits);
    assert_eq!(source, retained);
}

#[test]
fn log_pool_preserves_captured_hash_and_init_runtime_identity_through_json() {
    use alloy_primitives::{Address, B256};
    use evm_abstract::{
        Fork,
        world::{AbstractLog, LogKey, LogSourceKind, Store, World},
    };
    let owner = Address::from([0x11; 20]);
    let code_owner = Address::from([0x22; 20]);
    let hash = B256::from([0x33; 32]);
    let other_hash = B256::from([0x44; 32]);
    let mut source = Store::new(&World::new(Fork::Osaka, "log source projection"));
    let cases = [
        (hash, LogSourceKind::Init, api::LogSourceKind::Init),
        (hash, LogSourceKind::Runtime, api::LogSourceKind::Runtime),
        (
            other_hash,
            LogSourceKind::Runtime,
            api::LogSourceKind::Runtime,
        ),
    ];
    for (index, &(code_hash, source_kind, _)) in cases.iter().enumerate() {
        source
            .emit_log(
                LogKey {
                    address: owner,
                    code_address: code_owner,
                    code_hash,
                    source_kind,
                    pc: 7,
                },
                AbstractLog {
                    topics: vec![AbstractValue::constant(U256::from(index)); index],
                    data: ByteArray::exact(&[index as u8]),
                },
                Domain::default(),
            )
            .unwrap();
    }
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.store(&source).unwrap();
    assert_eq!(pools.store(&source.clone()).unwrap(), id);
    let snapshot = &pools.stores[id];
    assert_eq!(snapshot.logs.len(), 3);
    for (index, &(code_hash, _, source_kind)) in cases.iter().enumerate() {
        let log = snapshot
            .logs
            .iter()
            .find(|log| log.code_hash == code_hash.to_string() && log.source_kind == source_kind)
            .unwrap();
        assert_eq!(log.address, owner.to_string());
        assert_eq!(log.code_address, code_owner.to_string());
        assert_eq!(log.pc, 7);
        assert_eq!(log.topics.len(), index);
    }
    let encoded = serde_json::to_vec(snapshot).unwrap();
    assert_eq!(
        serde_json::from_slice::<api::StoreSnapshot>(&encoded).unwrap(),
        *snapshot
    );
}
