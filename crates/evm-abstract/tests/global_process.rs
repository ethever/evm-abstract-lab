//! Native process checkpoint with independent concrete execution and journaling.
#![allow(dead_code)]

#[path = "global_process/control.rs"]
mod control;
#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;

use alloy_primitives::{U256, keccak256};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, EnginePolicies, ExecutionConfig,
        FeedbackAcceleration, FirstResumeStatus, FrameCode, KnownProcessSummaries, OutcomeKind,
        ProcessAction, RecentDistinctJumps, SemanticPolicies, Status, StructuralPartition,
        TransferSemantics, control::Control, cooperation::NoExtension,
    },
    domain::NumericValue,
    world::{EvmAccount, EvmByteArray, EvmWorld, Existence},
};
use revm_bytecode::opcode;
use support::{A, Assembly, U, address, entry, insert, new_world, only, read, word};

fn run(
    world: EvmWorld<NumericValue>,
    max_work: usize,
) -> evm_abstract::analysis::EvmWorldAnalysis<NumericValue> {
    run_input(world, entry(EvmByteArray::empty()), max_work)
}

fn run_input(
    world: EvmWorld<NumericValue>,
    input: evm_abstract::world::EvmEntry<NumericValue>,
    max_work: usize,
) -> evm_abstract::analysis::EvmWorldAnalysis<NumericValue> {
    let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            KnownProcessSummaries,
        ),
    );
    let mut extension = NoExtension;
    AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run(
            AnalysisInput::new(world, input).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                max_work,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .unwrap()
}

fn evidence(label: &str, analysis: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>) {
    let mut frontiers = std::collections::BTreeMap::<String, usize>::new();
    for frontier in analysis.frontiers() {
        *frontiers
            .entry(format!("{:?}", frontier.reason))
            .or_default() += 1;
    }
    eprintln!(
        "GLOBAL_PROCESS_JSON {}",
        serde_json::json!({
            "case": label, "native_schema": evm_abstract::analysis::SCHEMA_VERSION,
            "status": analysis.status(), "work": analysis.work(), "outer_states": analysis.states().len(),
            "transfers": analysis.transfers(), "root_terminal_coverage": false, "frontier_counts": frontiers,
            "records": analysis.procedure_summaries().iter().map(|record| serde_json::json!({
                "kind": record.kind, "source_current": record.source_current, "closed": record.closed,
                "root_procedure": record.root_procedure, "state_count": record.state_count,
                "transfers": record.transfers, "propagations": record.propagations,
                "procedures": record.procedures.iter().map(|p| serde_json::json!({
                    "frame": p.frame, "semantics": p.semantics, "dependencies": p.dependencies,
                    "outputs": p.outputs.iter().map(|o| o.kind).collect::<Vec<_>>(), "covered_blocks": p.covered_blocks
                })).collect::<Vec<_>>(),
                "regional_cfg": record.regional_cfg,
            })).collect::<Vec<_>>(),
        })
    );
}

fn fixture(bytes: &[u8]) -> support::Fixture {
    support::Fixture {
        bytes: bytes.to_vec(),
        labels: Default::default(),
    }
}

#[test]
fn entered_pure_loop_has_native_process_failure_without_root_envelope() {
    let code = fixture(&[opcode::JUMPDEST, opcode::PUSH0, opcode::JUMP]);
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 5)]);
    let concrete = oracle::execute_with_gas(&world, &entry(EvmByteArray::empty()), 60_000);
    assert_eq!(concrete.kind, OutcomeKind::Failure);
    let analysis = run(world, 20_000_000);
    evidence("local-process", &analysis);
    eprintln!(
        "GLOBAL_PROCESS loop status={:?} states={} transfers={} work={} frontiers={:?}",
        analysis.status(),
        analysis.states().len(),
        analysis.transfers(),
        analysis.work(),
        analysis.frontiers()
    );
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    concrete.covers(&analysis);
    let record = analysis.procedure_summaries().last().unwrap();
    assert!(record.closed && record.source_current);
    let procedure = &record.procedures[record.root_procedure.unwrap()];
    assert_eq!(procedure.semantics, TransferSemantics::KnownProcess);
    assert_eq!(procedure.frame.mode, FrameCode::Runtime);
    assert_eq!(procedure.frame.code_hash, keccak256(&code.bytes));
    assert_eq!(procedure.outputs.len(), 1);
    assert_eq!(procedure.outputs[0].kind, OutcomeKind::Failure);
    assert!(procedure.covered_blocks.is_empty());
    let node = &record.regional_cfg.nodes[0];
    let receipt = node.transfer.as_ref().unwrap();
    assert!(receipt.current && receipt.complete);
    assert!(!receipt.visited_block);
    assert_eq!(receipt.outcomes, vec![OutcomeKind::Failure]);
    assert!(receipt.process.as_ref().unwrap().actions.is_empty());
    assert_eq!(receipt.input_revision, node.input_revision);
}

#[test]
fn push_data_is_not_a_primitive_call_capability() {
    let mut bytes = vec![opcode::PUSH32];
    bytes.extend([opcode::CALL; 32]);
    bytes.extend([opcode::POP, opcode::STOP]);
    let mut world = new_world();
    insert(&mut world, A, &fixture(&bytes), &[(0, 5)]);
    let analysis = run(world, 20_000_000);
    evidence("local-process", &analysis);
    assert_eq!(analysis.status(), Status::Converged);
    for record in analysis.procedure_summaries() {
        assert!(record.regional_cfg.calls.is_empty());
        for node in &record.regional_cfg.nodes {
            let transfer = node.transfer.as_ref().unwrap();
            assert!(!transfer.visited_block);
            assert!(transfer.process.as_ref().unwrap().actions.is_empty());
        }
    }
    for outcome in analysis.outcomes() {
        only(&read(&outcome.store, A, 0), &[5]);
    }
}

fn subject(op: u8) -> support::Fixture {
    let mut a = Assembly::default();
    a.input(0).op(opcode::ISZERO).jump("outer", true);
    a.store(0, 1).op(opcode::STOP);
    a.label("outer")
        .call(op, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .push(0)
        .op(opcode::REVERT);
    a.finish()
}

#[test]
fn known_unknown_static_process_actions_close_owner_and_constructor_control() {
    for op in [opcode::CALL, opcode::STATICCALL] {
        let code = subject(op);
        let mut u = Assembly::default();
        u.push(1)
            .mstore(0)
            .call(opcode::CALL, A as u16, 0, (0, 32), (0, 0))
            .op(opcode::POP)
            .op(opcode::STOP);
        let mut world = new_world();
        insert(&mut world, A, &code, &[(0, 0)]);
        world
            .insert(
                address(U),
                EvmAccount {
                    balance: word(1_000_000),
                    nonce: word(0),
                    existence: Existence::Present,
                    storage_unknown: false,
                    ..EvmAccount::unknown()
                },
            )
            .unwrap();
        let concrete = oracle::concrete_unknown(&world, U, &u.finish());
        let witness = oracle::execute_with_gas(&concrete, &entry(EvmByteArray::empty()), 200_000);
        assert_eq!(witness.kind, OutcomeKind::Revert);
        assert!(witness.steps.iter().any(|step| step.code == address(A)
            && step.opcode == opcode::SSTORE
            && step.is_static == (op == opcode::STATICCALL)));
        assert_eq!(witness.slot(A, 0), U256::ZERO);
        let analysis = run(world, 2_000_000_000);
        evidence(
            if op == opcode::CALL {
                "ordinary"
            } else {
                "static"
            },
            &analysis,
        );
        eprintln!(
            "GLOBAL_PROCESS mode={op:x} status={:?} states={} transfers={} work={} records={} frontiers={:?}",
            analysis.status(),
            analysis.states().len(),
            analysis.transfers(),
            analysis.work(),
            analysis.procedure_summaries().len(),
            analysis
                .frontiers()
                .iter()
                .map(|f| &f.reason)
                .collect::<Vec<_>>()
        );
        assert_eq!(analysis.status(), Status::Converged);
        assert!(analysis.frontiers().is_empty());
        witness.covers(&analysis);
        for outcome in analysis
            .outcomes()
            .iter()
            .filter(|o| o.kind != OutcomeKind::Return)
        {
            only(&read(&outcome.store, A, 0), &[0]);
        }
        let mut callback = false;
        let mut static_callback = false;
        let mut delivery = false;
        for record in analysis.procedure_summaries() {
            assert!(record.closed && record.source_current);
            for p in &record.procedures {
                if p.frame.code_address == address(A) {
                    assert_eq!(p.frame.mode, FrameCode::Runtime);
                    assert_eq!(p.frame.code_hash, keccak256(&code.bytes));
                    assert_eq!(p.semantics, TransferSemantics::KnownProcess);
                    assert!(p.covered_blocks.is_empty());
                }
                let process = p
                    .frame
                    .process
                    .as_ref()
                    .expect("installed process owner/source");
                assert_eq!(p.frame.is_static, process.is_static);
                assert!(p.process_input.is_some());
                let encoded = serde_json::to_value(&p.frame).unwrap();
                assert_eq!(encoded.as_object().unwrap().len(), 1);
                assert!(encoded.get("process").is_some());
                assert!(encoded.get("address").is_none());
                assert!(encoded.get("basic_block_index").is_none());
            }
            for call in &record.regional_cfg.calls {
                let action = call.action.expect("process call has explicit action");
                let target = call.target.expect("actual named or residual target");
                assert_eq!(
                    call.scope,
                    Some(evm_abstract::analysis::ProcessControlScope::Primitive)
                );
                let from = &record.regional_cfg.nodes[call.from];
                let callee = &record.procedures[call.callee];
                assert!(!from.transfer.as_ref().unwrap().visited_block);
                assert!(from.transfer.as_ref().unwrap().process.is_some());
                if from.frame.mode == FrameCode::Unknown && callee.frame.code_address == address(A)
                {
                    callback = true;
                    static_callback |= callee.frame.is_static;
                }
                let child_owner = callee.frame.process.as_ref().unwrap().owner;
                if matches!(
                    action,
                    ProcessAction::Call
                        | ProcessAction::StaticCall
                        | ProcessAction::Create
                        | ProcessAction::Create2
                ) {
                    assert_eq!(child_owner, target);
                }
                if matches!(
                    action,
                    ProcessAction::DelegateCall | ProcessAction::CallCode
                ) {
                    assert_eq!(child_owner, from.frame.process.as_ref().unwrap().owner);
                }
                if matches!(action, ProcessAction::Create | ProcessAction::Create2) {
                    assert_eq!(callee.semantics, TransferSemantics::ConstructorProcess);
                    assert_eq!(callee.frame.mode, FrameCode::InitCode);
                    assert!(matches!(
                        callee.frame.process.as_ref().unwrap().source,
                        evm_abstract::analysis::ProcessSource::AnyInitCode { .. }
                    ));
                    assert!(!from.frame.is_static && !callee.frame.is_static);
                } else if from.frame.is_static || action == ProcessAction::StaticCall {
                    assert!(callee.frame.is_static);
                }
                for returned in &call.deliveries {
                    delivery |= !returned.successors.is_empty();
                    if returned.current {
                        assert_eq!(returned.input_revision, call.input_revision);
                    }
                    assert!(
                        returned
                            .successors
                            .iter()
                            .all(|edge| edge.to < record.regional_cfg.nodes.len())
                    );
                }
            }
        }
        assert!(callback && static_callback && delivery);
    }
}

#[test]
fn process_admission_refusal_keeps_work_incomplete() {
    let mut world = new_world();
    insert(
        &mut world,
        A,
        &fixture(&[opcode::JUMPDEST, opcode::PUSH0, opcode::JUMP]),
        &[(0, 5)],
    );
    let analysis = run(world, 1);
    evidence("work-one", &analysis);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(!analysis.frontiers().is_empty());
    assert!(analysis.procedure_summaries().iter().all(|r| !r.closed));
}

#[test]
fn unknown_logical_owner_is_preserved_with_a_covering_named_class() {
    let mut world = new_world();
    insert(
        &mut world,
        A,
        &fixture(&[opcode::JUMPDEST, opcode::PUSH0, opcode::JUMP]),
        &[(0, 5)],
    );
    let mut input = entry(EvmByteArray::unknown_checked().unwrap());
    input.environment.to = evm_abstract::world::AddressInput::unknown_to();
    input.environment.caller = evm_abstract::world::AddressInput::unknown_caller();
    input.environment.value = evm_abstract::domain::EvmValue::top();
    let analysis = run_input(world, input.clone(), 20_000_000);
    evidence("symbolic-owner", &analysis);
    assert_eq!(analysis.entry().environment.to, input.environment.to);
    assert_eq!(
        analysis.entry().environment.caller,
        input.environment.caller
    );
    assert!(analysis.entry().environment.value.contains(U256::MAX));
    assert!(
        analysis
            .entry()
            .environment
            .calldata
            .len()
            .contains(U256::from(32))
    );
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    let record = analysis.procedure_summaries().last().unwrap();
    assert!(record.closed && record.source_current);
    let root = &record.procedures[record.root_procedure.unwrap()];
    assert_eq!(
        root.frame.process.as_ref().unwrap().owner,
        evm_abstract::analysis::ProcessOwner::Named(address(A))
    );
    let invocation = root.process_input.as_ref().unwrap();
    assert!(invocation.address.contains(U256::from(0xaaa)));
    assert!(invocation.address.contains(U256::from(0xbbb)));
    assert!(invocation.caller.contains(U256::from(0xccc)));
    assert!(invocation.value.contains(U256::MAX));
}
