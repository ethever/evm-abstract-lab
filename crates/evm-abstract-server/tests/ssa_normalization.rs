//! End-to-end checks across execution evidence, normalized SSA and strict wire data.

use evm_abstract_protocol as api;
use evm_abstract_server::analyze::analyze;

fn request(code: &str) -> api::AnalyzeRequest {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
    request.limits.context_depth = 0;
    request
}

fn instruction(report: &api::AnalysisReport, opcode: u8) -> &api::SsaInstruction {
    report
        .ssa
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find(|instruction| instruction.opcode == opcode)
        .unwrap()
}

#[test]
fn folded_result_retains_original_inputs_effects_and_independent_source_dependencies() {
    let report = analyze(request("600260030100")).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert_eq!(report.ssa.blocks[0].instructions.len(), 4);
    assert_eq!(
        report.ssa.value_count, 3,
        "dead definitions are intentionally retained"
    );
    let add = instruction(&report, 0x01);
    assert_eq!(add.operation, api::SsaOperation::Constant("0x5".into()));
    assert_eq!(add.progress, api::InstructionProgress::Completed);
    assert_eq!(add.operands.len(), 2);
    assert!(add.effect_result.is_some());
    let value = &report.ssa.values[add.results[0]];
    assert_eq!(value.dependencies, add.operands);
    assert_eq!(
        value.equivalent, None,
        "2 and 3 are sources, not aliases of 5"
    );
    assert!(
        value
            .facts
            .iter()
            .any(|fact| fact.status == api::SsaFactStatus::Current
                && fact.scope == api::SsaFactScope::Definition
                && fact.constant.as_deref() == Some("0x5"))
    );
    for (index, value) in report.ssa.values.iter().enumerate() {
        assert_eq!(value.id, index);
    }
    let encoded = serde_json::to_vec(&report).unwrap();
    let decoded: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, report);
}

#[test]
fn algebraic_copy_keeps_a_definition_and_a_real_equivalence_target() {
    let report = analyze(request("345f0100")).unwrap();
    let input = instruction(&report, 0x34).results[0];
    let add = instruction(&report, 0x01);
    assert_eq!(
        add.operation,
        api::SsaOperation::Copy(api::SsaOperand::Value(input))
    );
    assert_ne!(
        add.results[0], input,
        "copy propagation does not implement dead-definition removal"
    );
    let value = &report.ssa.values[add.results[0]];
    assert_eq!(value.equivalent, Some(input));
    assert_eq!(
        value.transformation,
        api::SsaTransformation::AlgebraicIdentity
    );
    assert_eq!(value.dependencies, add.operands);
}

#[test]
fn local_constant_proofs_survive_an_unrelated_unknown_call_frontier() {
    let report = analyze(request("60026003015f5f5f5f5f5f355af100")).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::UnknownTarget)
    );
    let add = instruction(&report, 0x01);
    assert_eq!(add.operation, api::SsaOperation::Constant("0x5".into()));
    let value = &report.ssa.values[add.results[0]];
    assert!(
        value
            .facts
            .iter()
            .any(|fact| matches!(fact.scope, api::SsaFactScope::State(_)))
    );
    assert!(
        value
            .facts
            .iter()
            .any(|fact| fact.scope == api::SsaFactScope::Definition
                && fact.constant.as_deref() == Some("0x5"))
    );
    let call = instruction(&report, 0xf1);
    assert_eq!(call.progress, api::InstructionProgress::Dispatched);
    assert!(call.results.is_empty());
    assert!(matches!(call.operation, api::SsaOperation::Opcode(_)));
}

#[test]
fn incomplete_memory_step_never_acquires_a_value_or_folded_result() {
    let mut input = request("60015f510100");
    input.limits.max_memory_bytes = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::Memory)
    );
    let load = instruction(&report, 0x51);
    assert_eq!(load.progress, api::InstructionProgress::OperandsConsumed);
    assert!(load.results.is_empty());
    assert!(matches!(load.operation, api::SsaOperation::Opcode(_)));
    assert!(
        !report.ssa.values.iter().any(|value| matches!(&value.origin,
        api::SsaValueOrigin::Instruction(origin) if origin.pc == load.pc))
    );
}

#[test]
fn stale_and_unexecuted_facts_remain_distinct_from_a_collected_unknown() {
    let mut input = request("5f5b600101600156");
    input.limits.max_transfers = 2;
    let report = analyze(input).unwrap();
    let stale = report
        .ssa
        .blocks
        .iter()
        .find(|block| block.coverage == api::BlockCoverage::Stale)
        .unwrap();
    assert!(stale.instructions.is_empty());
    for phi in &stale.phis {
        assert!(
            report.ssa.values[phi.result]
                .facts
                .iter()
                .any(|fact| fact.status == api::SsaFactStatus::Stale
                    && fact.scope == api::SsaFactScope::State(stale.state))
        );
    }
    let mut input = request(include_str!("../../../examples/diamond.hex"));
    input.limits.max_transfers = 1;
    let report = analyze(input).unwrap();
    for block in report
        .ssa
        .blocks
        .iter()
        .filter(|block| block.coverage == api::BlockCoverage::Unexecuted)
    {
        for phi in &block.phis {
            assert!(
                report.ssa.values[phi.result]
                    .facts
                    .iter()
                    .any(|fact| fact.status == api::SsaFactStatus::Unexecuted)
            );
        }
    }
}
