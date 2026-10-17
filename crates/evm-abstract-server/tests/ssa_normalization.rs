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
fn local_constant_proofs_survive_an_unrelated_call_depth_frontier() {
    let mut input = request("60026003015f5f5f5f5f5f355af100");
    input.limits.max_call_depth = 1;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::CallDepth)
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
    // Keep the symbolic recipient and the default expression/work policy:
    // the host materialization cap does not restrict logical MLOAD expansion.
    let complete = analyze(input.clone()).unwrap();
    assert_eq!(complete.status, api::AnalysisStatus::Converged);
    assert!(complete.frontiers.is_empty());
    assert!(complete.ssa.complete);
    assert_eq!(complete.metadata.limits, input.limits);
    assert_eq!(complete.cfg.len(), 1);
    assert_eq!(complete.cfg[0].executed_pcs, [0, 2, 3, 4, 5]);
    assert_eq!(complete.ssa.blocks.len(), 1);
    assert_eq!(complete.ssa.blocks[0].instructions.len(), 5);
    for opcode in [0x60, 0x5f, 0x51, 0x01] {
        assert_eq!(
            instruction(&complete, opcode).progress,
            api::InstructionProgress::Completed
        );
    }
    assert_eq!(
        instruction(&complete, 0x00).progress,
        api::InstructionProgress::Dispatched
    );
    assert_eq!(
        instruction(&complete, 0x51).operation,
        api::SsaOperation::Constant("0x0".into())
    );
    assert_eq!(
        instruction(&complete, 0x01).operation,
        api::SsaOperation::Constant("0x1".into())
    );
    let completed_frame = complete.states[0]
        .exit
        .as_ref()
        .unwrap()
        .frames
        .last()
        .unwrap();
    assert_eq!(completed_frame.stack.len(), 1);
    assert_eq!(completed_frame.stack[0].constants, Some(vec!["0x1".into()]));
    assert_eq!(
        complete.byte_arrays[completed_frame.memory]
            .length
            .constants,
        Some(vec!["0x20".into()])
    );
    assert!(
        complete
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == api::OutcomeKind::Return)
    );
    complete.validate().unwrap();

    // This acyclic prefix has fixed mandatory charges and no optional word
    // planning before MLOAD's visit. Find its minimum admission allowance,
    // bounded by work measured under the unchanged successful policy.
    let (mut minimum, mut maximum) = (1, complete.metadata.work);
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        input.limits.max_work = midpoint;
        let candidate = analyze(input.clone()).unwrap();
        if candidate
            .cfg
            .iter()
            .any(|block| block.executed_pcs.contains(&3))
        {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    assert!(minimum < complete.metadata.work);
    input.limits.max_work = minimum;
    let mut selected_limits = complete.metadata.limits.clone();
    selected_limits.max_work = minimum;
    let report = analyze(input).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
    assert_eq!(report.metadata.limits, selected_limits);
    assert_eq!(report.metadata.limits.max_memory_bytes, 1);
    assert_eq!(report.metadata.limits.max_work, minimum);
    // Visiting MLOAD spends the last unit and consumes offset zero. Its next
    // positive 32 * (capacity + 32) scan is refused before operation_work,
    // word planning or touch_memory, so no allocation or result can appear.
    assert_eq!(report.metadata.work, minimum);
    let [frontier] = report.frontiers.as_slice() else {
        panic!("expected only the mandatory MLOAD scan refusal")
    };
    assert_eq!(frontier.kind, api::FrontierKind::Work);
    assert_eq!(frontier.reason, api::FrontierDetails::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(3)));
    assert!(frontier.range.is_none());
    assert!(frontier.payload.is_none());
    let [state] = report.states.as_slice() else {
        panic!("expected the executed root state")
    };
    assert_eq!(state.state, 0);
    let exit = state.exit.as_ref().unwrap();
    assert_eq!(exit.store, state.entry.store);
    let frame = exit.frames.last().unwrap();
    assert_eq!(frame.stack.len(), 1);
    assert_eq!(frame.stack[0].constants, Some(vec!["0x1".into()]));
    assert_eq!(frame.memory, state.entry.frames.last().unwrap().memory);
    let memory = &report.byte_arrays[frame.memory];
    assert_eq!(memory.length.constants, Some(vec!["0x0".into()]));
    let api::Expression::View(view) = &report.symbolic.nodes[memory.symbolic.node] else {
        panic!("the pending load keeps the initial zero content root")
    };
    assert!(matches!(
        report.symbolic.nodes[view.state.node],
        api::Expression::BytesZero
    ));
    assert_eq!(report.cfg.len(), 1);
    assert_eq!(report.cfg[0].executed_pcs, [0, 2, 3]);
    assert_eq!(report.ssa.value_count, 2);
    let load = instruction(&report, 0x51);
    assert_eq!(load.pc, 3);
    assert_eq!(load.progress, api::InstructionProgress::OperandsConsumed);
    assert!(load.results.is_empty());
    assert_eq!(load.operands.len(), 1);
    let operand = &report.ssa.values[load.operands[0]];
    assert!(matches!(&operand.origin, api::SsaValueOrigin::Instruction(origin) if origin.pc == 2));
    assert!(operand.facts.iter().any(|fact| {
        fact.status == api::SsaFactStatus::Current && fact.constant.as_deref() == Some("0x0")
    }));
    assert!(load.effect_result.is_some());
    assert_ne!(load.effect_result, Some(load.effect_input));
    assert_eq!(
        Some(load.effect_input),
        instruction(&report, 0x5f).effect_result
    );
    assert!(matches!(load.operation, api::SsaOperation::Opcode(_)));
    assert!(
        !report.ssa.values.iter().any(|value| matches!(&value.origin,
        api::SsaValueOrigin::Instruction(origin) if origin.pc == load.pc))
    );
    assert!(
        !report
            .ssa
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| instruction.pc > load.pc)
    );
    assert!(!report.outcomes.is_empty());
    assert!(
        report
            .outcomes
            .iter()
            .all(|outcome| outcome.kind == api::OutcomeKind::Failure)
    );
    report.validate().unwrap();
    let wire = serde_json::to_vec(&report).unwrap();
    assert_eq!(
        serde_json::from_slice::<api::AnalysisReport>(&wire).unwrap(),
        report
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
