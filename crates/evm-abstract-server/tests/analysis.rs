//! Regression coverage for the native-to-browser semantic boundary.

use evm_abstract_protocol::{
    AnalysisStatus, AnalyzeReply, AnalyzeRequest, ApiErrorCode, BlockCoverage, FrontierKind,
    InstructionProgress,
};
use evm_abstract_server::analyze::analyze;

fn request(bytecode: &str) -> AnalyzeRequest {
    let mut request = AnalyzeRequest {
        bytecode: bytecode.into(),
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
