//! Entry-word projection preserves executable effects and immutable provenance.
use evm_bytecode::Program;
use evm_decompile::{
    Artifact, Block, Cancellation, Completion, Config, DiagnosticCode, EntityRef, ExecutionError,
    ExecutionLimits, ExecutionOutcome, ExitKind, ExternalResponse, FactScope, Fork, Input,
    MemoryHost, Phase, Stage, StageKind, Strategy, Terminator, U256, ValidationStatus, ValueKind,
    decompile, decompile_with_progress, execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

#[path = "dead_parameters/support.rs"]
mod support;
use support::{Asm, calldata, differential, exit, proxy_host, upgrade_and_call, word};

const USDC: &str =
    include_str!("../../../examples/known-recursion/captured/usdc-26180958-observed-world.json");

fn proxy() -> Input {
    let capture: serde_json::Value = serde_json::from_str(USDC).unwrap();
    let program = Program::from_hex_with_fork(
        capture["accounts"][1]["code"].as_str().unwrap(),
        Fork::Osaka,
    )
    .unwrap();
    Input::new(program.bytes().to_vec(), Fork::Osaka)
}

fn baseline(artifact: &Artifact) -> &Stage {
    artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Baseline)
        .expect("recovered baseline")
}

fn at_pc(stage: &Stage, pc: usize) -> Vec<&Block> {
    let blocks: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .filter(|block| block.source.is_some_and(|source| source.pc == pc))
        .collect();
    assert!(!blocks.is_empty(), "missing source PC 0x{pc:x}");
    blocks
}

fn validated(artifact: &Artifact) {
    for stage in &artifact.stages {
        assert_eq!(
            stage.validation.status,
            ValidationStatus::Passed,
            "{:?}: {:?}",
            stage.kind,
            stage.validation
        );
    }
}

fn complete(artifact: &Artifact) {
    validated(artifact);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Structured);
}

fn assert_projection(artifact: &Artifact, pc: usize, kept: &[usize]) {
    let baseline = baseline(artifact);
    let current = artifact.current().unwrap();
    for original in at_pc(baseline, pc) {
        let projected = current.ir.block(original.id).unwrap();
        let expected: Vec<_> = kept
            .iter()
            .map(|index| original.parameters[*index])
            .collect();
        assert_eq!(projected.parameters, expected, "PC 0x{pc:x}");
        assert_eq!(projected.memory_parameter, original.memory_parameter);
        assert_eq!(projected.effect_parameter, original.effect_parameter);
        for (index, parameter) in projected.parameters.iter().enumerate() {
            let value = current.ir.value(*parameter).unwrap();
            assert_eq!(
                value.kind,
                ValueKind::Parameter {
                    block: original.id,
                    index,
                }
            );
            assert_eq!(value.fact, baseline.ir.value(*parameter).unwrap().fact);
        }
        for incoming in baseline
            .ir
            .edges
            .iter()
            .filter(|edge| edge.to == original.id)
        {
            let edge = current.ir.edge(incoming.id).unwrap();
            let expected: Vec<_> = kept
                .iter()
                .map(|index| incoming.arguments[*index])
                .collect();
            assert_eq!(edge.arguments, expected, "PC 0x{pc:x}");
            assert_eq!(edge.memory, incoming.memory);
            assert_eq!(edge.effect, incoming.effect);
        }
    }
}

#[test]
fn captured_terminal_revert_projects_dead_words_and_retains_baseline_history() {
    let input = proxy();
    let cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Simplify {
            cancel.cancel();
        }
    });
    assert_eq!(before.completion, Completion::Cancelled);
    assert_eq!(before.current().unwrap().kind, StageKind::Baseline);
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Complete);
    let baseline = baseline(&artifact);
    assert_eq!(baseline.ir, before.current().unwrap().ir);
    let current = artifact.current().unwrap();
    assert_eq!(current.kind, StageKind::Structured);
    let observed: Vec<_> = at_pc(baseline, 0x6a3)
        .into_iter()
        .filter(|block| block.parameters.len() == 10)
        .collect();
    assert!(!observed.is_empty(), "missing ten-word revert context");
    for original in observed {
        let incoming: Vec<_> = baseline
            .ir
            .edges
            .iter()
            .filter(|edge| edge.to == original.id)
            .collect();
        assert!(!incoming.is_empty());
        assert!(incoming.iter().all(|edge| edge.arguments.len() == 10));
        let optimized = current.ir.block(original.id).unwrap();
        assert!(
            optimized.parameters.is_empty(),
            "terminal revert at PC 0x6a3 retains unused entry words: {:?}",
            optimized.parameters
        );
        assert_eq!(optimized.memory_parameter, original.memory_parameter);
        assert_eq!(optimized.effect_parameter, original.effect_parameter);
        assert_eq!(optimized.operations, original.operations);
        assert_eq!(optimized.terminator, original.terminator);
        assert!(matches!(
            current.ir.value(optimized.memory_parameter).unwrap().kind,
            ValueKind::MemoryParameter { block } if block == original.id
        ));
        assert!(matches!(
            current.ir.value(optimized.effect_parameter).unwrap().kind,
            ValueKind::EffectParameter { block } if block == original.id
        ));
        let event = artifact
            .events
            .iter()
            .find(|event| {
                original
                    .parameters
                    .iter()
                    .all(|parameter| event.deleted.contains(parameter))
            })
            .expect("explicit parameter deletion event");
        let from = event.from_stage.unwrap();
        assert_eq!(event.rule, "dead_block_parameters");
        let diff = artifact.diff(from, event.to_stage).unwrap();
        for parameter in &original.parameters {
            assert!(current.ir.value(*parameter).is_none());
            assert!(diff.removed.contains(parameter));
            let historical = EntityRef {
                stage: baseline.id,
                entity: *parameter,
            };
            assert!(artifact.origin(historical).is_some());
            assert!(baseline.ir.value(*parameter).is_some());
            assert!(event.inputs.contains(&EntityRef {
                stage: from,
                entity: *parameter,
            }));
        }
        for edge in incoming {
            assert!(current.ir.edge(edge.id).unwrap().arguments.is_empty());
            assert!(diff.changed.contains(&edge.id));
            assert_eq!(baseline.ir.edge(edge.id).unwrap().arguments.len(), 10);
            let origin = artifact
                .origin(EntityRef {
                    stage: event.to_stage,
                    entity: edge.id,
                })
                .unwrap();
            assert!(origin.parents.contains(&EntityRef {
                stage: from,
                entity: edge.id,
            }));
        }
    }
    for terminal in at_pc(current, 0x6a3) {
        assert!(terminal.parameters.is_empty());
        assert!(
            current
                .ir
                .edges
                .iter()
                .filter(|edge| edge.to == terminal.id)
                .all(|edge| edge.arguments.is_empty())
        );
    }
}

#[test]
fn projected_proxy_success_and_failure_preserve_calls_storage_logs_and_return_data() {
    let input = proxy();
    let limits = ExecutionLimits::default();
    let implementation = U256::from(0x1234);
    let payload = [0xde, 0xad, 0xbe, 0xef, 0x42];
    let child_slot = U256::from(42);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        complete(&artifact);
        for success in [false, true] {
            let mut host = proxy_host(&input, upgrade_and_call(implementation, &payload), true);
            let address = host.context[&0x30];
            host.context.insert(0x34, U256::from(7));
            host.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                return_data: vec![0x99, 0x88],
                storage_writes: BTreeMap::from([(child_slot, U256::from(77))]),
            });
            let (raw, observed) = differential(&input, &artifact, host, &limits);
            exit(
                &raw,
                if success {
                    ExitKind::Stop
                } else {
                    ExitKind::Revert
                },
            );
            let call = raw.trace.iter().find(|step| step.opcode == 0xf1).unwrap();
            assert_eq!(call.inputs[1], address);
            assert_eq!(call.inputs[2], U256::from(7));
            let offset = usize::try_from(call.inputs[3]).unwrap();
            assert_eq!(&observed.memory[offset..offset + payload.len()], &payload);
            assert_eq!(
                observed.storage[&word(support::IMPLEMENTATION_SLOT)],
                implementation
            );
            assert!(raw.trace.iter().any(|step| step.opcode == 0x55));
            assert!(raw.trace.iter().any(|step| step.effects.log));
            assert_eq!(observed.logs.len(), 1);
            assert!(observed.responses.is_empty());
            // MemoryHost exposes primitive effects before enclosing-frame rollback.
            assert_eq!(
                observed.storage.get(&child_slot).copied(),
                success.then_some(U256::from(77))
            );

            let mut host = proxy_host(&input, vec![0x12, 0x34, 0x56, 0x78, 9], false);
            let returned = vec![0xc0, 0xff, 0xee];
            host.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                return_data: returned.clone(),
                storage_writes: BTreeMap::from([(child_slot, U256::from(88))]),
            });
            let (raw, observed) = differential(&input, &artifact, host, &limits);
            exit(
                &raw,
                if success {
                    ExitKind::Return
                } else {
                    ExitKind::Revert
                },
            );
            assert!(raw.trace.iter().any(|step| step.opcode == 0xf4));
            assert!(raw.trace.iter().any(|step| step.opcode == 0x3e));
            assert_eq!(&observed.memory[..returned.len()], &returned);
            assert_eq!(observed.storage.contains_key(&child_slot), success);
            assert!(observed.responses.is_empty());
        }
        let mut host = proxy_host(&input, upgrade_and_call(implementation, &payload), true);
        host.context.insert(0x3b, U256::ZERO);
        let storage = host.storage.clone();
        let (raw, observed) = differential(&input, &artifact, host, &limits);
        exit(&raw, ExitKind::Revert);
        assert_eq!(observed.storage, storage);
        assert!(observed.logs.is_empty());
        assert!(raw.trace.iter().all(|step| step.opcode != 0xf1));
    }
}

fn mixed_join() -> (Input, usize) {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("right")
        .op(0x57)
        .push(201)
        .push(21)
        .push(202)
        .push(41)
        .target("join")
        .op(0x56)
        .dest("right")
        .push(203)
        .push(22)
        .push(204)
        .push(42)
        .dest("join")
        // The middle and top values are observed; the other slots only travel.
        .op(0x82)
        .op(0x5f)
        .op(0x55)
        .op(0x80)
        .push(1)
        .op(0x55)
        .op(0x00);
    let pc = a.labels["join"];
    (a.finish(), pc)
}

#[test]
fn every_predecessor_projects_the_same_mixed_slots_with_dense_survivor_indices() {
    let (input, pc) = mixed_join();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        complete(&artifact);
        for join in at_pc(baseline(&artifact), pc) {
            assert_eq!(join.parameters.len(), 4);
            assert_eq!(
                baseline(&artifact)
                    .ir
                    .edges
                    .iter()
                    .filter(|edge| edge.to == join.id)
                    .count(),
                2
            );
        }
        assert_projection(&artifact, pc, &[1, 3]);
        for selector in [0, 1] {
            let (_, observed) = differential(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(selector)),
                &ExecutionLimits::default(),
            );
            assert_eq!(observed.storage[&U256::ZERO], U256::from(21 + selector));
            assert_eq!(observed.storage[&U256::from(1)], U256::from(41 + selector));
        }
    }
}

#[test]
fn preserved_pop_and_unused_folded_operation_still_keep_their_entry_operands() {
    let mut a = Asm::default();
    a.push(201)
        .push(5)
        .push(7)
        .target("compute")
        .op(0x56)
        .dest("compute")
        .op(0x50)
        .push(2)
        .op(0x01)
        .op(0x00);
    let pc = a.labels["compute"];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    complete(&artifact);
    assert_projection(&artifact, pc, &[1, 2]);
    for original in at_pc(baseline(&artifact), pc) {
        let projected = artifact.current().unwrap().ir.block(original.id).unwrap();
        assert_eq!(projected.operations.len(), original.operations.len());
        let add = projected
            .operations
            .iter()
            .find(|op| op.opcode == 0x01)
            .unwrap();
        let original_add = original
            .operations
            .iter()
            .find(|op| op.opcode == 0x01)
            .unwrap();
        assert_eq!(add.inputs, original_add.inputs);
        assert_eq!(add.outputs, original_add.outputs);
        assert_eq!(add.replacement, Some(U256::from(7)));
        let pop = projected
            .operations
            .iter()
            .find(|op| op.opcode == 0x50)
            .unwrap();
        assert_eq!(pop.inputs, vec![original.parameters[2]]);
    }
    let (raw, _) = differential(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        &ExecutionLimits::default(),
    );
    exit(&raw, ExitKind::Stop);
}

#[test]
fn phi_only_dead_cycles_disappear_without_removing_the_loop_body() {
    let mut a = Asm::default();
    a.push(201)
        .target("head")
        .op(0x56)
        .dest("head")
        .op(0x5f)
        .op(0x35)
        .target("exit")
        .op(0x57)
        .dest("latch")
        .target("head")
        .op(0x56)
        .dest("exit")
        .op(0x00);
    let pcs = [a.labels["head"], a.labels["latch"], a.labels["exit"]];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    complete(&artifact);
    for pc in pcs {
        for block in at_pc(baseline(&artifact), pc) {
            assert_eq!(block.parameters.len(), 1);
            let current = artifact.current().unwrap().ir.block(block.id).unwrap();
            assert_eq!(current.operations, block.operations);
        }
        assert_projection(&artifact, pc, &[]);
    }
    let base = baseline(&artifact);
    let head = at_pc(base, pcs[0])[0];
    let latch = at_pc(base, pcs[1])[0];
    assert!(base.ir.edges.iter().any(|edge| {
        edge.from == latch.id && edge.to == head.id && edge.arguments == latch.parameters
    }));
    let limits = ExecutionLimits {
        max_steps: 128,
        ..ExecutionLimits::default()
    };
    for selector in [0, 1] {
        let (raw, _) = differential(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(selector)),
            &limits,
        );
        if selector == 0 {
            assert_eq!(raw.outcome, ExecutionOutcome::StepLimit);
        } else {
            exit(&raw, ExitKind::Stop);
        }
    }
}

#[test]
fn live_loop_carried_words_and_control_dependencies_survive_projection() {
    let mut a = Asm::default();
    a.push(201)
        .op(0x5f)
        .target("head")
        .op(0x56)
        .dest("head")
        .op(0x80)
        .push(3)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .dest("exit")
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .target("head")
        .op(0x56);
    let pcs = [a.labels["head"], a.labels["body"], a.labels["exit"]];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    complete(&artifact);
    for pc in pcs {
        assert!(
            at_pc(baseline(&artifact), pc)
                .iter()
                .all(|block| block.parameters.len() == 2)
        );
        assert_projection(&artifact, pc, &[1]);
    }
    let (raw, observed) = differential(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        &ExecutionLimits::default(),
    );
    exit(&raw, ExitKind::Stop);
    assert_eq!(observed.storage[&U256::ZERO], U256::from(3));
}

#[test]
fn unresolved_boundary_retains_all_entry_words_and_their_backward_dependencies() {
    let mut a = Asm::default();
    a.push(201)
        .push(202)
        .target("bridge")
        .op(0x56)
        .dest("bridge")
        .target("unknown")
        .op(0x56)
        .dest("unknown")
        .op(0x5f)
        .op(0x35)
        .op(0x56);
    let bridge_pc = a.labels["bridge"];
    let unknown_pc = a.labels["unknown"];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Partial);
    assert_eq!(
        artifact.current().unwrap().ir.fact_scope,
        FactScope::RecoveredPaths
    );
    assert!(
        artifact
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::UnknownJump)
    );
    for pc in [bridge_pc, unknown_pc] {
        assert!(
            at_pc(baseline(&artifact), pc)
                .iter()
                .all(|block| block.parameters.len() == 2)
        );
        assert_projection(&artifact, pc, &[0, 1]);
    }
    for block in at_pc(artifact.current().unwrap(), unknown_pc) {
        assert!(matches!(
            block.terminator,
            Terminator::Jump {
                unresolved: true,
                ..
            }
        ));
    }
}

#[test]
fn invalid_jump_opcode_and_underflow_keep_their_exception_after_projection() {
    for fault in [vec![0x60, 255, 0x56], vec![0xfe], vec![0x90]] {
        let mut a = Asm::default();
        a.push(201).target("fault").op(0x56).dest("fault");
        for opcode in fault {
            a.op(opcode);
        }
        let pc = a.labels["fault"];
        let input = a.finish();
        let artifact = decompile(&input, &Config::default());
        complete(&artifact);
        assert_projection(&artifact, pc, &[]);
        for block in at_pc(baseline(&artifact), pc) {
            let optimized = artifact.current().unwrap().ir.block(block.id).unwrap();
            assert_eq!(optimized.operations, block.operations);
            assert_eq!(optimized.terminator, block.terminator);
        }
        let (raw, _) = differential(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            &ExecutionLimits::default(),
        );
        assert!(matches!(raw.outcome, ExecutionOutcome::Exceptional { .. }));
    }
}

#[test]
fn unused_return_data_copy_result_still_raises_the_same_bounds_error() {
    let mut a = Asm::default();
    a.push(201)
        .target("copy")
        .op(0x56)
        .dest("copy")
        .push(1)
        .op(0x5f)
        .op(0x5f)
        .op(0x3e)
        .op(0x00);
    let pc = a.labels["copy"];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    complete(&artifact);
    assert_projection(&artifact, pc, &[]);
    let limits = ExecutionLimits::default();
    let host = MemoryHost::new(&input, Vec::new());
    let mut raw_host = host.clone();
    assert_eq!(
        execute_bytecode(&input, &mut raw_host, &limits),
        Err(ExecutionError::ReturnDataBounds)
    );
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        assert_eq!(
            execute_ir(stage, &mut graph_host, &limits),
            Err(ExecutionError::ReturnDataBounds)
        );
        assert_eq!(raw_host, graph_host);
        if stage.ir.structured.is_some() {
            let mut syntax_host = host.clone();
            assert_eq!(
                execute_structured(stage, &mut syntax_host, &limits),
                Err(ExecutionError::ReturnDataBounds)
            );
            assert_eq!(raw_host, syntax_host);
        }
    }
}

#[test]
fn physical_stack_overflow_survives_compacting_a_near_full_entry_stack() {
    let mut a = Asm::default();
    for _ in 0..1023 {
        a.op(0x5f);
    }
    a.target("full")
        .op(0x56)
        .dest("full")
        .op(0x5f)
        .op(0x5f)
        .op(0x00);
    let pc = a.labels["full"];
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    complete(&artifact);
    for block in at_pc(baseline(&artifact), pc) {
        assert_eq!(block.parameters.len(), 1023);
        assert!(
            matches!(&block.terminator, Terminator::Trap { reason } if reason == "EVM stack overflow")
        );
        let projected = artifact.current().unwrap().ir.block(block.id).unwrap();
        assert_eq!(projected.operations, block.operations);
        assert_eq!(projected.terminator, block.terminator);
    }
    assert_projection(&artifact, pc, &[]);
    let (raw, _) = differential(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        &ExecutionLimits::default(),
    );
    assert!(
        matches!(raw.outcome, ExecutionOutcome::Exceptional { reason } if reason == "EVM stack overflow")
    );
}

#[test]
fn cancelled_or_refused_projection_keeps_the_previous_valid_stage_and_history() {
    let (input, _) = mixed_join();
    let cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Simplify {
            cancel.cancel();
        }
    });
    assert_eq!(before.completion, Completion::Cancelled);
    assert_eq!(before.current().unwrap().kind, StageKind::Baseline);
    let assert_retained = |artifact: &Artifact| {
        validated(artifact);
        assert_eq!(artifact.current().unwrap().ir, before.current().unwrap().ir);
        assert_eq!(artifact.stages, before.stages);
        assert_eq!(artifact.events, before.events);
        assert_eq!(artifact.mappings, before.mappings);
    };
    for phase in [Phase::Validate, Phase::Provenance] {
        let cancel = Cancellation::new();
        let mut simplifying = false;
        let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
            simplifying |= progress.phase == Phase::Simplify;
            if simplifying && progress.phase == phase {
                cancel.cancel();
            }
        });
        assert_eq!(artifact.completion, Completion::Cancelled);
        assert_retained(&artifact);
    }
    let mut config = Config::default();
    config.limits.max_work = before.statistics.work;
    let limited = decompile(&input, &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert!(limited.statistics.work <= config.limits.max_work);
    assert_retained(&limited);
    let mut config = Config::default();
    config.limits.max_history_entities = before.statistics.history_entities;
    let limited = decompile(&input, &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert!(limited.statistics.history_entities <= config.limits.max_history_entities);
    assert_retained(&limited);
}
