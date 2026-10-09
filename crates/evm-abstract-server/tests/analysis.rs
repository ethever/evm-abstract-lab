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

#[test]
fn diamond_preserves_phi_predecessors_and_structural_json_roundtrip() {
    let report = analyze(request(include_str!("../../../examples/diamond.hex"))).unwrap();
    assert_eq!(report.status, AnalysisStatus::Converged);
    assert!(report.ssa.complete);
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
fn unknown_call_retains_pending_phase_and_no_fabricated_result() {
    let report = analyze(request("5f5f5f5f5f5f355af100")).unwrap();
    assert_eq!(report.status, AnalysisStatus::Incomplete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == FrontierKind::UnknownTarget)
    );
    let call = report
        .ssa
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find(|instruction| instruction.opcode == 0xf1)
        .unwrap();
    // Unknown target still allows observed immediate failure paths; dispatch
    // does not mean all possible callees or a successful result were covered.
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
    assert_eq!(report.states.len(), report.cfg.len());
    let root = &report.states[0].entry.frames[0];
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
    assert!(
        data.cells.iter().any(|cell| cell.offset == 31
            && data.values[cell.value].constants == Some(vec!["0x2a".into()]))
    );
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
        account
            .transient
            .iter()
            .any(|slot| slot.slot == "0x0" && slot.value.constants == Some(vec!["0x7".into()]))
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
fn dense_byte_arrays_share_complete_values_without_losing_cells() {
    let mut input = request("365f5f37365ff3");
    input.environment.calldata =
        evm_abstract_protocol::CalldataInput::Exact(format!("0x{}", "aa".repeat(1024)));
    let report = analyze(input).unwrap();
    let dense: Vec<_> = report
        .byte_arrays
        .iter()
        .filter(|bytes| bytes.cells.len() == 1024)
        .collect();
    assert!(
        dense.len() >= 2,
        "calldata and memory retain separate byte arrays"
    );
    for bytes in dense {
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
    let encoded = serde_json::to_vec(&report).unwrap();
    assert!(
        encoded.len() < 200_000,
        "1 KiB byte fixture expanded to {} bytes",
        encoded.len()
    );
    let decoded: evm_abstract_protocol::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, report);
}
