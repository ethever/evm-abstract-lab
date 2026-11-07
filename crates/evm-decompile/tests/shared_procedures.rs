//! Acceptance for reusable bodies, fresh dynamic bindings and shared state.
#[path = "shared_procedures/base_sources.rs"]
mod base_sources;
#[path = "shared_procedures/boundaries.rs"]
mod boundaries;
#[path = "lifted/oracle/expressions.rs"]
mod expressions;
#[path = "shared_procedures/fixtures.rs"]
mod fixtures;
#[path = "shared_procedures/mutations.rs"]
mod mutations;
#[path = "shared_procedures/oracle.rs"]
mod oracle;
#[path = "shared_procedures/rendering.rs"]
mod rendering;
#[path = "shared_procedures/support.rs"]
mod support;

use evm_decompile::{
    ExecutionError, ExecutionLimits, ExecutionOutcome, ExitKind, MemoryHost, U256,
};

fn checked(input: &evm_decompile::Input) -> evm_decompile::Artifact {
    let artifact = evm_decompile::decompile(input, &evm_decompile::Config::default());
    assert_eq!(
        artifact.completion,
        evm_decompile::Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(
        artifact.current().unwrap().kind,
        evm_decompile::StageKind::Lifted
    );
    for stage in &artifact.stages {
        assert_eq!(
            stage.validation.status,
            evm_decompile::ValidationStatus::Passed
        );
    }
    artifact
}

#[test]
fn sequential_multiblock_fixture_has_live_outputs_and_two_sets_of_effects() {
    let fixture = fixtures::sequential();
    eprintln!(
        "shared sequential runtime: {}",
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode)
    );
    let artifact = checked(&fixture.input);
    let host = fixtures::sequential_host(&fixture.input, U256::from(4), U256::from(9));
    let (outcome, observed, _) =
        support::source_execution(&fixture.input, &artifact, host, &ExecutionLimits::default());
    assert!(matches!(
        outcome.unwrap(),
        ExecutionOutcome::Exit {
            kind: ExitKind::Return,
            ..
        }
    ));
    assert_eq!(observed.state.storage[&U256::ZERO], U256::from(2));
    assert_eq!(observed.state.storage[&U256::from(1)], U256::from(9));
    assert_eq!(
        &observed.state.memory[..128],
        fixtures::words(&[1, 18, 2, 35].map(U256::from))
    );
    assert!(observed.state.responses.is_empty());
    for (opcode, count) in [(0x55, 6), (0xa0, 2), (0xfa, 2), (0x3e, 2)] {
        assert_eq!(
            observed
                .invocations
                .iter()
                .filter(|call| call.opcode == opcode)
                .count(),
            count,
            "dynamic opcode 0x{opcode:02x} count"
        );
    }
    let helper_pc = fixture.labels["helper"];
    assert!(
        artifact
            .current()
            .unwrap()
            .ir
            .blocks
            .iter()
            .filter(|block| { block.source.is_some_and(|source| source.pc == helper_pc) })
            .count()
            >= 2,
        "the same source helper needs independently bound caller contexts"
    );
}

#[test]
fn shared_definition_calls_execute_fresh_live_outputs_and_shared_effects() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "mandatory useful shared definitions"
    );
    assert!(
        high.invocations.len() >= 2,
        "mandatory distinct invocation sites"
    );
    assert!(
        high.invocations.iter().any(|call| call
            .bindings
            .iter()
            .any(|binding| binding.formal != binding.actual)),
        "mandatory alpha-renamed context reuse"
    );
    for (first, second) in [(4, 9), (9, 4), (0, 9), (4, 0)] {
        let report = oracle::certify(
            &fixture.input,
            &artifact,
            fixtures::sequential_host(&fixture.input, U256::from(first), U256::from(second)),
            &ExecutionLimits::default(),
        );
        assert!(
            !report.calls.is_empty(),
            "the actual normalized path for ({first}, {second}) must invoke a definition"
        );
    }
    for memory_limit in [255, 287, 351] {
        let report = oracle::certify(
            &fixture.input,
            &artifact,
            fixtures::sequential_host(&fixture.input, U256::from(4), U256::from(9)),
            &ExecutionLimits {
                max_memory_bytes: memory_limit,
                ..ExecutionLimits::default()
            },
        );
        assert_eq!(report.outcome, Err(ExecutionError::MemoryLimit));
        assert!(
            !report.calls.is_empty(),
            "fault must occur while executing shared code"
        );
    }
}

#[test]
fn shared_loop_definition_rebinds_live_phis_and_returns_each_call_result() {
    let fixture = fixtures::loop_calls();
    let artifact = checked(&fixture.input);
    let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "loop fixture must actually share code"
    );
    let index = high.render_index(&artifact.current().unwrap().ir);
    assert!(
        high.procedures.iter().any(|procedure| boundaries::nodes(
            index.node(procedure.body).unwrap()
        )
        .iter()
        .any(|node| matches!(node.kind, evm_decompile::StructuredKind::While { .. }))),
        "at least one shared definition must contain its internal loop and live phis"
    );
    for arguments in [[3, 4], [4, 3], [0, 5]] {
        let report = oracle::certify(
            &fixture.input,
            &artifact,
            fixtures::loop_host(&fixture.input, arguments),
            &ExecutionLimits::default(),
        );
        assert!(
            report.calls.len() >= 2,
            "both dynamic helper calls must use definitions"
        );
    }
}

#[test]
fn conditional_failure_is_lazy_and_preserves_the_first_call_state() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    for (first, second, expected_calls) in [(0, 9, 0), (4, 0, 1)] {
        let host = fixtures::sequential_host(&fixture.input, U256::from(first), U256::from(second));
        let (outcome, observed, _) =
            support::source_execution(&fixture.input, &artifact, host, &ExecutionLimits::default());
        assert!(matches!(
            outcome.unwrap(),
            ExecutionOutcome::Exit {
                kind: ExitKind::Revert,
                ..
            }
        ));
        assert_eq!(&observed.state.memory[68..82], b"shared failure");
        assert_eq!(
            observed
                .invocations
                .iter()
                .filter(|call| call.opcode == 0xfa)
                .count(),
            expected_calls
        );
        assert_eq!(
            observed.state.storage[&U256::ZERO],
            U256::from(expected_calls + 1)
        );
    }
}

#[test]
fn memory_fault_prefixes_include_the_failing_attempt_and_prior_mutations() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    for memory_limit in [255, 287, 351] {
        let host = fixtures::sequential_host(&fixture.input, U256::from(4), U256::from(9));
        let (outcome, observed, _) = support::source_execution(
            &fixture.input,
            &artifact,
            host,
            &ExecutionLimits {
                max_memory_bytes: memory_limit,
                ..ExecutionLimits::default()
            },
        );
        assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
        assert_eq!(
            observed.invocations.last().unwrap().result,
            Err(ExecutionError::MemoryLimit)
        );
        assert_eq!(observed.state.storage[&U256::ZERO], U256::from(1));
    }
}

#[test]
fn loop_phi_fixture_rebinds_each_iteration_and_each_call() {
    let fixture = fixtures::loop_calls();
    eprintln!(
        "shared loop runtime: {}",
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode)
    );
    let artifact = checked(&fixture.input);
    let host = fixtures::loop_host(&fixture.input, [3, 4]);
    let (outcome, observed, _) =
        support::source_execution(&fixture.input, &artifact, host, &ExecutionLimits::default());
    assert!(matches!(
        outcome.unwrap(),
        ExecutionOutcome::Exit {
            kind: ExitKind::Return,
            ..
        }
    ));
    assert_eq!(
        &observed.state.memory[..64],
        fixtures::words(&[6, 10].map(U256::from))
    );
    assert_eq!(
        observed.state.logs.len(),
        7,
        "all loop iterations retain their effects"
    );
    assert_eq!(
        observed
            .invocations
            .iter()
            .filter(|call| call.opcode == 0xfa)
            .count(),
        7,
        "external call runs in every loop iteration"
    );
}

#[test]
fn repeated_same_region_definitions_propagate_evm_terminals_and_faults() {
    for (terminal, expected_kind) in [
        (0x00, Some(ExitKind::Stop)),
        (0xf3, Some(ExitKind::Return)),
        (0xfd, Some(ExitKind::Revert)),
        (0xfe, None),
    ] {
        let fixture = fixtures::repeated_tail(terminal);
        let artifact = checked(&fixture.input);
        let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
        assert!(
            !high.procedures.is_empty(),
            "terminal 0x{terminal:02x} must share its repeated useful tail"
        );
        assert!(
            high.invocations.len() >= 2,
            "same-source AST occurrences require separate calls"
        );
        for flag in [0, 1] {
            let report = oracle::certify(
                &fixture.input,
                &artifact,
                MemoryHost::new(
                    &fixture.input,
                    fixtures::words(&[flag, 1, 1, 42].map(U256::from)),
                ),
                &ExecutionLimits::default(),
            );
            assert!(
                !report.calls.is_empty(),
                "shared tail must actually execute"
            );
            assert_eq!(report.host.state.storage[&U256::from(5)], U256::from(42));
            assert_eq!(
                report.host.state.logs.len(),
                1,
                "synthetic call adds no EVM call/frame effect"
            );
            match (report.outcome.unwrap(), expected_kind) {
                (ExecutionOutcome::Exit { kind, .. }, Some(expected)) => assert_eq!(kind, expected),
                (ExecutionOutcome::Exceptional { .. }, None) => {}
                actual => panic!("procedure swallowed an EVM terminal: {actual:?}"),
            }
        }
    }
}

#[test]
fn pure_caller_arguments_have_real_visible_bindings_and_execute_distinct_constants() {
    use std::collections::BTreeSet;
    let fixture = fixtures::pure_call_inputs();
    eprintln!(
        "shared pure runtime: {}",
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode)
    );
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "live arithmetic inputs must permit real context sharing"
    );
    assert!(high.invocations.len() >= 2);
    let mut bound: BTreeSet<_> = high
        .blocks
        .iter()
        .flat_map(|block| {
            block.parameters.iter().copied().chain(
                block
                    .statements
                    .iter()
                    .flat_map(|statement| statement.outputs.iter().copied()),
            )
        })
        .collect();
    for call in &high.invocations {
        let procedure = high
            .procedures
            .iter()
            .find(|procedure| procedure.id == call.procedure)
            .unwrap();
        bound.extend(
            procedure
                .interface
                .outputs
                .iter()
                .map(|id| call.actual(*id).unwrap()),
        );
    }
    let index = high.render_index(&stage.ir);
    for call in &high.invocations {
        let procedure = high
            .procedures
            .iter()
            .find(|procedure| procedure.id == call.procedure)
            .unwrap();
        let text = index
            .render_shared_invocation(call, false, |_| true)
            .unwrap();
        for input in &procedure.interface.inputs {
            let actual = call.actual(*input).unwrap();
            let name = format!("v{}", actual.0);
            let printed = text
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .any(|token| token == name);
            if printed
                && !matches!(
                    stage.ir.value(actual).unwrap().kind,
                    evm_decompile::ValueKind::Constant(_)
                )
            {
                assert!(
                    bound.contains(&actual),
                    "new call input v{} has no high parameter, retained primitive capture or call result binding",
                    actual.0
                );
            }
        }
    }
    let report = oracle::certify(
        &fixture.input,
        &artifact,
        MemoryHost::new(&fixture.input, fixtures::words(&[4, 9].map(U256::from))),
        &ExecutionLimits::default(),
    );
    assert!(report.calls.len() >= 2);
    for (slot, expected) in [(0, 18), (1, 11), (2, 12), (3, 18), (4, 19)] {
        assert_eq!(
            report.host.state.storage[&U256::from(slot)],
            U256::from(expected)
        );
    }
}

#[test]
fn independent_normalized_machine_baseline_covers_live_values_effects_and_faults() {
    let fixture = fixtures::sequential();
    let mut artifact = checked(&fixture.input);
    let high = artifact
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    high.procedures.clear();
    high.invocations.clear();
    for (first, second) in [(4, 9), (9, 4), (0, 9), (4, 0)] {
        oracle::certify(
            &fixture.input,
            &artifact,
            fixtures::sequential_host(&fixture.input, U256::from(first), U256::from(second)),
            &ExecutionLimits::default(),
        );
    }
    for memory_limit in [255, 287, 351] {
        oracle::certify(
            &fixture.input,
            &artifact,
            fixtures::sequential_host(&fixture.input, U256::from(4), U256::from(9)),
            &ExecutionLimits {
                max_memory_bytes: memory_limit,
                ..ExecutionLimits::default()
            },
        );
    }
    let fixture = fixtures::loop_calls();
    let mut artifact = checked(&fixture.input);
    let high = artifact
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    high.procedures.clear();
    high.invocations.clear();
    oracle::certify(
        &fixture.input,
        &artifact,
        fixtures::loop_host(&fixture.input, [3, 4]),
        &ExecutionLimits::default(),
    );
}

#[test]
fn shared_return_data_faults_preserve_both_dynamic_call_prefixes() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    for failing_call in [0, 1] {
        let mut host = fixtures::sequential_host(&fixture.input, U256::from(4), U256::from(9));
        host.responses[failing_call].return_data.clear();
        let report = oracle::certify(&fixture.input, &artifact, host, &ExecutionLimits::default());
        assert_eq!(report.outcome, Err(ExecutionError::ReturnDataBounds));
        assert!(
            !report.calls.is_empty(),
            "actual shared execution must reach the returndata fault"
        );
        assert_eq!(report.host.invocations.last().unwrap().opcode, 0x3e);
        assert_eq!(
            report.host.invocations.last().unwrap().result,
            Err(ExecutionError::ReturnDataBounds)
        );
        assert_eq!(
            report.host.state.storage[&U256::ZERO],
            U256::from(failing_call + 1)
        );
    }
}

#[test]
fn canonical_loop_body_rows_preserve_word_memory_and_effect_transfers() {
    use std::collections::BTreeSet;
    let fixture = fixtures::loop_calls();
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    let index = high.render_index(&stage.ir);
    let mut checked_edges = 0;
    let mut logical_transfers = 0;
    for procedure in &high.procedures {
        let source = boundaries::nodes(index.node(procedure.body).unwrap());
        if !source
            .iter()
            .any(|node| matches!(node.kind, evm_decompile::StructuredKind::While { .. }))
        {
            continue;
        }
        let shown = index
            .render_procedure_body(procedure, true, |_| true)
            .unwrap();
        let hidden = index
            .render_procedure_body(procedure, false, |_| true)
            .unwrap();
        let edges: BTreeSet<_> = source
            .iter()
            .filter_map(|node| match node.kind {
                evm_decompile::StructuredKind::Transfer { edge } => Some(edge),
                evm_decompile::StructuredKind::While { exit_edge, .. } => exit_edge,
                _ => None,
            })
            .collect();
        for edge in edges {
            let raw = stage.ir.edge(edge).unwrap();
            let target = stage.ir.block(raw.to).unwrap();
            for text in [
                format!(
                    "m{} = m{}; // memory token on e{}",
                    target.memory_parameter.0, raw.memory.0, edge.0
                ),
                format!(
                    "q{} = q{}; // effect token on e{}",
                    target.effect_parameter.0, raw.effect.0, edge.0
                ),
            ] {
                assert!(
                    shown
                        .iter()
                        .any(|row| row.entity == edge && row.text == text),
                    "missing canonical transfer dependency: {text}"
                );
                assert!(
                    !hidden.iter().any(|row| row.text == text),
                    "state toggle failed for {text}"
                );
            }
            if let Some(text) = index
                .render_transfer(high.transfer(edge).unwrap(), |_| true)
                .unwrap()
            {
                assert!(
                    shown
                        .iter()
                        .any(|row| row.entity == edge && row.text == text)
                );
                assert!(
                    hidden
                        .iter()
                        .any(|row| row.entity == edge && row.text == text)
                );
                logical_transfers += 1;
            }
            checked_edges += 1;
        }
    }
    assert!(
        checked_edges > 0,
        "internal loop definition needs actual state transfers"
    );
    assert!(
        logical_transfers > 0,
        "live loop phis need actual simultaneous word transfers"
    );
}

#[test]
fn closed_shared_calls_preserve_the_following_explicit_partial_frontier() {
    let fixture = fixtures::partial_calls();
    eprintln!(
        "shared partial runtime: {}",
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode)
    );
    let artifact = evm_decompile::decompile(&fixture.input, &evm_decompile::Config::default());
    assert_eq!(artifact.completion, evm_decompile::Completion::Partial);
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, evm_decompile::StageKind::Lifted);
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "closed helper regions must actually share before the partial frontier"
    );
    let host = MemoryHost::new(
        &fixture.input,
        fixtures::words(&[U256::from(4), U256::from(9), U256::MAX]),
    );
    let limits = ExecutionLimits::default();
    let mut graph_host = support::RecordingHost::new(host.clone());
    let graph = evm_decompile::execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut syntax_host = support::RecordingHost::new(host.clone());
    let syntax = evm_decompile::execute_structured(stage, &mut syntax_host, &limits).unwrap();
    let report = oracle::run(stage, host.clone(), &limits);
    assert!(report.calls.len() >= 2);
    assert_eq!(
        report.outcome,
        Ok(ExecutionOutcome::PartialJump {
            target: Some(U256::MAX)
        })
    );
    assert_eq!(report.outcome, Ok(graph.outcome));
    assert_eq!(report.outcome, Ok(syntax.outcome));
    assert_eq!(report.host, graph_host);
    assert_eq!(report.host, syntax_host);
    let mut raw_host = support::RecordingHost::new(host);
    let raw = evm_decompile::execute_bytecode(&fixture.input, &mut raw_host, &limits).unwrap();
    assert!(
        matches!(raw.outcome, ExecutionOutcome::Exceptional { .. }),
        "direct EVM machine resolves the supplied invalid concrete target"
    );
    assert_eq!(
        report.host, raw_host,
        "partial frontier preserves all prior concrete effects and primitive attempts"
    );
}
