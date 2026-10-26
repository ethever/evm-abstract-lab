//! Source-qualified Process control with independent concrete inclusion controls.
#![allow(dead_code)]

#[path = "support/native.rs"]
mod native;
#[path = "support/revm.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;

use evm_abstract::{U256, bytecode::Program};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, ContextInsensitive, EdgeKind, EnginePolicies,
        EvmProcedureSummaryRecord, ExecutionConfig, FeedbackAcceleration, KnownProcessSummaries,
        MachineEdgeKind, MergeResumeStatus, OutcomeKind, ProcessAction, ProcessLocation,
        ProcessOwner, ProcessSource, SemanticPolicies, Status, StructuralPartition,
        control::Control, cooperation::NoExtension,
    },
    domain::NumericValue,
    world::{EvmByteArray, EvmWorld},
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;
use support::{A, Assembly, Fixture, U, address, calldata, entry, insert, new_world, word};

fn run(world: EvmWorld<NumericValue>) -> evm_abstract::analysis::EvmWorldAnalysis<NumericValue> {
    run_input(world, EvmByteArray::empty())
}

fn run_input(
    world: EvmWorld<NumericValue>,
    data: EvmByteArray<NumericValue>,
) -> evm_abstract::analysis::EvmWorldAnalysis<NumericValue> {
    let policies = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            KnownProcessSummaries,
        ),
    );
    AnalysisEngine::new(&native::product(), &policies, &mut NoExtension)
        .run(
            AnalysisInput::new(world, entry(data)).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                max_work: 2_000_000_000,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .unwrap()
}

fn world(code: &Fixture) -> EvmWorld<NumericValue> {
    let mut world = new_world();
    insert(&mut world, A, code, &[(0, 7)]);
    world
}

fn bytes(code: &[u8]) -> Fixture {
    Fixture {
        bytes: code.to_vec(),
        labels: Default::default(),
    }
}

#[test]
fn route_process_probe() {
    let cases = [
        ("stop", bytes(&[opcode::STOP])),
        (
            "stop-unreachable-call",
            bytes(&[opcode::STOP, opcode::CALL]),
        ),
        (
            "constant-jump-skips-call",
            bytes(&[
                opcode::PUSH1,
                4,
                opcode::JUMP,
                opcode::CALL,
                opcode::JUMPDEST,
                opcode::STOP,
            ]),
        ),
    ];
    for (name, code) in cases {
        let world = world(&code);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 60_000);
        let start = std::time::Instant::now();
        let result = run(world);
        let calls: usize = result
            .procedure_summaries()
            .iter()
            .map(|record| record.regional_cfg.calls.len())
            .sum();
        eprintln!(
            "ROUTE_PROCESS_PROBE case={name} status={:?} work={} transfers={} calls={calls} elapsed_us={}",
            result.status(),
            result.work(),
            result.transfers(),
            start.elapsed().as_micros()
        );
        assert_closed(&result);
        assert_eq!(
            calls, 0,
            "unreachable CALL must not enter the semantic relation: {name}"
        );
        cover(&witness, &result);
    }
}

fn assert_closed(result: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>) {
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(result.root_terminal_coverage().is_none());
    for record in result.procedure_summaries() {
        assert!(record.closed && record.source_current);
        assert!(
            record
                .procedures
                .iter()
                .all(|procedure| procedure.covered_blocks.is_empty())
        );
        for node in &record.regional_cfg.nodes {
            let transfer = node
                .transfer
                .as_ref()
                .expect("closed admitted body transferred");
            assert!(transfer.current && transfer.complete && !transfer.visited_block);
            assert!(transfer.process.is_some());
            assert_eq!(
                node.frame.basic_block_index, 0,
                "MAY location is separate from Native PC"
            );
        }
    }
}

fn record(
    result: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
) -> &EvmProcedureSummaryRecord<NumericValue> {
    result
        .procedure_summaries()
        .iter()
        .find(|record| record.source_current)
        .unwrap()
}

fn root_locations(
    result: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
) -> BTreeSet<ProcessLocation> {
    record(result).regional_cfg.nodes.iter().filter_map(|node| {
        let frame = node.frame.process.as_ref()?;
        (frame.owner == ProcessOwner::Named(address(A))
            && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == address(A)))
            .then_some(frame.location)
    }).collect()
}

#[test]
fn false_jumpi_ignores_invalid_destination_and_falls_through() {
    let code = bytes(&[
        opcode::PUSH0,
        opcode::PUSH1,
        0xff,
        opcode::JUMPI,
        opcode::STOP,
        opcode::CALL,
    ]);
    let world = world(&code);
    let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 60_000);
    assert_eq!(witness.kind, OutcomeKind::Return);
    let result = run(world);
    assert_closed(&result);
    cover(&witness, &result);
    assert!(record(&result).regional_cfg.calls.is_empty());
    let kinds = control_kinds(&result);
    assert!(kinds.contains(&MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse)));
    assert!(!kinds.contains(&MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)));
}

#[test]
fn taken_invalid_jump_and_push_data_jumpdest_only_fail() {
    let cases = [
        bytes(&[opcode::PUSH1, 0xff, opcode::JUMP, opcode::CALL]),
        bytes(&[
            opcode::PUSH1,
            opcode::JUMPDEST,
            opcode::POP,
            opcode::PUSH1,
            1,
            opcode::JUMP,
            opcode::CALL,
        ]),
        bytes(&[
            opcode::PUSH1,
            1,
            opcode::PUSH1,
            0xff,
            opcode::JUMPI,
            opcode::CALL,
        ]),
    ];
    for code in cases {
        let world = world(&code);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 60_000);
        assert_eq!(witness.kind, OutcomeKind::Failure);
        let result = run(world);
        assert_closed(&result);
        cover(&witness, &result);
        assert!(
            result
                .outcomes()
                .iter()
                .all(|outcome| outcome.kind == OutcomeKind::Failure)
        );
        assert!(record(&result).regional_cfg.calls.is_empty());
        assert_eq!(
            root_locations(&result),
            BTreeSet::from([ProcessLocation::Block(0)])
        );
    }
}

#[test]
fn unknown_jump_has_one_shared_union_covering_all_legal_targets_and_failure() {
    let mut code = Assembly::default();
    code.input(0).op(opcode::JUMP);
    code.op(opcode::CALL); // Not a legal target.
    code.label("left").op(opcode::STOP);
    code.label("right").store(0, 9).op(opcode::STOP);
    code.op(opcode::PUSH1).op(opcode::JUMPDEST); // Data, never a target.
    let code = code.finish();
    let program = Program::decode_with_fork(&code.bytes, evm_abstract::Fork::Osaka).unwrap();
    let targets = program
        .jumpdest_blocks()
        .values()
        .copied()
        .map(ProcessLocation::Block)
        .collect::<BTreeSet<_>>();
    let world = world(&code);
    let result = run_input(world.clone(), EvmByteArray::unknown());
    assert_closed(&result);
    for destination in [code.labels["left"] as u64, code.labels["right"] as u64, 999] {
        let witness = oracle::execute_gas(&world, &entry(calldata(destination)), 60_000);
        cover(&witness, &result);
    }
    let cfg = &record(&result).regional_cfg;
    assert!(cfg.calls.is_empty());
    let hubs = cfg
        .nodes
        .iter()
        .filter(|node| {
            node.frame.process.as_ref().unwrap().location == ProcessLocation::JumpTargets
        })
        .collect::<Vec<_>>();
    assert_eq!(hubs.len(), 1);
    let destinations = hubs[0]
        .transfer
        .as_ref()
        .unwrap()
        .successors
        .iter()
        .map(|edge| cfg.nodes[edge.to].frame.process.as_ref().unwrap().location)
        .collect::<BTreeSet<_>>();
    assert_eq!(destinations, targets);
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
}

#[test]
fn unknown_jumpi_condition_keeps_taken_and_fallthrough_effects() {
    let mut code = Assembly::default();
    code.input(0).jump("write", true).op(opcode::STOP);
    code.label("write").store(0, 9).op(opcode::STOP);
    let world = world(&code.finish());
    let result = run_input(world.clone(), EvmByteArray::unknown());
    assert_closed(&result);
    for value in [0, 1] {
        cover(
            &oracle::execute_gas(&world, &entry(calldata(value)), 60_000),
            &result,
        );
    }
    let kinds = control_kinds(&result);
    assert!(kinds.contains(&MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)));
    assert!(kinds.contains(&MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse)));
}

#[test]
fn pure_loop_closes_with_entered_failure_and_retained_cycle() {
    let code = bytes(&[opcode::JUMPDEST, opcode::PUSH0, opcode::JUMP]);
    let world = world(&code);
    let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 30_000);
    assert_eq!(witness.kind, OutcomeKind::Failure);
    let result = run(world);
    assert_closed(&result);
    cover(&witness, &result);
    assert!(
        result
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    let cfg = &record(&result).regional_cfg;
    assert!(cfg.nodes.iter().any(|node| {
        node.transfer
            .as_ref()
            .unwrap()
            .process
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .is_some_and(|region| {
                region.through.contains(&ProcessLocation::Block(0))
                    && region.relation.blocks[0]
                        .successors
                        .into_iter()
                        .flatten()
                        .any(|(location, _)| location == ProcessLocation::Block(0))
            })
    }));
}

#[test]
fn reachable_call_has_real_deliveries_at_the_following_block() {
    let mut code = Assembly::default();
    code.call(opcode::CALL, U as u16, 0, (0, 0), (0, 0));
    code.store(0, 9).op(opcode::STOP).op(opcode::CALL);
    let code = code.finish();
    let world = world(&code);
    let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 100_000);
    let result = run(world);
    assert_closed(&result);
    cover(&witness, &result);
    let cfg = &record(&result).regional_cfg;
    let calls = cfg.calls.iter().filter(|call| {
        cfg.nodes[call.from].frame.process.as_ref().is_some_and(|frame|
            frame.owner == ProcessOwner::Named(address(A))
            && frame.location == ProcessLocation::Block(0)
            && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == address(A)))
    }).collect::<Vec<_>>();
    assert!(
        !calls.is_empty(),
        "reachable CALL must participate in the relation"
    );
    assert!(
        calls
            .iter()
            .all(|call| call.action == Some(ProcessAction::Call))
    );
    let deliveries = calls
        .iter()
        .flat_map(|call| &call.deliveries)
        .collect::<Vec<_>>();
    assert!(!deliveries.is_empty());
    assert!(deliveries.iter().all(|delivery| delivery.current));
    assert!(
        deliveries
            .iter()
            .flat_map(|delivery| &delivery.successors)
            .all(
                |edge| cfg.nodes[edge.to].frame.process.as_ref().unwrap().location
                    == ProcessLocation::Block(1)
            )
    );
    assert_eq!(
        root_locations(&result),
        BTreeSet::from([ProcessLocation::Block(0), ProcessLocation::Block(1)])
    );
}

#[test]
fn erased_prefix_stack_underflow_and_overflow_remain_entered_failures() {
    let underflow = bytes(&[opcode::POP, opcode::STOP]);
    let mut overflow = vec![opcode::PUSH0; 1025];
    overflow.push(opcode::STOP);
    for code in [underflow, bytes(&overflow)] {
        let world = world(&code);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 100_000);
        assert_eq!(witness.kind, OutcomeKind::Failure);
        let result = run(world);
        assert_closed(&result);
        cover(&witness, &result);
    }
}

#[test]
fn creation_subscriber_delivers_to_the_following_control_block() {
    let mut code = Assembly::default();
    code.push(0)
        .push(0)
        .push(0)
        .op(opcode::CREATE)
        .op(opcode::STOP)
        .op(opcode::CALL);
    let world = world(&code.finish());
    let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 120_000);
    let result = run(world);
    assert_closed(&result);
    cover(&witness, &result);
    let cfg = &record(&result).regional_cfg;
    let calls = cfg.calls.iter().filter(|call| call.action == Some(ProcessAction::Create)
        && cfg.nodes[call.from].frame.process.as_ref().is_some_and(|frame|
            frame.owner == ProcessOwner::Named(address(A))
            && frame.location == ProcessLocation::Block(0)
            && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == address(A))))
        .collect::<Vec<_>>();
    assert!(!calls.is_empty());
    let deliveries = calls
        .iter()
        .flat_map(|call| &call.deliveries)
        .collect::<Vec<_>>();
    assert!(!deliveries.is_empty());
    assert!(
        deliveries
            .iter()
            .any(|delivery| !delivery.creation.is_empty())
    );
    assert!(
        deliveries
            .iter()
            .flat_map(|delivery| &delivery.successors)
            .all(
                |edge| cfg.nodes[edge.to].frame.process.as_ref().unwrap().location
                    == ProcessLocation::Block(1)
            )
    );
    assert!(!root_locations(&result).contains(&ProcessLocation::Block(2)));
}

#[test]
fn call_roles_preserve_storage_owner_code_source_and_static_bound() {
    for op in [
        opcode::CALL,
        opcode::CALLCODE,
        opcode::DELEGATECALL,
        opcode::STATICCALL,
    ] {
        let mut caller = Assembly::default();
        caller
            .call(op, U as u16, 0, (0, 0), (0, 0))
            .op(opcode::STOP);
        let mut callee = Assembly::default();
        callee.store(0, 11).op(opcode::STOP);
        let mut world = world(&caller.finish());
        insert(&mut world, U, &callee.finish(), &[(0, 5)]);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 120_000);
        let result = run(world);
        assert_closed(&result);
        cover(&witness, &result);
        let expected_owner = if matches!(op, opcode::CALL | opcode::STATICCALL) {
            U
        } else {
            A
        };
        assert!(record(&result).regional_cfg.nodes.iter().any(|node|
            node.frame.process.as_ref().is_some_and(|frame|
                frame.owner == ProcessOwner::Named(address(expected_owner))
                && frame.is_static == (op == opcode::STATICCALL)
                && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == address(U)))));
        if op == opcode::STATICCALL {
            assert!(result.outcomes().iter().all(|outcome| {
                outcome
                    .store
                    .read(address(A), &word(0), native::product().borrowed())
                    .singleton()
                    == Some(U256::from(7))
            }));
        }
    }
}

#[test]
fn nested_callback_continuation_and_outer_rollback_have_concrete_inclusion() {
    for revert in [false, true] {
        let mut caller = Assembly::default();
        caller
            .op(opcode::CALLDATASIZE)
            .op(opcode::ISZERO)
            .jump("outer", true);
        caller.store(0, 8).op(opcode::STOP);
        caller
            .label("outer")
            .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0));
        caller.store(1, 9);
        if revert {
            caller.push(0).push(0).op(opcode::REVERT);
        } else {
            caller.op(opcode::STOP);
        }
        let mut callee = Assembly::default();
        callee
            .call(opcode::CALL, A as u16, 0, (0, 32), (0, 0))
            .op(opcode::STOP);
        let mut world = world(&caller.finish());
        insert(&mut world, U, &callee.finish(), &[]);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 180_000);
        assert!(
            witness
                .steps
                .iter()
                .filter(|step| step.owner == address(A) && step.opcode == opcode::SSTORE)
                .count()
                >= 2
        );
        assert_eq!(
            witness.slot(A, 0),
            if revert { U256::from(7) } else { U256::from(8) }
        );
        let result = run(world);
        assert_closed(&result);
        cover(&witness, &result);
        if revert {
            assert!(
                result
                    .outcomes()
                    .iter()
                    .filter(|outcome| outcome.kind == OutcomeKind::Revert)
                    .all(|outcome| outcome
                        .store
                        .read(address(A), &word(0), native::product().borrowed())
                        .singleton()
                        == Some(U256::from(7)))
            );
        }
    }
}

#[test]
fn empty_truncated_push_and_last_call_reach_end_without_fake_pc() {
    // The shared revm trace oracle requires a visited instruction. Empty code
    // has its separate EVM boundary assertion; keep that oracle guard intact.
    let empty = run(world(&bytes(&[])));
    assert_closed(&empty);
    assert!(record(&empty).regional_cfg.calls.is_empty());
    assert!(
        empty
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    for code in [
        bytes(&[opcode::PUSH2, 0xab]),
        bytes(&[opcode::PUSH1, opcode::CALL]),
    ] {
        let world = world(&code);
        let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 60_000);
        let result = run(world);
        assert_closed(&result);
        cover(&witness, &result);
        assert!(record(&result).regional_cfg.calls.is_empty());
        assert!(
            result
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return)
        );
    }
    let mut code = Assembly::default();
    code.call(opcode::CALL, U as u16, 0, (0, 0), (0, 0));
    let world = world(&code.finish());
    let result = run(world);
    assert_closed(&result);
    assert!(root_locations(&result).contains(&ProcessLocation::End));
}

#[test]
fn unknown_static_callee_delegates_known_control_under_its_own_owner() {
    use evm_abstract::world::EvmAccount;
    let mut caller = Assembly::default();
    caller
        .op(opcode::CALLDATASIZE)
        .op(opcode::ISZERO)
        .jump("outer", true);
    caller.push(0).push(0).op(opcode::RETURN);
    caller
        .label("outer")
        .call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut concrete = world(&caller.finish());
    let mut abstract_world = concrete.clone();
    abstract_world
        .insert(address(U), EvmAccount::unknown())
        .unwrap();
    let mut opaque = Assembly::default();
    opaque
        .call(opcode::DELEGATECALL, A as u16, 0, (0, 32), (0, 0))
        .op(opcode::STOP);
    insert(&mut concrete, U, &opaque.finish(), &[]);
    let witness = oracle::execute_gas(&concrete, &entry(EvmByteArray::empty()), 180_000);
    assert!(witness.steps.iter().any(|step| step.code == address(A)
        && step.owner == address(U)
        && step.is_static
        && step.opcode == opcode::JUMPI));
    let result = run(abstract_world);
    assert_closed(&result);
    cover(&witness, &result);
    let cfg = &record(&result).regional_cfg;
    let delegated = cfg.calls.iter().filter(|call|
        call.action == Some(ProcessAction::DelegateCall)
        && cfg.nodes[call.from].frame.process.as_ref().is_some_and(|frame|
            frame.owner == ProcessOwner::Named(address(U)) && frame.is_static)
        && record(&result).procedures[call.callee].frame.process.as_ref().is_some_and(|frame|
            frame.owner == ProcessOwner::Named(address(U)) && frame.is_static
            && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == address(A))))
        .collect::<Vec<_>>();
    assert!(!delegated.is_empty());
    assert!(
        delegated
            .iter()
            .flat_map(|call| &call.deliveries)
            .any(|delivery| delivery.current)
    );
}

fn control_kinds(
    result: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
) -> BTreeSet<MachineEdgeKind> {
    let mut kinds = BTreeSet::new();
    for node in &record(result).regional_cfg.nodes {
        let transfer = node.transfer.as_ref().unwrap();
        kinds.extend(transfer.successors.iter().map(|edge| edge.kind));
        if let Some(region) = transfer.process.as_ref().unwrap().control.as_ref() {
            assert_eq!(region.entry, node.frame.process.as_ref().unwrap().location);
            assert_eq!(
                region.relation.source,
                node.frame.process.as_ref().unwrap().source
            );
            assert_eq!(
                region.relation.epoch,
                node.frame.process.as_ref().unwrap().epoch
            );
            for location in &region.through {
                if let ProcessLocation::Block(index) = location {
                    assert!(!region.relation.blocks[*index].boundary);
                    kinds.extend(
                        region.relation.blocks[*index]
                            .successors
                            .into_iter()
                            .flatten()
                            .map(|(_, kind)| kind),
                    );
                }
            }
        }
    }
    kinds
}

#[test]
fn pure_region_retains_control_without_a_world_payload_per_block() {
    let mut code = Assembly::default();
    for _ in 0..100 {
        code.op(opcode::JUMPDEST).push(1).op(opcode::POP);
    }
    code.op(opcode::STOP).op(opcode::CALL);
    let world = world(&code.finish());
    let witness = oracle::execute_gas(&world, &entry(EvmByteArray::empty()), 100_000);
    assert!(witness.steps.len() > 100);
    let result = run(world);
    assert_closed(&result);
    cover(&witness, &result);
    let cfg = &record(&result).regional_cfg;
    assert_eq!(cfg.nodes.len(), 2);
    assert!(cfg.calls.is_empty());
    let region = cfg
        .nodes
        .iter()
        .find_map(|node| node.transfer.as_ref()?.process.as_ref()?.control.as_ref())
        .unwrap();
    assert_eq!(region.through.len(), 99);
    assert_eq!(region.boundaries, [ProcessLocation::Block(99)]);
    assert_eq!(region.relation.blocks.len(), 101);
    assert!(
        cfg.nodes
            .iter()
            .flat_map(|node| &node.transfer.as_ref().unwrap().successors)
            .any(|edge| edge.kind == MachineEdgeKind::ProcessControl)
    );
    assert!(
        !region.boundaries.contains(&ProcessLocation::Block(100)),
        "dead CALL never reaches an effect boundary"
    );
}

fn cover(
    witness: &oracle::Observation,
    result: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>,
) {
    witness.covers(result);
    // The concrete inspector chooses the code/owner/static/PC independently.
    // Check MAY control inclusion separately from coherent terminal effects.
    for step in &witness.steps {
        if step.is_init {
            continue;
        }
        let Some(account) = result.world().account(step.code) else {
            continue;
        };
        let evm_abstract::world::Code::Runtime(program) = &account.code else {
            continue;
        };
        let location = if step.pc >= program.byte_len() {
            // revm's padded bytecode emits an implicit STOP beyond the source,
            // including after a right-zero-padded truncated PUSH immediate.
            assert_eq!(step.opcode, opcode::STOP);
            ProcessLocation::End
        } else {
            ProcessLocation::Block(
                program
                    .blocks()
                    .iter()
                    .find(|block| {
                        block
                            .instructions
                            .iter()
                            .any(|instruction| instruction.pc == step.pc)
                    })
                    .unwrap()
                    .id,
            )
        };
        let covered = record(result).regional_cfg.nodes.iter().any(|node| {
            let frame = node.frame.process.as_ref().unwrap();
            let owner = match frame.owner {
                ProcessOwner::Named(owner) => owner == step.owner,
                ProcessOwner::RootNamespace => result.entry().environment.to.as_concrete().is_none(),
                ProcessOwner::Residual => result.world().account(step.owner).is_none() && step.owner != address(3),
            };
            owner && frame.is_static == step.is_static
                && matches!(frame.source, ProcessSource::Runtime { source, .. } if source == step.code)
                && (frame.location == location || node.transfer.as_ref().unwrap().process.as_ref().unwrap()
                    .control.as_ref().is_some_and(|region| region.through.contains(&location)))
        });
        assert!(
            covered,
            "concrete control missing from source-qualified MAY relation: code={} owner={} static={} pc={} location={location:?}",
            step.code, step.owner, step.is_static, step.pc
        );
    }
}
