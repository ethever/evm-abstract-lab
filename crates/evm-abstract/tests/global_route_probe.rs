//! Isolated route audit. All abstract execution uses the unchanged native engine.
#![allow(dead_code)]

#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;

use alloy_primitives::{U256, hex};
use evm_abstract::{
    analysis::{
        Acceleration, AnalysisEngine, AnalysisInput, BoundedJoin, EnginePolicies, ExecutionConfig,
        FeedbackAcceleration, FirstResumeStatus, FrameCode, MachineError, OutcomeKind,
        PolicyContext, RecentDistinctJumps, RegionalSummaries, RelativeState, SemanticPolicies,
        Status, StructuralPartition, TransferSemantics, control::Control, cooperation::NoExtension,
    },
    domain::NumericValue,
    resource::WorkBudget,
    world::{EvmAccount, EvmByteArray, Existence},
};
use revm_bytecode::opcode;
use serde_json::json;
use support::{A, Assembly, U, address, entry, insert, new_world, only, read, word};

fn subject(op: u8) -> support::Fixture {
    let mut a = Assembly::default();
    a.input(0).op(opcode::ISZERO).jump("outer", true);
    a.label("callback_write").store(0, 1).op(opcode::STOP);
    a.label("outer")
        .call(op, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .push(0)
        .op(opcode::REVERT);
    a.finish()
}

fn concrete_u() -> support::Fixture {
    let mut u = Assembly::default();
    u.push(1)
        .mstore(0)
        .call(opcode::CALL, A as u16, 0, (0, 32), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    u.finish()
}

#[test]
fn native_cycle_rollback_and_api_boundary() {
    let values = native::product();
    let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            RegionalSummaries::with_early_known_admission(),
        ),
    );
    let mut statuses = Vec::new();
    for (label, op) in [("ordinary", opcode::CALL), ("static", opcode::STATICCALL)] {
        let a = subject(op);
        let u = concrete_u();
        let mut world = new_world();
        insert(&mut world, A, &a, &[(0, 0)]);
        world
            .insert(
                address(U),
                EvmAccount {
                    balance: word(1_000_000),
                    nonce: word(0),
                    existence: Existence::Present,
                    storage_unknown: false,
                    ..EvmAccount::<NumericValue>::unknown()
                },
            )
            .unwrap();
        let input = entry(EvmByteArray::<NumericValue>::empty());
        let concrete = oracle::concrete_unknown(&world, U, &u);
        let observed = oracle::execute(&concrete, &input);
        assert_eq!(observed.kind, OutcomeKind::Revert);
        assert_eq!(observed.slot(A, 0), U256::ZERO);
        assert!(observed.steps.iter().any(|step| {
            step.owner == address(A)
                && step.code == address(A)
                && step.opcode == opcode::SSTORE
                && step.is_static == (op == opcode::STATICCALL)
        }));
        let config = ExecutionConfig {
            root_terminal_coverage: false,
            max_work: 100_000_000,
            ..ExecutionConfig::default()
        };
        let mut extension = NoExtension;
        let analysis = AnalysisEngine::new(&values, &policies, &mut extension)
            .run(
                AnalysisInput::new(world, input.clone()).unwrap(),
                config,
                &Control::default(),
            )
            .unwrap();
        let mut known_to_unknown = 0;
        let mut unknown_to_known = 0;
        let mut callback_sstore_visited = false;
        let mut current_static_callbacks = 0;
        let mut static_callback_failures = 0;
        let mut records = Vec::new();
        for record in analysis.procedure_summaries() {
            for call in &record.regional_cfg.calls {
                let from = &record.regional_cfg.nodes[call.from].frame;
                let callee = &record.procedures[call.callee].frame;
                known_to_unknown += usize::from(
                    from.mode == FrameCode::Runtime && callee.mode == FrameCode::Unknown,
                );
                unknown_to_known += usize::from(
                    from.mode == FrameCode::Unknown && callee.mode == FrameCode::Runtime,
                );
            }
            if op == opcode::STATICCALL && record.closed && record.source_current {
                for procedure in &record.procedures {
                    if procedure.frame.mode == FrameCode::Runtime {
                        assert_eq!(procedure.semantics, TransferSemantics::Native);
                        assert_eq!(procedure.frame.code_address, address(A));
                        assert_eq!(procedure.frame.address, address(A));
                        assert_eq!(
                            procedure.frame.code_hash,
                            alloy_primitives::keccak256(&a.bytes)
                        );
                        assert!(procedure.frame.is_static);
                        assert!(!procedure.outputs.is_empty());
                        for output in &procedure.outputs {
                            assert_ne!(output.kind, OutcomeKind::Return);
                            assert_eq!(
                                serde_json::to_value(&output.store).unwrap(),
                                serde_json::to_value(&procedure.input).unwrap()
                            );
                            only(output.store.storage_bound(address(A), U256::ZERO), &[0]);
                            static_callback_failures +=
                                usize::from(output.kind == OutcomeKind::Failure);
                        }
                        current_static_callbacks += 1;
                    }
                }
            }
            for node in &record.regional_cfg.nodes {
                let block = analysis
                    .world()
                    .runtime(address(A))
                    .and_then(|program| program.blocks().get(node.frame.basic_block_index));
                callback_sstore_visited |= node.frame.code_address == address(A)
                    && node
                        .transfer
                        .as_ref()
                        .is_some_and(|t| t.current && t.complete && t.visited_block)
                    && block.is_some_and(|block| {
                        block
                            .instructions
                            .iter()
                            .any(|i| i.opcode == opcode::SSTORE)
                    });
            }
            records.push(json!({
                "closed": record.closed, "source_current": record.source_current,
                "kind": record.kind, "states": record.state_count,
                "procedures": record.procedures.len(), "bodies": record.regional_cfg.nodes.len(),
                "subscribers": record.regional_cfg.calls.len(), "transfers": record.transfers,
                "propagations": record.propagations,
                "identities": record.procedures.iter().map(|p| json!({
                    "code": p.frame.code_address, "hash": p.frame.code_hash, "mode": p.frame.mode,
                    "owner": p.frame.address, "caller": p.frame.caller, "static": p.frame.is_static,
                    "dependencies": p.dependencies, "outputs": p.outputs.iter().map(|o| o.kind).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            }));
        }
        eprintln!(
            "GLOBAL_ROUTE {}",
            json!({
                "case": label, "bytecode_a": hex::encode(&a.bytes), "concrete_bytecode_u": hex::encode(&u.bytes),
                "status": analysis.status(), "work": analysis.work(), "outer_states": analysis.states().len(),
                "transfers": analysis.transfers(), "frontiers": analysis.frontiers().iter().map(|f| format!("{:?}", f.reason)).collect::<Vec<_>>(),
                "outcomes": analysis.outcomes().iter().map(|o| o.kind).collect::<Vec<_>>(),
                "known_to_unknown_calls_across_snapshots": known_to_unknown,
                "unknown_to_known_calls_across_snapshots": unknown_to_known,
                "current_callback_sstore_body": callback_sstore_visited,
                "current_static_callbacks": current_static_callbacks,
                "static_callback_failures": static_callback_failures,
                "concrete_callback_sstore": true, "concrete_static": op == opcode::STATICCALL,
                "concrete_rollback_slot0": 0, "root_terminal_coverage": false,
                "records": records,
            })
        );
        observed.covers(&analysis);
        assert!(!analysis.outcomes().is_empty());
        for outcome in analysis.outcomes() {
            assert_ne!(outcome.kind, OutcomeKind::Return);
            only(&read(&outcome.store, A, 0), &[0]);
        }
        if op == opcode::CALL {
            assert!(known_to_unknown > 0 && unknown_to_known > 0);
            assert!(callback_sstore_visited);
            // This is an API counterexample, not a new owner abstraction:
            // the same known code under another state owner is a valid frame,
            // but current coarse/relative admission refuses their union.
            let first = &analysis.states()[0].entry;
            let mut other_owner = first.clone();
            other_owner.call_stack.active_mut().key.address = address(0x302);
            other_owner.call_stack.active_mut().key.address_value = address(0x302).into();
            let mut budget = WorkBudget::new(20_000_000);
            let mut cx = PolicyContext::new(values.borrowed(), &input, &mut budget);
            let left = RelativeState::project(first, &mut cx)
                .unwrap()
                .coarsen(&mut cx)
                .unwrap();
            let right = RelativeState::project(&other_owner, &mut cx)
                .unwrap()
                .coarsen(&mut cx)
                .unwrap();
            assert!(matches!(
                left.regional_join(&right, Acceleration::Widen, &mut cx),
                Err(MachineError::Shape)
            ));
            eprintln!(
                "GLOBAL_ROUTE {}",
                json!({"case": "owner_merge_api", "same_known_code": true,
                "left_owner": left.active_frame().address, "right_owner": right.active_frame().address,
                "both_coarsened": true, "regional_join": "MachineError::Shape"})
            );
        } else {
            // A static callback reaches SSTORE and fails there, as the independent
            // concrete trace above observes. Visiting that block is not a write.
            assert!(known_to_unknown > 0 && unknown_to_known > 0);
            assert!(callback_sstore_visited);
            assert!(current_static_callbacks > 0 && static_callback_failures > 0);
        }
        statuses.push(analysis.status());
    }
    assert!(
        statuses.iter().all(|s| *s == Status::Converged),
        "{statuses:?}"
    );
}
