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
        // The shared trace is collected before each instruction. Pair the
        // attempted SSTORE with the resumed caller's actual CALL result.
        let (sstore_index, sstore) = observed
            .steps
            .iter()
            .enumerate()
            .find(|(_, step)| {
                step.owner == address(A)
                    && step.code == address(A)
                    && step.caller == address(U)
                    && step.opcode == opcode::SSTORE
            })
            .unwrap();
        assert_eq!(sstore.storage_write, Some((U256::ZERO, U256::from(1))));
        let caller = observed.steps[..sstore_index]
            .iter()
            .rfind(|step| {
                step.owner == address(U)
                    && step.code == address(U)
                    && step.depth + 1 == sstore.depth
                    && step.opcode == opcode::CALL
            })
            .unwrap();
        assert_eq!(caller.stack[caller.stack.len() - 2], U256::from(A));
        let (resume_index, resumed) = observed
            .steps
            .iter()
            .enumerate()
            .skip(sstore_index + 1)
            .find(|(_, step)| step.depth == caller.depth)
            .unwrap();
        assert_eq!((resumed.owner, resumed.code), (address(U), address(U)));
        assert_eq!(resumed.opcode, opcode::POP);
        assert_eq!(
            resumed.stack.last(),
            Some(&U256::from(u64::from(op == opcode::CALL)))
        );
        if op == opcode::CALL {
            let stopped = &observed.steps[sstore_index + 1];
            assert_eq!((stopped.owner, stopped.code), (address(A), address(A)));
            assert_eq!((stopped.depth, stopped.pc), (sstore.depth, sstore.pc + 1));
            assert_eq!(stopped.opcode, opcode::STOP);
        } else {
            assert!(sstore.is_static && resumed.is_static);
            assert_eq!(resume_index, sstore_index + 1);
        }
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
        let mut current_callback_witnesses = 0;
        let mut static_callback_failures = 0;
        let mut records = Vec::new();
        // The abstract environment also permits zero, repeated and nested
        // callbacks. Bind a positive receipt to its own subscriber and callee;
        // these counts are not an enumeration or a must-call claim.
        for record in analysis.procedure_summaries() {
            if op == opcode::STATICCALL && record.closed && record.source_current {
                assert!(record.procedures.iter().all(|p| p.frame.is_static));
                assert!(record.regional_cfg.nodes.iter().all(|n| n.frame.is_static));
            }
            for call in &record.regional_cfg.calls {
                let caller_node = &record.regional_cfg.nodes[call.from];
                let from = &caller_node.frame;
                let callee = &record.procedures[call.callee];
                known_to_unknown += usize::from(
                    from.mode == FrameCode::Runtime && callee.frame.mode == FrameCode::Unknown,
                );
                let callback =
                    from.mode == FrameCode::Unknown && callee.frame.mode == FrameCode::Runtime;
                unknown_to_known += usize::from(callback);
                if !callback || !record.closed || !record.source_current {
                    continue;
                }
                assert_eq!(callee.semantics, TransferSemantics::Native);
                assert_eq!(callee.frame.code_address, address(A));
                assert_eq!(callee.frame.address, address(A));
                assert_eq!(
                    callee.frame.code_hash,
                    alloy_primitives::keccak256(&a.bytes)
                );
                assert_eq!(callee.frame.is_static, op == opcode::STATICCALL);
                assert_eq!(from.is_static, callee.frame.is_static);
                let source = caller_node.transfer.as_ref().unwrap();
                assert!(!caller_node.excluded && source.current && source.complete);
                assert_eq!(source.semantics, TransferSemantics::Environment);
                assert_eq!(source.input_revision, caller_node.input_revision);
                assert!(source.calls.contains(&call.id));
                assert!(!call.entries.is_empty());
                assert!(
                    call.entries.iter().all(|entry| {
                        record.regional_cfg.nodes[*entry].procedure == call.callee
                    })
                );
                let kind = if op == opcode::STATICCALL {
                    OutcomeKind::Failure
                } else {
                    OutcomeKind::Return
                };
                let body = record
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| {
                        node.procedure == call.callee
                            && !node.excluded
                            && node.frame.code_address == callee.frame.code_address
                            && node.frame.code_hash == callee.frame.code_hash
                            && node.frame.address == callee.frame.address
                            && node.frame.address_value == callee.frame.address_value
                            && node.frame.caller == callee.frame.caller
                            && node.frame.is_static == callee.frame.is_static
                            && analysis
                                .world()
                                .runtime(address(A))
                                .unwrap()
                                .blocks()
                                .get(node.frame.basic_block_index)
                                .is_some_and(|block| {
                                    block
                                        .instructions
                                        .iter()
                                        .any(|i| i.opcode == opcode::SSTORE)
                                })
                            && node.transfer.as_ref().is_some_and(|transfer| {
                                transfer.current
                                    && transfer.complete
                                    && transfer.visited_block
                                    && transfer.semantics == TransferSemantics::Native
                                    && transfer.input_revision == node.input_revision
                                    && transfer.outcomes.contains(&kind)
                            })
                    })
                    .expect("current source-bound callback SSTORE transfer");
                assert_eq!(body.procedure, call.callee);
                let output = callee
                    .outputs
                    .iter()
                    .find(|output| output.kind == kind)
                    .unwrap();
                // Body and subscriber-template versions are independent clocks.
                // A delivery binds only its template and THIS callee generation.
                assert!(call.deliveries.iter().any(|delivery| {
                    delivery.current
                        && delivery.kind == output.kind
                        && delivery.input_revision == call.input_revision
                        && delivery.output_generation == output.generation
                }));
                if op == opcode::STATICCALL {
                    assert!(!callee.outputs.is_empty());
                    for output in &callee.outputs {
                        assert_ne!(output.kind, OutcomeKind::Return);
                        assert_eq!(
                            serde_json::to_value(&output.store).unwrap(),
                            serde_json::to_value(&callee.input).unwrap()
                        );
                        only(output.store.storage_bound(address(A), U256::ZERO), &[0]);
                        static_callback_failures +=
                            usize::from(output.kind == OutcomeKind::Failure);
                    }
                } else {
                    only(output.store.storage_bound(address(A), U256::ZERO), &[1]);
                }
                callback_sstore_visited = true;
                current_callback_witnesses += 1;
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
                "current_callback_witnesses": current_callback_witnesses,
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
            assert!(callback_sstore_visited && current_callback_witnesses > 0);
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
            // The resumed concrete CALL has failed, while the same abstract
            // callback preserves its whole Store. A block visit is not a write.
            assert!(known_to_unknown > 0 && unknown_to_known > 0);
            assert!(callback_sstore_visited && current_callback_witnesses > 0);
            assert!(static_callback_failures > 0);
        }
        statuses.push(analysis.status());
    }
    assert!(
        statuses.iter().all(|s| *s == Status::Converged),
        "{statuses:?}"
    );
}
