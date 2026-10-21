//! Regression coverage for the native-to-browser semantic boundary.

use evm_abstract_protocol::{
    AnalysisStatus, AnalyzeReply, AnalyzeRequest, ApiErrorCode, BlockCoverage, FrontierKind,
    InstructionProgress,
};
use evm_abstract_server::analyze::analyze;

fn request(bytecode: &str) -> AnalyzeRequest {
    let mut request = AnalyzeRequest {
        input: evm_abstract_protocol::AnalysisInput::Bytecode(
            evm_abstract_protocol::BytecodeInput {
                bytecode: bytecode.into(),
                address: None,
            },
        ),
        ..AnalyzeRequest::default()
    };
    request.limits.context_depth = 0;
    request
}

fn byte_contains(value: &evm_abstract_protocol::ValueInfo, byte: u8) -> bool {
    use alloy_primitives::U256;
    use evm_abstract_protocol::CongruenceValue;
    let word = |text: &str| text.parse::<U256>().unwrap();
    let byte = U256::from(byte);
    let sign = U256::from(1) << 255;
    value
        .constants
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| word(value) == byte))
        && byte & word(&value.known_zero) == U256::ZERO
        && byte & word(&value.known_one) == word(&value.known_one)
        && (!value.nonzero || byte != U256::ZERO)
        && word(&value.unsigned.lower) <= byte
        && byte <= word(&value.unsigned.upper)
        && word(&value.unsigned.upper) <= U256::from(255)
        && (word(&value.signed.lower) ^ sign) <= (byte ^ sign)
        && (byte ^ sign) <= (word(&value.signed.upper) ^ sign)
        && match &value.congruence {
            CongruenceValue::Any => true,
            CongruenceValue::Exact(exact) => byte == word(exact),
            CongruenceValue::Modulo(class) => byte % word(&class.modulus) == word(&class.residue),
        }
}

// The protocol exports stored numeric upper bounds separately from its exact
// immutable root. Decode only the concrete constructors used by these fixtures;
// an unsupported or unknown root cannot satisfy the full-content assertion.
fn native_view(
    graph: &evm_abstract_protocol::SymbolicGraph,
    node: usize,
    limits: evm_abstract::domain::symbolic::ExprLimits,
) -> Option<evm_abstract::domain::symbolic::ByteView> {
    use evm_abstract::domain::symbolic::{ByteStateId, ByteView, ExprId, ExprLimits};
    use evm_abstract_protocol::{Expression, SymbolicGraph};
    fn scalar(graph: &SymbolicGraph, node: usize) -> Option<ExprId> {
        let Expression::Constant(value) = graph.nodes.get(node)? else {
            return None;
        };
        Some(ExprId::constant(value.parse().ok()?))
    }
    fn state(graph: &SymbolicGraph, node: usize, limits: ExprLimits) -> Option<ByteStateId> {
        match graph.nodes.get(node)? {
            Expression::BytesZero => Some(ByteStateId::zero()),
            Expression::BytesExact(hex) => Some(ByteStateId::exact(
                alloy_primitives::hex::decode(hex).ok()?.into(),
            )),
            Expression::WriteWord(write) => state(graph, write.previous.node, limits)?
                .write_word(
                    scalar(graph, write.offset.node)?,
                    scalar(graph, write.value.node)?,
                    limits.into(),
                )
                .ok(),
            Expression::Copy(copy) => state(graph, copy.previous.node, limits)?
                .copy(
                    scalar(graph, copy.destination.node)?,
                    native_view(graph, copy.source.node, limits)?,
                    scalar(graph, copy.length.node)?,
                    limits.into(),
                )
                .ok(),
            Expression::Slice(source) => {
                let source = native_view(graph, source.node, limits)?;
                source
                    .state
                    .slice(source.offset, source.length, limits.into())
                    .ok()
            }
            _ => None,
        }
    }
    let Expression::View(view) = graph.nodes.get(node)? else {
        return None;
    };
    Some(ByteView::new(
        state(graph, view.state.node, limits)?,
        scalar(graph, view.offset.node)?,
        scalar(graph, view.length.node)?,
    ))
}

fn buffer_matches(
    report: &evm_abstract_protocol::AnalysisReport,
    bytes: &evm_abstract_protocol::ByteArraySnapshot,
    expected: &[u8],
) -> bool {
    use evm_abstract::domain::symbolic::ExprLimits;
    let limits = ExprLimits {
        max_nodes: report
            .metadata
            .limits
            .max_expression_nodes
            .try_into()
            .unwrap(),
        max_depth: report
            .metadata
            .limits
            .max_expression_depth
            .try_into()
            .unwrap(),
    };
    bytes.length.constants == Some(vec![format!("0x{:x}", expected.len())])
        && native_view(&report.symbolic, bytes.symbolic.node, limits)
            .and_then(|view| view.canonicalize().known_bytes(limits).ok().flatten())
            .as_deref()
            == Some(expected)
        && bytes
            .cells
            .windows(2)
            .all(|pair| pair[0].offset < pair[1].offset)
        && bytes.cells.iter().all(|cell| {
            expected.get(cell.offset).is_some_and(|byte| {
                bytes
                    .values
                    .get(cell.value)
                    .is_some_and(|value| byte_contains(value, *byte))
            })
        })
        && expected.iter().enumerate().all(|(offset, byte)| {
            bytes
                .cells
                .binary_search_by_key(&offset, |cell| cell.offset)
                .is_ok()
                || byte_contains(&bytes.default, *byte)
        })
}

#[test]
fn invalid_opcode_report_preserves_original_bytes_names_causes_and_closure() {
    use evm_abstract_protocol::{
        DiagnosticKind, Fork, InvalidInstruction, InvalidOpcodeDiagnostic, InvalidOpcodeReason,
    };
    for (bytecode, fork, pc, evidence) in [
        (
            "fe",
            Fork::Osaka,
            0,
            InvalidInstruction {
                opcode: 0xfe,
                mnemonic: Some("INVALID".into()),
                reason: InvalidOpcodeReason::ExplicitInvalid,
            },
        ),
        (
            "ef",
            Fork::Osaka,
            0,
            InvalidInstruction {
                opcode: 0xef,
                mnemonic: None,
                reason: InvalidOpcodeReason::UndefinedOpcode,
            },
        ),
        (
            "5f1e00",
            Fork::Cancun,
            1,
            InvalidInstruction {
                opcode: 0x1e,
                mnemonic: Some("CLZ".into()),
                reason: InvalidOpcodeReason::UnavailableInFork(Fork::Cancun),
            },
        ),
    ] {
        let mut input = request(bytecode);
        input.fork = fork;
        let report = analyze(input).unwrap();
        assert_eq!(report.schema_version, evm_abstract_protocol::SCHEMA_VERSION);
        assert_eq!(report.status, AnalysisStatus::Converged);
        assert!(report.frontiers.is_empty());
        let diagnostics: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::InvalidOpcode)
            .collect();
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics[0];
        assert_eq!(diagnostic.pc, pc);
        assert_eq!(
            diagnostic.invalid_opcode,
            Some(InvalidOpcodeDiagnostic::Instruction(evidence.clone()))
        );
        assert!(
            diagnostic
                .detail
                .contains(&format!("0x{:02x}", evidence.opcode))
        );
        let state = &report.states[diagnostic.state];
        let source = &report.programs[state.entry.frames.last().unwrap().program.unwrap()];
        let bytes = alloy_primitives::hex::decode(&source.bytecode).unwrap();
        assert_eq!(bytes[diagnostic.pc], evidence.opcode);
        assert_eq!(
            serde_json::from_slice::<evm_abstract_protocol::AnalysisReport>(
                &serde_json::to_vec(&report).unwrap()
            )
            .unwrap(),
            report
        );
    }
    let mut input = request("5f1e00");
    input.fork = Fork::Osaka;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.kind != DiagnosticKind::InvalidOpcode)
    );
}

#[test]
fn unreachable_invalid_bytes_are_only_static_source_and_other_halts_keep_categories() {
    use evm_abstract_protocol::DiagnosticKind;
    let report = analyze(request("00feef")).unwrap();
    assert!(report.diagnostics.is_empty());
    assert!(
        report
            .disassembly
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| instruction.opcode == 0xfe && !instruction.valid)
    );
    for (bytecode, kind) in [
        ("5f56", DiagnosticKind::InvalidJump),
        ("01", DiagnosticKind::StackUnderflow),
    ] {
        let report = analyze(request(bytecode)).unwrap();
        assert_eq!(report.status, AnalysisStatus::Converged);
        assert!(report.frontiers.is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == kind && diagnostic.invalid_opcode.is_none())
        );
    }
}

#[test]
fn diamond_preserves_phi_predecessors_and_structural_json_roundtrip() {
    let report = analyze(request(include_str!("../../../examples/diamond.hex"))).unwrap();
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.ssa.complete);
    let timings = &report.metadata.timings;
    assert_eq!(timings.queue_ns, 0, "direct analyses have no worker queue");
    assert!(timings.validating_ns > 0);
    assert!(timings.analyzing_ns > 0);
    assert!(timings.building_ssa_ns > 0);
    assert!(timings.projecting_ns > 0);
    assert_eq!(timings.pinning_ns, 0);
    assert_eq!(timings.acquiring_ns, 0);
    assert_eq!(
        timings.processing_ns,
        timings.validating_ns
            + timings.analyzing_ns
            + timings.building_ssa_ns
            + timings.projecting_ns
    );
    assert!(
        report
            .ssa
            .blocks
            .iter()
            .flat_map(|block| &block.phis)
            .any(|phi| phi.inputs.len() == 2)
    );
    for input in report
        .ssa
        .blocks
        .iter()
        .flat_map(|block| &block.phis)
        .flat_map(|phi| &phi.inputs)
    {
        assert_eq!(report.edges[input.edge].from, input.predecessor);
        assert!(input.value < report.ssa.value_count);
    }
    let reply = AnalyzeReply { result: Ok(report) };
    let bytes = serde_json::to_vec(&reply).unwrap();
    let decoded: AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, reply);
}

#[test]
fn direct_failures_and_cancellation_freeze_the_control_timing() {
    use evm_abstract::analysis::{control::Control, progress::Observer};
    use evm_abstract_server::analyze::analyze_with_control;

    for cancelled in [false, true] {
        let control = Control::new(Observer::default());
        if cancelled {
            control.cancellation().cancel();
        }
        let error = analyze_with_control(request("gg"), &control).unwrap_err();
        assert_eq!(
            error.code,
            if cancelled {
                ApiErrorCode::Cancelled
            } else {
                ApiErrorCode::InvalidBytecode
            }
        );
        let frozen = control.observer().timing();
        assert!(frozen.elapsed_ns > 0);
        assert_eq!(frozen.elapsed_ns, frozen.phases.validating_ns);
        assert_eq!(control.observer().timing(), frozen);
    }
}

#[test]
fn reusing_a_frozen_direct_control_starts_a_separate_timing_session() {
    use evm_abstract::analysis::control::Control;
    use evm_abstract_server::analyze::analyze_with_control;

    let control = Control::default();
    let report = analyze_with_control(
        request(include_str!("../../../examples/diamond.hex")),
        &control,
    )
    .unwrap();
    let first = control.observer().timing();
    assert!(first.phases.building_ssa_ns > 0);
    let error = analyze_with_control(request("gg"), &control).unwrap_err();
    assert_eq!(error.code, ApiErrorCode::InvalidBytecode);
    let second = control.observer().timing();
    assert!(second.started_at > first.started_at);
    assert!(second.elapsed_ns > 0);
    assert_eq!(second.elapsed_ns, second.phases.validating_ns);
    assert_eq!(second.phases.analyzing_ns, 0);
    assert_eq!(second.phases.building_ssa_ns, 0);
    assert_eq!(second.phases.projecting_ns, 0);
    assert_eq!(report.metadata.timings.processing_ns, first.elapsed_ns);
    assert_eq!(control.observer().timing(), second);
}

#[test]
fn loop_has_backedge_and_phi_without_infinite_names() {
    let report = analyze(request(include_str!("../../../examples/loop.hex"))).unwrap();
    assert!(report.ssa.complete);
    assert!(report.edges.iter().any(|edge| edge.to <= edge.from));
    assert!(
        report
            .ssa
            .blocks
            .iter()
            .any(|block| block.phis.iter().any(|phi| phi.inputs.len() > 1))
    );
    assert!(report.ssa.value_count < 100);
}

#[test]
fn truncated_graph_keeps_unexecuted_blocks_and_open_coverage() {
    let mut input = request(include_str!("../../../examples/diamond.hex"));
    input.limits.max_transfers = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == FrontierKind::Transfers)
    );
    assert!(
        report
            .ssa
            .blocks
            .iter()
            .all(|block| !block.incoming_complete)
    );
    let unexecuted = report
        .ssa
        .blocks
        .iter()
        .find(|block| block.coverage == BlockCoverage::Unexecuted)
        .unwrap();
    assert!(unexecuted.instructions.is_empty());
}

#[test]
fn bounded_call_retains_pending_phase_and_no_fabricated_result() {
    let mut input = request("5f5f5f5f5f5f355af100");
    input.limits.max_call_depth = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Incomplete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == FrontierKind::CallDepth)
    );
    let call = report
        .ssa
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find(|instruction| instruction.opcode == 0xf1)
        .unwrap();
    // Immediate failure remains possible; dispatch does not claim a completed
    // callee when the configured frame limit prevents its entry.
    assert_eq!(call.progress, InstructionProgress::Dispatched);
    assert!(call.results.is_empty());
    assert!(!report.ssa.complete);
}

#[test]
fn stale_loop_receipt_has_no_body_and_keeps_deferred_edges() {
    let mut input = request("5f5b600101600156");
    input.limits.max_transfers = 2;
    let report = analyze(input).unwrap();
    let stale = report
        .ssa
        .blocks
        .iter()
        .find(|block| block.coverage == BlockCoverage::Stale)
        .unwrap();
    assert!(stale.instructions.is_empty());
    assert!(!stale.open_incoming.is_empty());
    assert!(
        report
            .ssa
            .deferred_edges
            .iter()
            .any(|edge| report.edges[edge.edge].from == stale.state)
    );
    assert_eq!(report.status, AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
}

#[test]
fn fork_and_invalid_input_remain_explicit() {
    let mut input = request("1e");
    input.fork = evm_abstract_protocol::Fork::Cancun;
    let report = analyze(input).unwrap();
    assert!(!report.disassembly[0].instructions[0].valid);
    assert_eq!(
        analyze(request("xyz")).unwrap_err().code,
        ApiErrorCode::InvalidBytecode
    );
    let mut input = request("00");
    input.limits.max_states = 0;
    assert_eq!(
        analyze(input).unwrap_err().code,
        ApiErrorCode::InvalidLimits
    );
}

#[test]
fn empty_program_is_complete_and_truncated_push_keeps_padding() {
    assert!(analyze(request("")).unwrap().ssa.complete);
    let report = analyze(request("61ab")).unwrap();
    let instruction = &report.disassembly[0].instructions[0];
    assert_eq!(instruction.immediate.as_deref(), Some("0xab00"));
    assert_eq!(instruction.size, 3);
}

#[test]
fn world_payload_keeps_exact_memory_storage_transient_and_terminal_bytes() {
    use evm_abstract_protocol::{CalldataInput, OutcomeKind, WordInput};
    let mut input = request("602a5f5560075f5d602a5f5260205ff3");
    input.environment.calldata = CalldataInput::Exact("0xaabb".into());
    input.environment.call_value = WordInput::Concrete("0".into());
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(report.ssa.complete);
    assert_eq!(report.states.len(), report.cfg.len());
    let root = &report.states[0].entry.frames[0];
    assert!(buffer_matches(
        &report,
        &report.byte_arrays[root.calldata],
        &[0xaa, 0xbb]
    ));
    assert_eq!(
        report.byte_arrays[root.calldata]
            .length
            .constants
            .as_deref(),
        Some(&["0x2".into()][..])
    );
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.kind == OutcomeKind::Return)
        .expect("a committing execution remains possible");
    let data = &report.byte_arrays[outcome.data];
    assert_eq!(data.length.constants.as_deref(), Some(&["0x20".into()][..]));
    let mut word = [0; 32];
    word[31] = 42;
    assert!(buffer_matches(&report, data, &word));
    assert_eq!(data.cells.len(), word.len());
    for (offset, cell) in data.cells.iter().enumerate() {
        assert_eq!(cell.offset, offset);
    }
    let mut wrong = word;
    wrong[15] = 1;
    assert!(!buffer_matches(&report, data, &wrong));
    let mut excluded = data.clone();
    let value = excluded.cells[31].value;
    excluded.values[value].constants = Some(vec!["0x2b".into()]);
    assert!(!buffer_matches(&report, &excluded, &word));
    let account = report.stores[outcome.store]
        .accounts
        .iter()
        .find(|account| account.address == root.storage_address)
        .unwrap();
    assert!(
        account
            .storage
            .iter()
            .any(|slot| slot.slot == "0x0" && slot.value.constants == Some(vec!["0x2a".into()]))
    );
    assert!(
        account.transient.is_empty(),
        "transaction-finalized transient slots are cleared"
    );
    assert!(matches!(
        report.symbolic.nodes[account.transient_root.node],
        evm_abstract_protocol::Expression::SlotsZero
    ));
    let exit = report.states[outcome.state].exit.as_ref().unwrap();
    assert!(buffer_matches(
        &report,
        &report.byte_arrays[exit.frames[0].memory],
        &word
    ));
    assert!(
        report.stores[exit.store].accounts.iter().any(|account| {
            account.address == root.storage_address
                && account.transient.iter().any(|slot| {
                    slot.slot == "0x0" && slot.value.constants == Some(vec!["0x7".into()])
                })
        }),
        "the executed write remains visible before transaction finalization"
    );
    assert!(
        report
            .accounts
            .iter()
            .all(|account| account.storage.is_empty())
    );
}
#[test]
fn caller_origin_and_unknown_logical_address_survive_bytecode_world_entry() {
    use evm_abstract_protocol::{AddressValue, InputSymbolKind};
    let report = analyze(request("3032333400")).unwrap();
    let environment = &report.metadata.environment;
    assert!(
        matches!(&environment.to,AddressValue::Symbolic(symbol) if symbol.kind==InputSymbolKind::To)
    );
    assert_eq!(environment.caller, environment.origin);
    assert!(environment.call_value.constants.is_none());
    let exit = report.states[0].exit.as_ref().unwrap();
    assert_eq!(exit.frames[0].stack.len(), 4);
    assert!(exit.frames[0].stack[0].constants.is_none());
}
#[test]
fn typed_admission_and_precancelled_requests_never_fabricate_reports() {
    use evm_abstract::analysis::{control::Control, progress::Observer};
    use evm_abstract_protocol::{
        BytecodeFailure, EnvironmentFailure, ErrorDetails, IndexedHash, WordInput,
    };
    assert!(
        matches!(analyze(request("0x00gg")).unwrap_err().details,ErrorDetails::Bytecode(BytecodeFailure::InvalidHex(failure)) if failure.index==2&&failure.character=='g')
    );
    let mut input = request("00");
    input.environment.blob_count = WordInput::Concrete("0".into());
    input.environment.blob_hashes.push(IndexedHash {
        index: "0".into(),
        hash: format!("0x{}", "11".repeat(32)),
    });
    assert!(
        matches!(analyze(input).unwrap_err().details,ErrorDetails::Environment(EnvironmentFailure::BlobIndex(failure)) if failure.index=="0x0"&&failure.count=="0x0")
    );
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    assert_eq!(
        evm_abstract_server::analyze::analyze_with_control(request("00"), &control)
            .unwrap_err()
            .code,
        ApiErrorCode::Cancelled
    );
}

#[test]
fn source_directory_remains_available_when_work_stops_before_root_state() {
    let mut input = request("602a00");
    input.limits.root_terminal_coverage = false;
    input.limits.max_work = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Incomplete);
    assert_eq!(report.metadata.limits.max_work, 1);
    assert_eq!(
        report.metadata.postprocessing_max_work,
        evm_abstract_server::analyze::DEFAULT_POSTPROCESS_WORK as u64
    );
    assert!(report.metadata.postprocessing_work <= report.metadata.postprocessing_max_work);
    report.validate().unwrap();
    let source = &report.programs[report
        .metadata
        .root_program
        .expect("submitted source still exists before execution")];
    assert_eq!(source.bytecode, "0x602a00");
    assert_eq!(report.disassembly, source.blocks);
}

#[test]
fn dense_input_and_lazy_copy_share_complete_values_without_losing_content() {
    let mut input = request("365f5f37365ff3");
    input.environment.calldata =
        evm_abstract_protocol::CalldataInput::Exact(format!("0x{}", "aa".repeat(1024)));
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(report.ssa.complete);
    let expected = vec![0xaa; 1024];
    let dense: Vec<_> = report
        .byte_arrays
        .iter()
        .filter(|bytes| bytes.cells.len() == 1024)
        .collect();
    assert!(
        !dense.is_empty(),
        "the exact input retains every observed byte cell"
    );
    for bytes in &dense {
        assert!(buffer_matches(&report, bytes, &expected));
        assert_eq!(
            bytes.values.len(),
            1,
            "equal full scalar facts share a dictionary entry"
        );
        for (offset, cell) in bytes.cells.iter().enumerate() {
            assert_eq!(cell.offset, offset);
            let value = &bytes.values[cell.value];
            assert_eq!(value.constants, Some(vec!["0xaa".into()]));
            assert!(value.origins.is_some());
        }
    }
    let memory = report
        .byte_arrays
        .iter()
        .find(|bytes| bytes.memory && bytes.length.constants == Some(vec!["0x400".into()]))
        .unwrap();
    assert!(
        memory.cells.len() < 1024,
        "the concrete fast path is a bounded observation cache, not a full copy expansion"
    );
    assert!(!memory.cells.is_empty());
    assert!(buffer_matches(&report, memory, &expected));
    for (offset, cell) in memory.cells.iter().enumerate() {
        assert_eq!(cell.offset, offset);
        let value = &memory.values[cell.value];
        assert!(byte_contains(value, expected[offset]));
        // The presentation has no literal certificate while the length's
        // calldata provenance is retained in operand facts and the source view.
        assert_eq!(value.origins, None);
        assert_eq!(value.expression, None);
        assert!(!value.symbolic_limit);
    }
    let returned = report
        .outcomes
        .iter()
        .find(|outcome| outcome.kind == evm_abstract_protocol::OutcomeKind::Return)
        .unwrap();
    assert!(buffer_matches(
        &report,
        &report.byte_arrays[returned.data],
        &expected
    ));
    for offset in [0, 511, 512, 1023] {
        let mut wrong = expected.clone();
        wrong[offset] = 0xab;
        assert!(!buffer_matches(&report, memory, &wrong));
        assert!(!buffer_matches(
            &report,
            &report.byte_arrays[returned.data],
            &wrong
        ));
    }
    let mut excluded = (*dense[0]).clone();
    excluded.values[0].constants = Some(vec!["0xab".into()]);
    assert!(!buffer_matches(&report, &excluded, &expected));
    let mut invalid = memory.clone();
    invalid.cells[0].value = invalid.values.len();
    assert!(!buffer_matches(&report, &invalid, &expected));
    let mut invalid_offset = memory.clone();
    invalid_offset.cells[0].offset = expected.len();
    assert!(!buffer_matches(&report, &invalid_offset, &expected));
    let mut invalid_length = memory.clone();
    invalid_length.length.constants = Some(vec!["0x3ff".into()]);
    assert!(!buffer_matches(&report, &invalid_length, &expected));
    let mut excluded_bound = memory.clone();
    let value = excluded_bound.cells[0].value;
    excluded_bound.values[value].unsigned.lower = "0xab".into();
    assert!(!buffer_matches(&report, &excluded_bound, &expected));
    let mut missing = (*dense[0]).clone();
    missing.cells.pop();
    assert!(!buffer_matches(&report, &missing, &expected));
    let evm_abstract_protocol::Expression::View(view) =
        &report.symbolic.nodes[memory.symbolic.node]
    else {
        panic!("memory has a typed view")
    };
    let evm_abstract_protocol::Expression::Copy(copy) = &report.symbolic.nodes[view.state.node]
    else {
        panic!("memory captures the copy")
    };
    assert_eq!(
        report.symbolic.nodes[copy.destination.node],
        evm_abstract_protocol::Expression::Constant("0x0".into())
    );
    assert_eq!(
        report.symbolic.nodes[copy.length.node],
        evm_abstract_protocol::Expression::Constant("0x400".into())
    );
    assert!(memory.symbolic_facts.iter().any(|fact| {
        fact.expression == copy.length
            && fact.value.origins.as_ref().is_some_and(|origins| {
                origins.contains(&evm_abstract_protocol::ValueOrigin::Calldata)
            })
    }));
    let evm_abstract_protocol::Expression::View(source) = &report.symbolic.nodes[copy.source.node]
    else {
        panic!("copy captures the input view")
    };
    assert_eq!(
        report.symbolic.nodes[source.offset.node],
        evm_abstract_protocol::Expression::Constant("0x0".into())
    );
    assert_eq!(
        report.symbolic.nodes[source.length.node],
        evm_abstract_protocol::Expression::Constant("0x400".into())
    );
    assert_eq!(
        report.symbolic.nodes[source.state.node],
        evm_abstract_protocol::Expression::BytesExact(format!("0x{}", "aa".repeat(1024)))
    );
    report.validate().unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    assert!(
        encoded.len() < 200_000,
        "1 KiB byte fixture expanded to {} bytes",
        encoded.len()
    );
    let decoded: evm_abstract_protocol::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, report);
}

#[test]
fn user_budget_above_old_state_ceiling_builds_a_complete_graph_and_ssa() {
    // Each JUMPDEST starts a different source block. Fallthrough has no jump
    // history, so the 4097 states remain distinct under idempotent histories.
    let code = format!("{}00", "5b".repeat(4_097));
    let mut input = request(&code);
    input.limits.context_depth = 4_096;
    input.limits.max_states = 4_100;
    input.limits.max_transfers = 4_100;
    let selected_limits = input.limits.clone();
    let report = analyze(input).unwrap();
    assert_eq!(report.cfg.len(), 4_097);
    assert_eq!(report.states.len(), 4_097);
    assert_eq!(report.transfers, 4_097);
    assert_eq!(report.disassembly.len(), 4_097);
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(report.ssa.complete);
    assert_eq!(report.ssa.blocks.len(), report.cfg.len());
    for (id, block) in report.cfg.iter().enumerate() {
        assert_eq!(block.id, id);
        assert_eq!(block.basic_block, id);
        assert_eq!(block.start_pc, Some(id));
        assert!(block.context.is_empty());
        assert_eq!(report.disassembly[id].id, id);
        assert_eq!(report.disassembly[id].start_pc, id);
        let ssa = &report.ssa.blocks[id];
        assert_eq!(ssa.state, id);
        assert_eq!(ssa.coverage, BlockCoverage::Current);
        assert!(ssa.incoming_complete);
        assert!(ssa.open_incoming.is_empty());
        assert_eq!(ssa.instructions.len(), if id == 4_096 { 2 } else { 1 });
        assert!(ssa.instructions.iter().all(|instruction| {
            instruction.progress
                == if instruction.opcode == 0x00 {
                    InstructionProgress::Dispatched
                } else {
                    InstructionProgress::Completed
                }
        }));
    }
    assert_eq!(report.edges.len(), 4_096);
    for (id, edge) in report.edges.iter().enumerate() {
        assert_eq!((edge.id, edge.from, edge.to), (id, id, id + 1));
        assert_eq!(edge.kind, evm_abstract_protocol::EdgeKind::Fallthrough);
    }
    assert_eq!(report.ssa.transitions.len(), report.edges.len());
    for (id, transition) in report.ssa.transitions.iter().enumerate() {
        assert_eq!(transition.edge, id);
        assert_eq!(
            transition.kind,
            evm_abstract_protocol::EdgeKind::Fallthrough
        );
    }
    assert!(report.ssa.deferred_edges.is_empty());
    report.validate().unwrap();
    assert_eq!(report.metadata.limits, selected_limits);
}

#[test]
fn large_memory_and_work_budgets_are_effective_and_smaller_choices_still_stop() {
    use alloy_primitives::U256;
    use evm_abstract::domain::symbolic::{ByteStateKind, ExprId, ExprLimits};
    use evm_abstract_protocol::{
        AddressValue, AnalysisInput, Expression, FrontierDetails, InputSymbol, InputSymbolKind,
        SsaFactStatus, SsaOperand, SsaOperation, SsaValueOrigin,
    };

    // Repeated sparse MSTORE at 1 MiB needs a span beyond the old memory ceiling.
    // The configured finite domain makes this account for >100 million units,
    // while only 32 concrete memory bytes need storing.
    let code = format!("{}602062100000f3", "602a6210000052".repeat(20));
    let mut input = request(&code);
    input.limits.max_memory_bytes = 1_048_576 + 32;
    // Keep the detailed prefix and refusal controls independent of root preparation.
    input.limits.root_terminal_coverage = false;
    input.limits.max_work = 10_000_000_000;
    input.limits.max_constants = 512;
    let selected_limits = input.limits.clone();
    let report = analyze(input.clone()).unwrap();
    assert_eq!(
        report.status,
        AnalysisStatus::Converged,
        "{:?}",
        report.frontiers
    );
    assert!(report.ssa.complete);
    assert!(
        report.metadata.work > 100_000_000,
        "work={}",
        report.metadata.work
    );
    assert_eq!(report.metadata.limits, selected_limits);
    let returned = report
        .outcomes
        .iter()
        .find(|outcome| outcome.kind == evm_abstract_protocol::OutcomeKind::Return)
        .unwrap();
    let bytes = &report.byte_arrays[returned.data];
    assert_eq!(bytes.length.constants, Some(vec!["0x20".into()]));
    let mut word = [0; 32];
    word[31] = 42;
    assert!(buffer_matches(&report, bytes, &word));
    assert_eq!(bytes.cells.len(), 32);
    let mut wrong = word;
    wrong[15] = 1;
    assert!(!buffer_matches(&report, bytes, &wrong));
    let mut excluded = bytes.clone();
    let value = excluded.cells[31].value;
    excluded.values[value].constants = Some(vec!["0x2b".into()]);
    assert!(!buffer_matches(&report, &excluded, &word));

    // Price changes can move the refusal to a later MSTORE. Require its exact
    // current receipt and committed prefix rather than fixing a guard fee/PC.
    let mut bounded = input.clone();
    bounded.limits.max_work = 100_000_000;
    let work_limits = bounded.limits.clone();
    let partial = analyze(bounded).unwrap();
    assert_eq!(partial.metadata.limits, work_limits);
    assert_eq!(partial.status, AnalysisStatus::Incomplete);
    assert!(!partial.ssa.complete);
    assert!(partial.metadata.work > 0 && partial.metadata.work <= work_limits.max_work);
    let [frontier] = partial.frontiers.as_slice() else {
        panic!(
            "expected one pending MSTORE Work refusal: {:?}",
            partial.frontiers
        )
    };
    assert_eq!(frontier.reason, FrontierDetails::Work);
    assert_eq!(frontier.kind, FrontierKind::Work);
    assert_eq!(frontier.from, Some(0));
    assert!(frontier.range.is_none());
    assert!(frontier.payload.is_none());
    let pc = frontier.pc.expect("Work must name the attempted MSTORE");
    assert!(
        (6..7 * 20).contains(&pc),
        "Work outside the write prefix: {pc}"
    );
    assert_eq!((pc - 6) % 7, 0, "Work must stop at an original MSTORE");
    let completed_writes = (pc - 6) / 7;
    assert!(completed_writes < 20);
    let phase = if completed_writes == 0 {
        "before the first committed MSTORE"
    } else {
        "after a committed MSTORE prefix"
    };
    let [state] = partial.states.as_slice() else {
        panic!("expected only the root state {phase}")
    };
    let [cfg] = partial.cfg.as_slice() else {
        panic!("expected only the root CFG block {phase}")
    };
    let [block] = partial.ssa.blocks.as_slice() else {
        panic!("expected only the root SSA block {phase}")
    };
    assert_eq!(
        (state.state, cfg.id, block.state, cfg.basic_block),
        (0, 0, 0, 0)
    );
    assert_eq!(cfg.start_pc, Some(0));
    assert_eq!(cfg.frame_depth, 1);
    assert_eq!(block.coverage, BlockCoverage::Current);
    assert!(!block.incoming_complete);
    assert!(block.phis.is_empty());
    assert!(block.effect_inputs.is_empty());
    assert!(block.open_incoming.is_empty());
    let [entry] = state.entry.frames.as_slice() else {
        panic!("expected the original root frame")
    };
    assert_eq!(entry.basic_block, 0);
    assert!(entry.context.is_empty());
    assert!(entry.stack.is_empty());
    assert!(cfg.entry_stack.is_empty());
    assert_eq!(cfg.program, entry.program);
    let program = &partial.programs[entry.program.unwrap()];
    assert_eq!(program.bytecode, format!("0x{code}"));
    assert_eq!(program.code_address, entry.code_address);
    assert_eq!(program.code_hash, entry.code_hash);
    let [source] = program.blocks.as_slice() else {
        panic!("expected one original source block")
    };
    assert_eq!((source.id, source.start_pc), (0, 0));
    assert_eq!(cfg.instructions, source.instructions);
    let prefix: Vec<_> = (0..=completed_writes)
        .flat_map(|write| [7 * write, 7 * write + 2, 7 * write + 6])
        .collect();
    assert_eq!(cfg.executed_pcs, prefix);
    assert_eq!(block.instructions.len(), prefix.len());
    assert_eq!(
        source
            .instructions
            .iter()
            .take_while(|item| item.pc <= pc)
            .map(|item| item.pc)
            .collect::<Vec<_>>(),
        prefix
    );
    let mut effects = vec![block.effect];
    for (index, (step, original)) in block
        .instructions
        .iter()
        .zip(&source.instructions)
        .enumerate()
    {
        assert_eq!((step.pc, step.opcode), (prefix[index], original.opcode));
        assert_eq!(step.immediate, original.immediate);
        assert!(!step.fault);
        assert_eq!(
            step.progress,
            if index + 1 == prefix.len() {
                InstructionProgress::OperandsConsumed
            } else {
                InstructionProgress::Completed
            },
            "instruction {} {phase}",
            step.pc
        );
        assert_eq!(step.effect_input, *effects.last().unwrap());
        let effect = step
            .effect_result
            .expect("each observed step has an effect name");
        assert!(!effects.contains(&effect));
        effects.push(effect);
        if index % 3 == 2 {
            assert_eq!(step.opcode, 0x52);
            assert_eq!(
                step.operands,
                [
                    block.instructions[index - 1].results[0],
                    block.instructions[index - 2].results[0],
                ]
            );
            assert!(step.results.is_empty());
            let SsaOperation::Opcode(operation) = &step.operation else {
                panic!("MSTORE cannot acquire a folded result")
            };
            assert_eq!(operation.opcode, 0x52);
            assert_eq!(
                operation.operands,
                [
                    SsaOperand::Constant("0x100000".into()),
                    SsaOperand::Constant("0x2a".into()),
                ]
            );
            for (operand, constant) in step.operands.iter().zip(["0x100000", "0x2a"]) {
                assert!(partial.ssa.values[*operand].facts.iter().any(|fact| {
                    fact.status == SsaFactStatus::Current
                        && fact.constant.as_deref() == Some(constant)
                }));
            }
        } else {
            assert_eq!(step.opcode, if index % 3 == 0 { 0x60 } else { 0x62 });
            assert!(step.operands.is_empty());
            let [result] = step.results.as_slice() else {
                panic!("every prefix PUSH must retain its original definition")
            };
            let SsaValueOrigin::Instruction(origin) = &partial.ssa.values[*result].origin else {
                panic!("PUSH result must retain its instruction origin")
            };
            assert_eq!(
                (origin.state, origin.instruction_index, origin.pc),
                (0, index, step.pc)
            );
        }
    }
    let pending = block.instructions.last().unwrap();
    assert_eq!((pending.pc, pending.opcode), (pc, 0x52));
    assert_eq!(block.exit_effect, pending.effect_result.unwrap());
    assert_eq!(partial.ssa.effect_count, effects.len());
    assert_eq!(partial.ssa.value_count, 2 * (completed_writes + 1));
    assert_eq!(partial.ssa.values.len(), partial.ssa.value_count);
    assert!(!partial.ssa.values.iter().any(|value| {
        matches!(&value.origin, SsaValueOrigin::Instruction(origin)
            if origin.state == 0 && (origin.pc >= pc || origin.instruction_index >= prefix.len() - 1))
    }));
    assert_eq!(
        block
            .instructions
            .iter()
            .filter(|step| {
                step.opcode == 0x52 && step.progress == InstructionProgress::Completed
            })
            .count(),
        completed_writes
    );
    assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
    assert!(cfg.exit_stack.is_empty());
    assert!(partial.edges.is_empty());
    assert!(partial.ssa.transitions.is_empty());
    assert!(partial.ssa.deferred_edges.is_empty());
    assert!(partial.outcomes.iter().all(|outcome| {
        outcome.state == 0 && outcome.kind == evm_abstract_protocol::OutcomeKind::Failure
    }));
    let exit = state
        .exit
        .as_ref()
        .expect("a current prefix must retain its real state");
    assert_eq!(exit.store, state.entry.store);
    assert_eq!(exit.path, state.entry.path);
    let [frame] = exit.frames.as_slice() else {
        panic!("pending MSTORE must retain the root frame")
    };
    let mut unchanged = entry.clone();
    unchanged.memory = frame.memory;
    assert_eq!(frame, &unchanged);
    let initial_memory = &partial.byte_arrays[entry.memory];
    assert!(initial_memory.memory);
    assert!(buffer_matches(&partial, initial_memory, &[]));
    assert!(initial_memory.cells.is_empty());
    assert_eq!(initial_memory.default.constants, Some(vec!["0x0".into()]));
    assert!(byte_contains(&initial_memory.default, 0));
    let limits = ExprLimits {
        max_nodes: work_limits.max_expression_nodes.try_into().unwrap(),
        max_depth: work_limits.max_expression_depth.try_into().unwrap(),
    };
    let initial = native_view(&partial.symbolic, initial_memory.symbolic.node, limits).unwrap();
    assert_eq!(initial.offset.as_constant(), Some(U256::ZERO));
    assert_eq!(initial.length.as_constant(), Some(U256::ZERO));
    assert!(matches!(initial.state.kind(), ByteStateKind::Zero));
    let Expression::View(initial_view) = &partial.symbolic.nodes[initial_memory.symbolic.node]
    else {
        panic!("entry memory must retain its original view")
    };
    assert!(matches!(
        partial.symbolic.nodes[initial_view.state.node],
        Expression::BytesZero
    ));
    let memory = &partial.byte_arrays[frame.memory];
    assert!(memory.memory);
    assert_eq!(memory.default, initial_memory.default);
    let captured = native_view(&partial.symbolic, memory.symbolic.node, limits).unwrap();
    assert_eq!(captured.offset.as_constant(), Some(U256::ZERO));
    let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
        panic!("pending MSTORE must retain its captured memory view")
    };
    // Identical writes are idempotent in the physical root. Only the complete
    // current receipt above counts writes and distinguishes the pending effect.
    // Check raw structure too: native decoding can normalize equivalent writes.
    if completed_writes == 0 {
        assert_eq!(memory, initial_memory, "{phase}");
        assert_eq!(view.state, initial_view.state);
        assert!(matches!(captured.state.kind(), ByteStateKind::Zero));
        assert!(buffer_matches(&partial, memory, &[]));
    } else {
        assert_eq!(memory.length.constants, Some(vec!["0x100020".into()]));
        assert_eq!(captured.length.as_constant(), Some(U256::from(0x100020)));
        let Expression::WriteWord(write) = &partial.symbolic.nodes[view.state.node] else {
            panic!("committed prefix must retain its raw WriteWord")
        };
        assert_eq!(write.previous, initial_view.state);
        for (operand, constant) in [(write.offset, "0x100000"), (write.value, "0x2a")] {
            assert_eq!(
                partial.symbolic.nodes[operand.node],
                Expression::Constant(constant.into())
            );
        }
        let ByteStateKind::WriteWord {
            previous,
            offset,
            value,
        } = captured.state.kind()
        else {
            panic!("committed prefix must retain exactly its one effective WriteWord")
        };
        assert_eq!(previous, &initial.state);
        assert_eq!(offset.as_constant(), Some(U256::from(0x100000)));
        assert_eq!(value.as_constant(), Some(U256::from(42)));
        // The sole WriteWord over the unchanged zero root proves every unwritten
        // byte is zero. Extract only its full word, under the reported limits.
        let written = captured
            .slice(
                ExprId::constant(U256::from(0x100000)),
                ExprId::constant(U256::from(32)),
                limits.into(),
            )
            .unwrap();
        assert_eq!(
            written.known_bytes(limits).unwrap().as_deref(),
            Some(word.as_slice())
        );
        assert_eq!(memory.cells.len(), 32);
        for (index, cell) in memory.cells.iter().enumerate() {
            assert_eq!(cell.offset, 0x100000 + index);
            assert!(byte_contains(&memory.values[cell.value], word[index]));
        }
    }
    partial.validate().unwrap();

    // Isolate the mandatory four-node WriteWord from unknown-recipient cleanup:
    // a concrete non-precompile owner leaves RIPEMD definitely untouched, so
    // late_failure does not attempt its unrelated MAY-deletion at block entry.
    // The old smaller memory cap still admits lazy expansion before the refusal.
    {
        let mut bounded = input.clone();
        bounded.limits.max_memory_bytes = 1_048_576;
        bounded.limits.max_expression_nodes = 3;
        let owner = "0x1111111111111111111111111111111111111111";
        let AnalysisInput::Bytecode(source) = &mut bounded.input else {
            panic!("expected the original bytecode input")
        };
        source.address = Some(owner.into());
        let limits = bounded.limits.clone();
        let partial = analyze(bounded).unwrap();
        assert_eq!(partial.metadata.limits, limits);
        assert_eq!(
            partial.metadata.environment.to,
            AddressValue::Concrete(owner.into())
        );
        assert_eq!(partial.status, AnalysisStatus::Incomplete);
        assert!(!partial.ssa.complete);
        let [frontier] = partial.frontiers.as_slice() else {
            panic!(
                "expected only the first MSTORE refusal: {:?}",
                partial.frontiers
            )
        };
        assert_eq!(
            frontier.reason,
            evm_abstract_protocol::FrontierDetails::Relations(
                evm_abstract_protocol::QueryBoundary::ExpressionLimit,
            )
        );
        assert_eq!(frontier.kind, FrontierKind::Relations);
        assert_eq!((frontier.from, frontier.pc), (Some(0), Some(6)));
        assert!(frontier.range.is_none());
        assert!(frontier.payload.is_none());
        let [block] = partial.ssa.blocks.as_slice() else {
            panic!("expected only the root SSA block")
        };
        assert_eq!(block.state, 0);
        assert_eq!(block.coverage, BlockCoverage::Current);
        assert!(!block.incoming_complete);
        assert!(block.phis.is_empty());
        assert!(block.effect_inputs.is_empty());
        assert!(block.open_incoming.is_empty());
        let [value, offset, store] = block.instructions.as_slice() else {
            panic!("expected the two PUSHes and pending first MSTORE")
        };
        for (push, pc, opcode) in [(value, 0, 0x60), (offset, 2, 0x62)] {
            assert_eq!((push.pc, push.opcode), (pc, opcode));
            assert_eq!(push.progress, InstructionProgress::Completed);
            assert!(push.operands.is_empty());
            assert_eq!(push.results.len(), 1);
        }
        let mut effect = block.effect;
        for step in &block.instructions {
            assert!(!step.fault);
            assert_eq!(step.effect_input, effect);
            let next = step
                .effect_result
                .expect("each observed step has an effect");
            assert_ne!(next, effect);
            effect = next;
        }
        assert_eq!(block.exit_effect, effect);
        assert_eq!(partial.ssa.effect_count, 4);
        assert_eq!(partial.ssa.value_count, 2);
        assert_eq!(partial.ssa.values.len(), 2);
        assert_eq!((store.pc, store.opcode), (6, 0x52));
        assert_eq!(store.progress, InstructionProgress::OperandsConsumed);
        assert_eq!(store.operands, [offset.results[0], value.results[0]]);
        for (operand, constant) in store.operands.iter().zip(["0x100000", "0x2a"]) {
            assert!(partial.ssa.values[*operand].facts.iter().any(|fact| {
                fact.status == evm_abstract_protocol::SsaFactStatus::Current
                    && fact.constant.as_deref() == Some(constant)
            }));
        }
        assert!(store.results.is_empty());
        assert!(store.effect_result.is_some());
        let SsaOperation::Opcode(operation) = &store.operation else {
            panic!("pending MSTORE cannot acquire a folded result")
        };
        assert_eq!(operation.opcode, 0x52);
        assert_eq!(
            operation.operands,
            [
                SsaOperand::Constant("0x100000".into()),
                SsaOperand::Constant("0x2a".into()),
            ]
        );
        assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
        let [state] = partial.states.as_slice() else {
            panic!("expected only the root state")
        };
        let [cfg] = partial.cfg.as_slice() else {
            panic!("expected only the root CFG block")
        };
        assert_eq!((state.state, cfg.id), (0, 0));
        assert_eq!(cfg.executed_pcs, [0, 2, 6]);
        assert!(cfg.entry_stack.is_empty());
        assert!(cfg.exit_stack.is_empty());
        assert!(partial.edges.is_empty());
        assert!(partial.ssa.transitions.is_empty());
        assert!(partial.ssa.deferred_edges.is_empty());
        let exit = state.exit.as_ref().unwrap();
        assert_eq!(exit.store, state.entry.store);
        assert_eq!(exit.path, state.entry.path);
        let [entry] = state.entry.frames.as_slice() else {
            panic!("expected the original root frame")
        };
        assert_eq!(entry.address_value, partial.metadata.environment.to);
        let [frame] = exit.frames.as_slice() else {
            panic!("pending MSTORE must retain the root frame")
        };
        assert!(frame.stack.is_empty());
        let mut unchanged = entry.clone();
        unchanged.memory = frame.memory;
        assert_eq!(frame, &unchanged);
        let initial_memory = &partial.byte_arrays[entry.memory];
        assert!(buffer_matches(&partial, initial_memory, &[]));
        let memory = &partial.byte_arrays[frame.memory];
        assert!(memory.memory);
        assert_eq!(memory.length.constants, Some(vec!["0x100020".into()]));
        assert!(memory.cells.is_empty());
        assert_eq!(memory.default, initial_memory.default);
        assert_eq!(memory.default.constants, Some(vec!["0x0".into()]));
        let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
            panic!("refused write retains memory view")
        };
        assert!(matches!(
            partial.symbolic.nodes[view.state.node],
            Expression::BytesZero
        ));
        let captured = native_view(
            &partial.symbolic,
            memory.symbolic.node,
            ExprLimits {
                max_nodes: limits.max_expression_nodes.try_into().unwrap(),
                max_depth: limits.max_expression_depth.try_into().unwrap(),
            },
        )
        .unwrap();
        assert_eq!(captured.offset.as_constant(), Some(U256::ZERO));
        assert_eq!(captured.length.as_constant(), Some(U256::from(0x100020)));
        assert!(matches!(captured.state.kind(), ByteStateKind::Zero));
        assert!(
            partial
                .symbolic
                .nodes
                .iter()
                .all(|node| !matches!(node, Expression::WriteWord(_)))
        );
        assert!(
            partial
                .outcomes
                .iter()
                .all(|outcome| { outcome.kind == evm_abstract_protocol::OutcomeKind::Failure })
        );
        partial.validate().unwrap();
    }

    // Keep the original symbolic recipient as an independent entry-boundary
    // control. Its optional RIPEMD touch requires a five-node rollback MAY root,
    // so this three-node policy refuses before any submitted instruction runs.
    {
        let mut bounded = input.clone();
        bounded.limits.max_memory_bytes = 1_048_576;
        bounded.limits.max_expression_nodes = 3;
        let AnalysisInput::Bytecode(source) = &bounded.input else {
            panic!("expected the original symbolic bytecode input")
        };
        assert!(source.address.is_none());
        assert_eq!(source.bytecode, code);
        let limits = bounded.limits.clone();
        let partial = analyze(bounded).unwrap();
        assert_eq!(partial.metadata.limits, limits);
        assert_eq!(
            partial.metadata.environment.to,
            AddressValue::Symbolic(InputSymbol {
                kind: InputSymbolKind::To,
                index: None,
            })
        );
        assert_eq!(partial.status, AnalysisStatus::Incomplete);
        assert!(!partial.ssa.complete);
        let [frontier] = partial.frontiers.as_slice() else {
            panic!(
                "expected only the symbolic entry refusal: {:?}",
                partial.frontiers
            )
        };
        assert_eq!(
            frontier.reason,
            FrontierDetails::Relations(evm_abstract_protocol::QueryBoundary::ExpressionLimit)
        );
        assert_eq!(frontier.kind, FrontierKind::Relations);
        assert_eq!((frontier.from, frontier.pc), (Some(0), Some(0)));
        assert!(frontier.range.is_none());
        assert!(frontier.payload.is_none());
        let [state] = partial.states.as_slice() else {
            panic!("expected only the symbolic root state")
        };
        let [cfg] = partial.cfg.as_slice() else {
            panic!("expected only the symbolic root CFG block")
        };
        let [block] = partial.ssa.blocks.as_slice() else {
            panic!("expected only the symbolic root SSA block")
        };
        assert_eq!((state.state, cfg.id, block.state), (0, 0, 0));
        assert!(cfg.executed_pcs.is_empty());
        assert!(cfg.entry_stack.is_empty());
        assert!(cfg.exit_stack.is_empty());
        assert_eq!(block.coverage, BlockCoverage::Current);
        assert!(!block.incoming_complete);
        assert!(block.instructions.is_empty());
        assert!(block.phis.is_empty());
        assert!(block.effect_inputs.is_empty());
        assert!(block.open_incoming.is_empty());
        assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
        assert_eq!(block.exit_effect, block.effect);
        assert_eq!(partial.ssa.value_count, 0);
        assert!(partial.ssa.values.is_empty());
        assert!(partial.edges.is_empty());
        assert!(partial.ssa.transitions.is_empty());
        assert!(partial.ssa.deferred_edges.is_empty());
        assert_eq!(state.exit.as_ref(), Some(&state.entry));
        assert_eq!(partial.stores[state.entry.store].touched_default, None);
        let [frame] = state.entry.frames.as_slice() else {
            panic!("expected the original symbolic root frame")
        };
        assert!(frame.stack.is_empty());
        assert_eq!(frame.address_value, partial.metadata.environment.to);
        assert_eq!(
            partial.stores[frame.rollback_store].touched_default,
            Some(false)
        );
        let memory = &partial.byte_arrays[frame.memory];
        assert!(memory.memory);
        assert!(buffer_matches(&partial, memory, &[]));
        assert!(memory.cells.is_empty());
        assert_eq!(memory.default.constants, Some(vec!["0x0".into()]));
        let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
            panic!("symbolic entry retains its initial memory view")
        };
        assert!(matches!(
            partial.symbolic.nodes[view.state.node],
            Expression::BytesZero
        ));
        let root = partial.metadata.root_program.unwrap();
        assert_eq!(frame.program, Some(root));
        assert_eq!(cfg.program, Some(root));
        let program = &partial.programs[root];
        assert_eq!(program.bytecode, format!("0x{code}"));
        assert_eq!(program.code_address, frame.code_address);
        assert_eq!(program.code_hash, frame.code_hash);
        assert_eq!(partial.disassembly, program.blocks);
        let [source] = program.blocks.as_slice() else {
            panic!("expected the complete original source block")
        };
        assert_eq!((source.id, source.start_pc), (0, 0));
        assert_eq!(cfg.instructions, source.instructions);
        assert_eq!(source.instructions.len(), 20 * 3 + 3);
        assert!(partial.outcomes.iter().all(|outcome| {
            outcome.state == 0 && outcome.kind == evm_abstract_protocol::OutcomeKind::Failure
        }));
        partial.validate().unwrap();
    }
}

#[test]
fn nonpositive_execution_and_precision_budgets_still_fail_native_admission() {
    use evm_abstract_protocol::AnalysisLimits;
    for limits in [
        AnalysisLimits {
            max_states: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_transfers: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_work: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_call_depth: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_memory_bytes: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_constants: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            reduction_rounds: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_facts: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_constraints: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_expression_nodes: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            max_expression_depth: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            relations_enabled: false,
            max_constraints: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            relations_enabled: false,
            max_expression_nodes: 0,
            ..AnalysisLimits::default()
        },
        AnalysisLimits {
            relations_enabled: false,
            max_expression_depth: 0,
            ..AnalysisLimits::default()
        },
    ] {
        let invalid_relations = limits.max_constraints == 0
            || limits.max_expression_nodes == 0
            || limits.max_expression_depth == 0;
        let mut input = request("00");
        input.limits = limits;
        let error = analyze(input).unwrap_err();
        assert_eq!(error.code, ApiErrorCode::InvalidLimits);
        if invalid_relations {
            assert_eq!(
                error.details,
                evm_abstract_protocol::ErrorDetails::Configuration(
                    evm_abstract_protocol::ConfigurationFailure::Relations
                )
            );
        }
    }
    // Context-insensitivity is a valid precision choice with a distinct zero meaning.
    assert!(analyze(request("00")).unwrap().ssa.complete);
}

#[test]
fn bytecode_requests_still_require_positive_rpc_settings() {
    for field in 0..4 {
        let mut input = request("00");
        match field {
            0 => input.limits.rpc_max_accounts = 0,
            1 => input.limits.rpc_max_requests = 0,
            2 => input.limits.rpc_max_response_bytes = 0,
            _ => input.limits.rpc_timeout_ms = 0,
        }
        assert_eq!(
            analyze(input).unwrap_err().code,
            ApiErrorCode::InvalidLimits
        );
    }
}
