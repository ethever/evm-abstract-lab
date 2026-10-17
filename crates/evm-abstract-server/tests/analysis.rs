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
fn buffer_matches(
    report: &evm_abstract_protocol::AnalysisReport,
    bytes: &evm_abstract_protocol::ByteArraySnapshot,
    expected: &[u8],
) -> bool {
    use evm_abstract::domain::symbolic::{ByteStateId, ByteView, ExprId, ExprLimits};
    use evm_abstract_protocol::{Expression, SymbolicGraph};
    fn scalar(graph: &SymbolicGraph, node: usize) -> Option<ExprId> {
        let Expression::Constant(value) = graph.nodes.get(node)? else {
            return None;
        };
        Some(ExprId::constant(value.parse().ok()?))
    }
    fn view(graph: &SymbolicGraph, node: usize, limits: ExprLimits) -> Option<ByteView> {
        let Expression::View(view) = graph.nodes.get(node)? else {
            return None;
        };
        Some(ByteView::new(
            state(graph, view.state.node, limits)?,
            scalar(graph, view.offset.node)?,
            scalar(graph, view.length.node)?,
        ))
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
                    view(graph, copy.source.node, limits)?,
                    scalar(graph, copy.length.node)?,
                    limits.into(),
                )
                .ok(),
            Expression::Slice(source) => {
                let source = view(graph, source.node, limits)?;
                source
                    .state
                    .slice(source.offset, source.length, limits.into())
                    .ok()
            }
            _ => None,
        }
    }
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
        && view(&report.symbolic, bytes.symbolic.node, limits)
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
    input.limits.max_work = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, AnalysisStatus::Incomplete);
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
    // Repeated sparse MSTORE at 1 MiB needs a span beyond the old memory ceiling.
    // The configured finite domain makes this account for >100 million units,
    // while only 32 concrete memory bytes need storing.
    let code = format!("{}602062100000f3", "602a6210000052".repeat(20));
    let mut input = request(&code);
    input.limits.max_memory_bytes = 1_048_576 + 32;
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
    // One logical guard costs (14 + 14 + 2) * 16 * (512^2 + 1)
    // = 125,829,600, so 100 million stops the first MSTORE before expansion.
    // The old smaller memory cap still admits lazy expansion; three expression
    // nodes instead reject its four-node WriteWord after that expansion.
    for (max_work, max_memory, max_nodes, reason, memory_length) in [
        (
            100_000_000,
            selected_limits.max_memory_bytes,
            selected_limits.max_expression_nodes,
            evm_abstract_protocol::FrontierDetails::Work,
            "0x0",
        ),
        (
            selected_limits.max_work,
            1_048_576,
            3,
            evm_abstract_protocol::FrontierDetails::Relations(
                evm_abstract_protocol::QueryBoundary::ExpressionLimit,
            ),
            "0x100020",
        ),
    ] {
        let mut bounded = input.clone();
        bounded.limits.max_work = max_work;
        bounded.limits.max_memory_bytes = max_memory;
        bounded.limits.max_expression_nodes = max_nodes;
        let limits = bounded.limits.clone();
        let partial = analyze(bounded).unwrap();
        assert_eq!(partial.metadata.limits, limits);
        assert_eq!(partial.status, AnalysisStatus::Incomplete);
        assert!(!partial.ssa.complete);
        let [frontier] = partial.frontiers.as_slice() else {
            panic!(
                "expected only the first MSTORE refusal: {:?}",
                partial.frontiers
            )
        };
        assert_eq!(frontier.reason, reason);
        assert_eq!(
            frontier.kind,
            if max_work == 100_000_000 {
                FrontierKind::Work
            } else {
                FrontierKind::Relations
            }
        );
        assert_eq!((frontier.from, frontier.pc), (Some(0), Some(6)));
        assert!(frontier.range.is_none());
        let block = &partial.ssa.blocks[0];
        let store = block.instructions.last().unwrap();
        assert_eq!((store.pc, store.opcode), (6, 0x52));
        assert_eq!(store.progress, InstructionProgress::OperandsConsumed);
        assert_eq!(store.operands.len(), 2);
        for (operand, constant) in store.operands.iter().zip(["0x100000", "0x2a"]) {
            assert!(partial.ssa.values[*operand].facts.iter().any(|fact| {
                fact.status == evm_abstract_protocol::SsaFactStatus::Current
                    && fact.constant.as_deref() == Some(constant)
            }));
        }
        assert!(store.results.is_empty());
        assert!(store.effect_result.is_some());
        assert!(matches!(
            store.operation,
            evm_abstract_protocol::SsaOperation::Opcode(_)
        ));
        let state = &partial.states[0];
        assert_eq!(partial.cfg[0].executed_pcs, [0, 2, 6]);
        let exit = state.exit.as_ref().unwrap();
        assert_eq!(exit.store, state.entry.store);
        let frame = exit.frames.last().unwrap();
        assert!(frame.stack.is_empty());
        let memory = &partial.byte_arrays[frame.memory];
        assert_eq!(memory.length.constants, Some(vec![memory_length.into()]));
        let evm_abstract_protocol::Expression::View(view) =
            &partial.symbolic.nodes[memory.symbolic.node]
        else {
            panic!("refused write retains memory view")
        };
        assert!(matches!(
            partial.symbolic.nodes[view.state.node],
            evm_abstract_protocol::Expression::BytesZero
        ));
        assert!(
            partial
                .outcomes
                .iter()
                .all(|outcome| { outcome.kind == evm_abstract_protocol::OutcomeKind::Failure })
        );
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
    ] {
        let mut input = request("00");
        input.limits = limits;
        assert_eq!(
            analyze(input).unwrap_err().code,
            ApiErrorCode::InvalidLimits
        );
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
