//! Folding requires a repeated call edge that is still on the active stack.
//! Sequential leaf invocations do not establish a recursive bytecode cycle.

use super::{fixtures, oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{ExecutionConfig, FrontierReason, OutcomeKind, Status},
    world::ByteArray,
};
use revm_bytecode::opcode;

fn sequential_self_leaves() -> Fixture {
    let mut code = Assembly::default();
    code.op(opcode::CALLDATASIZE)
        .op(opcode::ISZERO)
        .jump("outer");
    code.input()
        .push(224)
        .op(opcode::SHR)
        .op(opcode::PUSH4)
        .op(0x34)
        .op(0x08)
        .op(0xe4)
        .op(0x70)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("wrong_selector");
    code.label("leaf")
        .op(opcode::CHAINID)
        .mstore(0)
        .exit(opcode::RETURN, 32);
    code.label("wrong_selector").return_word(99);
    code.label("outer").store(0, 42).push(0).mstore(64);
    // Both iterations execute this one CALL instruction. Its exact four-byte
    // input selects the leaf through the same immutable memory/view machinery
    // used by a bytecode dispatcher; completed edges are not active recursion.
    code.label("call")
        .op(opcode::PUSH4)
        .op(0x34)
        .op(0x08)
        .op(0xe4)
        .op(0x70)
        .mstore(0)
        .push(32)
        .push(0)
        .push(4)
        .push(28)
        .push(0)
        .op(opcode::ADDRESS)
        .op(opcode::GAS)
        .op(opcode::CALL)
        .op(opcode::ISZERO)
        .jump("failed")
        .push(64)
        .op(opcode::MLOAD)
        .push(1)
        .op(opcode::ADD)
        .op(opcode::DUP1)
        .mstore(64)
        .push(2)
        .op(opcode::GT)
        .jump("call")
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .exit(opcode::RETURN, 96);
    code.label("failed").exit(opcode::REVERT, 0);
    code.finish()
}

#[test]
fn repeated_sequential_self_leaf_calls_do_not_enter_recursive_summaries() {
    let code = sequential_self_leaves();
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 5)]);
    let mut input = entry(ByteArray::empty());
    input.environment.chain_id = Some(word(1));
    let observed = oracle::execute(&world, &input);
    assert_eq!(observed.kind, OutcomeKind::Return);
    assert_eq!(observed.data.len(), 96);
    for (offset, value) in [(0, 1), (32, 42), (64, 2)] {
        assert_eq!(observed.output_word(offset), U256::from(value));
    }
    assert_eq!(observed.slot(A, 0), U256::from(42));
    assert_eq!(
        observed
            .steps
            .iter()
            .filter(|step| step.depth == 2 && step.opcode == opcode::CHAINID)
            .count(),
        2
    );
    assert!(observed.steps.iter().all(|step| step.depth <= 2));
    assert!(
        observed
            .steps
            .iter()
            .filter(|step| step.depth == 2)
            .all(|step| !matches!(
                step.opcode,
                opcode::CALL
                    | opcode::STATICCALL
                    | opcode::DELEGATECALL
                    | opcode::CALLCODE
                    | opcode::CREATE
                    | opcode::CREATE2
            ))
    );
    for use_summaries in [true, false] {
        let analysis = run_config(
            world.clone(),
            input.clone(),
            ExecutionConfig {
                use_summaries,
                ..ExecutionConfig::default()
            },
        );
        complete(&analysis);
        assert!(analysis.procedure_summaries().is_empty());
        for outcome in returns(&analysis) {
            only(outcome.data.len(), &[96]);
            for (offset, value) in [(0, 1), (32, 42), (64, 2)] {
                only(&output_word(outcome, offset), &[value]);
            }
            only(&read(&outcome.store, A, 0), &[42]);
            only(&outcome.store.read_balance(address(A)), &[BALANCE]);
            assert!(outcome.store.possible_logs().is_empty());
            assert!(!outcome.store.logs_unknown());
        }
        observed.covers(&analysis);
    }
}

#[test]
fn changing_calldata_still_folds_real_direct_and_mutual_call_cycles() {
    for mutual in [false, true] {
        let mut world = new_world();
        insert(
            &mut world,
            A,
            &fixtures::counter(if mutual { B as u16 } else { A as u16 }),
            &[(0, 5)],
        );
        if mutual {
            insert(&mut world, B, &fixtures::counter(A as u16), &[(0, 9)]);
        }
        // The same active structural call edge repeats with distinct calldata.
        // Comparing complete input payloads would miss this genuine recursion.
        let input = entry(input(6));
        let observed = oracle::execute(&world, &input);
        assert_eq!(observed.kind, OutcomeKind::Return);
        assert_eq!(observed.output_word(0), U256::from(42));
        assert!(observed.steps.iter().any(|step| step.depth >= 5));
        if !mutual {
            // This changing-input cycle closes under the original detailed
            // budget. Explicit work-cutoff coverage lives in resources.rs.
            let default_policy = run_config(
                world.clone(),
                input.clone(),
                ExecutionConfig {
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
            );
            assert_eq!(default_policy.config().max_work, 20_000_000);
            assert_eq!(default_policy.config().max_call_depth, 32);
            complete_recursion(&default_policy);
            for outcome in returns(&default_policy) {
                only(outcome.data.len(), &[32]);
                only(&output_word(outcome, 0), &[42]);
                only(&read(&outcome.store, A, 0), &[5]);
            }
            observed.covers(&default_policy);
        }
        for use_summaries in [true, false] {
            let analysis = run_config(
                world.clone(),
                input.clone(),
                ExecutionConfig {
                    use_summaries,
                    max_work: 100_000_000,
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
            );
            complete_recursion(&analysis);
            for outcome in returns(&analysis) {
                only(outcome.data.len(), &[32]);
                only(&output_word(outcome, 0), &[42]);
                only(&read(&outcome.store, A, 0), &[5]);
                if mutual {
                    only(&read(&outcome.store, B, 0), &[9]);
                }
            }
            observed.covers(&analysis);
        }
    }
}

#[test]
fn analysis_depth_before_the_first_repeated_call_edge_stays_open() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::counter(A as u16), &[(0, 5)]);
    let analysis = run_config(
        world,
        entry(input(6)),
        ExecutionConfig {
            max_call_depth: 2,
            ..ExecutionConfig::default()
        },
    );
    // The first self call alone is not evidence of a recursive edge. Reaching
    // the next callee would exceed this analysis limit, rather than the EVM's
    // physical depth limit; a semantic CALL rejection cannot close that gap.
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(!analysis.frontiers().is_empty());
    assert!(
        analysis
            .frontiers()
            .iter()
            .all(|frontier| frontier.reason == FrontierReason::CallDepth)
    );
    assert!(analysis.procedure_summaries().is_empty());
}
