//! Real opcode execution is projected, encoded and parsed before assertions.
use super::*;

fn analyze_hex(hex: &str) -> api::AnalysisReport {
    crate::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: hex.into(),
            address: Some(Address::repeat_byte(0x11).to_string()),
        }),
        ..api::AnalyzeRequest::default()
    })
    .unwrap()
}
fn roundtrip(report: api::AnalysisReport) -> api::AnalysisReport {
    report.validate().unwrap();
    let bytes = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed, report);
    parsed.validate().unwrap();
    parsed
}
#[test]
fn actual_symbolic_storage_write_read_reaches_protocol_and_ssa() {
    let report = roundtrip(analyze_hex("5f3580600790555400"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(
        report
            .symbolic
            .nodes
            .iter()
            .any(|node| matches!(node, api::Expression::Store(_)))
    );
    assert!(
        report
            .states
            .iter()
            .filter_map(|state| state.exit.as_ref())
            .any(|machine| machine.frames.last().is_some_and(|frame| frame
                .stack
                .last()
                .is_some_and(|v| v.constants == Some(vec!["0x7".into()]))))
    );
    assert!(report.ssa.values.iter().any(|v| {
        matches!(&v.origin,api::SsaValueOrigin::Instruction(origin) if origin.pc==7)
            && v.facts.iter().any(|fact| {
                fact.status == api::SsaFactStatus::Current
                    && fact.constant.as_deref() == Some("0x7")
            })
    }));
    for store in &report.stores {
        for account in &store.accounts {
            report
                .symbolic
                .reference(
                    None,
                    api::SymbolicReference {
                        sort: api::SymbolicSort::Slots,
                        node: account.storage_root.node,
                    },
                )
                .unwrap();
        }
    }
}

#[test]
fn actual_storage_alias_keeps_both_ordered_values_and_the_read_address() {
    let report = roundtrip(analyze_hex("5f358060079055602035600990555400"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    // select(store(store(S,k,7),q,9),k) preserves the conditional aliasing
    // directly; the native graph need not materialize a separate scalar Ite.
    let read = report
        .symbolic
        .nodes
        .iter()
        .find_map(|node| match node {
            api::Expression::SlotRead(read) => Some(read),
            _ => None,
        })
        .unwrap();
    let api::Expression::Store(latest) = &report.symbolic.nodes[read.state.node] else {
        panic!("read retains the latest update")
    };
    let api::Expression::Store(earlier) = &report.symbolic.nodes[latest.previous.node] else {
        panic!("read retains the earlier update")
    };
    assert_eq!(read.key, earlier.key);
    assert_ne!(read.key, latest.key);
    assert_eq!(
        report.symbolic.nodes[latest.value.node],
        api::Expression::Constant("0x9".into())
    );
    assert_eq!(
        report.symbolic.nodes[earlier.value.node],
        api::Expression::Constant("0x7".into())
    );
    assert!(
        report
            .states
            .iter()
            .filter_map(|state| state.exit.as_ref())
            .any(|machine| {
                machine.frames.last().is_some_and(|frame| {
                    frame.stack.last().is_some_and(|value| {
                        value.constants == Some(vec!["0x7".into(), "0x9".into()])
                            && value.expression.is_some()
                    })
                })
            })
    );
    assert_eq!(
        report
            .symbolic
            .nodes
            .iter()
            .filter(|node| matches!(node, api::Expression::Store(_)))
            .count(),
        2
    );
}

#[test]
fn consumed_symbolic_memory_address_keeps_complete_non_singleton_numeric_facts() {
    let report = roundtrip(analyze_hex("5f356010166007905200"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    let facts = report
        .byte_arrays
        .iter()
        .filter(|bytes| bytes.memory)
        .flat_map(|bytes| &bytes.symbolic_facts);
    assert!(
        facts.into_iter().any(|fact| {
            fact.value.unsigned.lower == "0x0"
                && fact.value.unsigned.upper == "0x10"
                && fact.value.known_zero != "0x0"
                && fact.value.expression == Some(fact.expression)
        }),
        "the consumed p = calldata_word & 0x10 must retain its interval, masks and exact expression binding"
    );
}
#[test]
fn unknown_length_copy_retains_guard_and_pre_effect_state_in_json() {
    let report = roundtrip(analyze_hex("366000803700"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(report.ssa.complete);
    let state = &report.states[0];
    let exit = state.exit.as_ref().unwrap();
    let frame = exit.frames.last().unwrap();
    let input = &report.byte_arrays[frame.calldata];
    let length = input.length.expression.unwrap();
    assert!(
        matches!(&report.symbolic.nodes[length.node], api::Expression::Input(identity)
        if identity.symbol.kind == api::InputSymbolKind::CalldataLength)
    );
    let api::Expression::View(memory) =
        &report.symbolic.nodes[report.byte_arrays[frame.memory].symbolic.node]
    else {
        panic!("memory retains its typed view")
    };
    let api::Expression::Copy(copy) = &report.symbolic.nodes[memory.state.node] else {
        panic!("the successful side commits the lazy copy")
    };
    assert!(matches!(
        report.symbolic.nodes[copy.previous.node],
        api::Expression::BytesZero
    ));
    assert_eq!(
        report.symbolic.nodes[copy.destination.node],
        api::Expression::Constant("0x0".into())
    );
    assert_eq!(copy.length, length);
    let api::Expression::View(source) = &report.symbolic.nodes[copy.source.node] else {
        panic!("the copy captures the immutable calldata view")
    };
    let api::Expression::View(input_view) = &report.symbolic.nodes[input.symbolic.node] else {
        panic!("calldata retains its input view")
    };
    assert_eq!(source.state, input_view.state);
    assert_eq!(source.offset, input_view.offset);
    assert_eq!(source.length, length);
    let api::Expression::PathAnd(paths) = &report.symbolic.nodes[exit.path.node] else {
        panic!("success retains the logical range assumption")
    };
    let guard = paths
        .iter()
        .find_map(|path| {
            let api::Expression::PathAtoms(atoms) = &report.symbolic.nodes[path.node] else {
                return None;
            };
            atoms.iter().find_map(|atom| match atom {
                api::PathConstraint::Truth(truth) if truth.nonzero => Some(truth.expression),
                _ => None,
            })
        })
        .unwrap();
    let api::Expression::Operation(guard) = &report.symbolic.nodes[guard.node] else {
        panic!("the memory guard retains its empty-or-fitting predicate")
    };
    assert_eq!(guard.opcode, 0x17);
    assert_eq!(guard.arguments.len(), 2);
    let api::Expression::Operation(empty) = &report.symbolic.nodes[guard.arguments[0].node] else {
        panic!("the empty branch retains ISZERO(length)")
    };
    assert_eq!(empty.opcode, 0x15);
    assert_eq!(empty.arguments, [length]);
    let api::Expression::Operation(fits) = &report.symbolic.nodes[guard.arguments[1].node] else {
        panic!("the nonempty branch retains both endpoint checks")
    };
    assert_eq!(fits.opcode, 0x16);
    assert_eq!(fits.arguments.len(), 2);
    assert_eq!(
        report.symbolic.nodes[fits.arguments[0].node],
        api::Expression::Constant("0x1".into())
    );
    let api::Expression::Operation(size_fits) = &report.symbolic.nodes[fits.arguments[1].node]
    else {
        panic!("the requested size retains its upper bound")
    };
    assert_eq!(size_fits.opcode, 0x15);
    assert_eq!(size_fits.arguments.len(), 1);
    let api::Expression::Operation(too_large) = &report.symbolic.nodes[size_fits.arguments[0].node]
    else {
        panic!("the size check uses the original calldata length")
    };
    assert_eq!(too_large.opcode, 0x11);
    assert_eq!(too_large.arguments.len(), 2);
    assert_eq!(too_large.arguments[0], length);
    assert_eq!(
        report.symbolic.nodes[too_large.arguments[1].node],
        api::Expression::Constant(format!("0x{}e0", "ff".repeat(31)))
    );
    assert!(frame.stack.is_empty());
    let copy_instruction = report.ssa.blocks[0]
        .instructions
        .iter()
        .find(|instruction| instruction.pc == 4)
        .unwrap();
    assert_eq!(copy_instruction.opcode, 0x37);
    assert_eq!(
        copy_instruction.progress,
        api::InstructionProgress::Completed
    );
    assert_eq!(copy_instruction.operands.len(), 3);
    assert_eq!(copy_instruction.operands[0], copy_instruction.operands[1]);
    assert!(
        matches!(&report.ssa.values[copy_instruction.operands[2]].origin,
        api::SsaValueOrigin::Instruction(origin) if origin.pc == 0)
    );
    assert!(copy_instruction.results.is_empty());
    assert!(copy_instruction.effect_result.is_some());

    // Three nodes retain the original calldata input. The four-node
    // ISZERO(GT(length, MAX_END)) cannot be retained, so mandatory partitioning
    // stops before expansion/COPY and records the entire pre-effect input.
    let mut input = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "366000803700".into(),
            address: Some(Address::repeat_byte(0x11).to_string()),
        }),
        ..api::AnalyzeRequest::default()
    };
    input.limits.root_terminal_coverage = false;
    input.limits.max_expression_nodes = 3;
    let partial = roundtrip(crate::analyze::analyze(input).unwrap());
    assert_eq!(partial.status, api::AnalysisStatus::Incomplete);
    assert!(!partial.ssa.complete);
    assert_eq!(partial.metadata.limits.max_expression_nodes, 3);
    let [frontier] = partial.frontiers.as_slice() else {
        panic!("expected only the mandatory logical range refusal")
    };
    assert_eq!(frontier.kind, api::FrontierKind::Relations);
    assert_eq!(
        frontier.reason,
        api::FrontierDetails::Relations(api::QueryBoundary::ExpressionLimit)
    );
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
    let range = frontier.range.as_ref().unwrap();
    assert_eq!(range.request.kind, api::RangeKind::LogicalMemory);
    assert_eq!(range.request.offset.constants, Some(vec!["0x0".into()]));
    assert_eq!(
        range.request.limit.constants,
        Some(vec![format!("0x{}e0", "ff".repeat(31))])
    );
    let length = partial.byte_arrays[partial.states[0].entry.frames[0].calldata]
        .length
        .expression
        .unwrap();
    assert_eq!(range.request.size.expression, Some(length));
    assert!(range.request.size.constants.is_none());
    assert!(
        range.condition.expression.is_none(),
        "the refused guard cannot be fabricated"
    );
    assert_eq!(range.nonzero, None);
    let pending = &partial.pending_machines[range.payload];
    let frame = pending.frames.last().unwrap();
    let memory = &partial.byte_arrays[frame.memory];
    assert_eq!(memory.length.constants, Some(vec!["0x0".into()]));
    let api::Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
        panic!("the pending state retains the unchanged memory view")
    };
    assert!(matches!(
        partial.symbolic.nodes[view.state.node],
        api::Expression::BytesZero
    ));
    assert!(frame.stack.is_empty());
    let state = &partial.states[0];
    let exit = state.exit.as_ref().unwrap();
    assert_eq!(pending.frames, exit.frames);
    assert_eq!(pending.store, exit.store);
    assert_eq!(exit.store, state.entry.store);
    let instruction = partial.ssa.blocks[0].instructions.last().unwrap();
    assert_eq!((instruction.pc, instruction.opcode), (4, 0x37));
    assert_eq!(
        instruction.progress,
        api::InstructionProgress::OperandsConsumed
    );
    assert_eq!(instruction.operands.len(), 3);
    assert_eq!(instruction.operands[0], instruction.operands[1]);
    assert!(
        matches!(&partial.ssa.values[instruction.operands[2]].origin,
        api::SsaValueOrigin::Instruction(origin) if origin.pc == 0)
    );
    assert!(instruction.results.is_empty());
    assert!(instruction.effect_result.is_some());
    assert!(matches!(
        instruction.operation,
        api::SsaOperation::Opcode(_)
    ));
    assert!(!partial.ssa.values.iter().any(|value| {
        matches!(&value.origin, api::SsaValueOrigin::Instruction(origin) if origin.pc == 4)
    }));
    assert!(
        partial
            .symbolic
            .nodes
            .iter()
            .all(|node| !matches!(node, api::Expression::Copy(_)))
    );
    assert!(
        partial
            .outcomes
            .iter()
            .all(|outcome| outcome.kind == api::OutcomeKind::Failure)
    );
}
#[test]
fn legal_twenty_four_kib_codecopy_exports_one_lazy_copy_and_exact_source() {
    let source = format!("616000600a5f3900{}", "00".repeat(24 * 1024 - 8));
    let report = roundtrip(analyze_hex(&source));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert_eq!(
        report
            .symbolic
            .nodes
            .iter()
            .filter(|node| matches!(node, api::Expression::Copy(_)))
            .count(),
        1
    );
    assert!(
        report.symbolic.nodes.iter().any(
            |node| matches!(node,api::Expression::BytesExact(bytes) if bytes.len()==2+24*1024*2)
        )
    );
    assert!(report.symbolic.nodes.len() < 64);
    assert!(report.metadata.export_usage.exact_bytes >= 24 * 1024);
}
#[test]
fn symbolic_export_limits_and_cancellation_are_typed_without_changing_native_values() {
    use evm_abstract::domain::{AbstractValue, symbolic::ExprId};
    let control = Control::default();
    let source = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let retained = source.clone();
    let mut pool = super::super::symbolic::Pool::new(&control);
    pool.limits.max_nodes = 0;
    let failure = pool.value(&source).unwrap_err();
    let api::ErrorDetails::Export(detail) = failure.details else {
        panic!("expected an export failure");
    };
    assert!(matches!(
        *detail,
        api::ExportFailure {
            kind: api::ExportFailureKind::Nodes,
            ..
        }
    ));
    assert_eq!(detail.limits, pool.limits);
    assert_eq!(detail.usage, pool.usage);
    assert_eq!(detail.written, None);
    assert_eq!(source, retained);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let mut pool = super::super::symbolic::Pool::new(&control);
    let failure = pool.value(&source).unwrap_err();
    assert_eq!(failure.code, api::ApiErrorCode::Cancelled);
    let api::ErrorDetails::Export(detail) = failure.details else {
        panic!("expected an export failure");
    };
    assert!(matches!(
        *detail,
        api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            ..
        }
    ));
    assert_eq!(detail.limits, pool.limits);
    assert_eq!(detail.usage, pool.usage);
    assert_eq!(detail.written, None);
    assert_eq!(source, retained);
}

#[test]
fn mixed_projection_keeps_binding_scopes_sharing_fresh_numbers_and_work() {
    for width in [1, 4096] {
        mixed_projection(width);
    }
}

fn mixed_projection(width: usize) {
    use alloy_primitives::U256;
    use evm_abstract::domain::{
        AbstractValue, Domain,
        identity::Symbol,
        relational::{CheckResult, RelationLimits, RelationState},
        symbolic::{
            ByteStateId, ByteView, ExprId, SlotStateId, SourceId, StateBinding, StateLimits,
        },
    };
    let control = Control::default();
    let mut pool = super::super::symbolic::Pool::new(&control);
    let token = ExprId::fresh().unwrap();
    let length = ExprId::input(u64::MAX, Symbol::CalldataLength);
    let cell = ExprId::fresh().unwrap();
    let numeric = Domain::default()
        .join(
            &AbstractValue::constant(U256::from(7)),
            &AbstractValue::constant(U256::from(9)),
        )
        .numeric()
        .into();
    let binding = StateBinding {
        index: U256::from(2),
        value: cell.clone(),
        numeric,
    };
    let limits = StateLimits {
        max_nodes: 3 + width * (1 + binding.numeric.work_size()),
        ..StateLimits::default()
    };
    let source = SourceId::scoped(token.clone(), U256::from(0x123));
    let bytes = ByteStateId::observed(
        source.clone(),
        length.clone(),
        (0..width)
            .map(|index| StateBinding {
                index: U256::from(index + 2),
                ..binding.clone()
            })
            .collect(),
        limits,
    )
    .unwrap();
    let view = ByteView::new(bytes, ExprId::constant(U256::ZERO), length.clone());
    let root = pool.view(&view).unwrap();
    assert_eq!(root.node, 5);
    assert_eq!(
        pool.usage,
        api::ExportUsage {
            nodes: 6,
            exact_bytes: 0,
            work: 23 + 5 * (width as u64 - 1)
        }
    );
    assert_eq!(pool.view(&view).unwrap(), root);
    assert_eq!(pool.usage.work, 24 + 5 * (width as u64 - 1));
    assert_eq!(pool.graph.nodes[0], api::Expression::Fresh(0));
    assert_eq!(pool.graph.nodes[2], api::Expression::Fresh(1));
    let api::Expression::BytesInput(input) = &pool.graph.nodes[3] else {
        panic!("source input")
    };
    assert_eq!(input.source.namespace, "0x123");
    assert_eq!(input.source.token.node, 0);
    assert_eq!(input.length.node, 1);
    assert_eq!(input.bindings[0].value.node, 2);
    assert_eq!(input.bindings.len(), width);
    assert!(input.bindings.iter().all(|binding| binding.value.node == 2));
    assert_eq!(
        input.bindings[0].numeric.constants,
        Some(vec!["0x7".into(), "0x9".into()])
    );
    assert_eq!(input.bindings[0].numeric.expression, None);
    let api::Expression::Input(identity) = &pool.graph.nodes[1] else {
        panic!("scoped length")
    };
    assert_eq!(identity.scope, api::DecimalU64(u64::MAX));
    let slots = SlotStateId::observed(
        source,
        (0..width)
            .map(|index| StateBinding {
                index: U256::from(index + 2),
                ..binding.clone()
            })
            .collect(),
        limits,
    )
    .unwrap();
    let work = pool.usage.work;
    let slots = pool.slots(&slots).unwrap();
    assert_eq!(pool.usage.work - work, 5 * width as u64 + 5);
    let api::Expression::SlotsInput(input) = &pool.graph.nodes[slots.node] else {
        panic!("storage source")
    };
    assert_eq!(input.source.token.node, 0);
    assert_eq!(input.source.namespace, "0x123");
    assert_eq!(input.bindings[0].value.node, 2);
    assert_eq!(input.bindings.len(), width);
    assert!(input.bindings.iter().all(|binding| binding.value.node == 2));

    let mut relations = RelationState::default();
    assert_eq!(
        relations.add_equal(&cell, &length, &RelationLimits::default()),
        CheckResult::MayBeFeasible
    );
    let path = pool.relations(&relations).unwrap();
    assert_eq!(pool.relations(&relations).unwrap(), path);
    assert!(
        pool.graph
            .nodes
            .iter()
            .any(|node| matches!(node, api::Expression::PathAtoms(_)))
    );
    let conditional = relations
        .join_guarded(
            &RelationState::default(),
            ExprId::fresh().unwrap(),
            &RelationLimits::default(),
        )
        .unwrap();
    let conditional = pool.path(conditional.path()).unwrap();
    let api::Expression::PathIte(path) = &pool.graph.nodes[conditional.node] else {
        panic!("mixed word/path dependencies");
    };
    assert_eq!(
        pool.graph.nodes[path.selector.node],
        api::Expression::Fresh(2)
    );
    assert!(matches!(
        pool.graph.nodes[path.then_path.node],
        api::Expression::PathAtoms(_)
    ));
    assert_eq!(
        pool.graph.nodes[path.else_path.node],
        api::Expression::PathTrue
    );
    pool.graph.validate().unwrap();
    let encoded = serde_json::to_vec(&pool.graph).unwrap();
    let parsed: api::SymbolicGraph = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(parsed, pool.graph);
    parsed.validate().unwrap();
}
