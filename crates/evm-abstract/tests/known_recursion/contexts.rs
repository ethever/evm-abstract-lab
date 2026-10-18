use super::{fixtures, oracle, support::*};
use alloy_primitives::{U256, hex};
use evm_abstract::{analysis::ExecutionConfig, world::Entry};
use revm_bytecode::opcode;

#[test]
fn recursive_call_delegatecall_and_callcode_preserve_their_contexts() {
    for op in [opcode::CALL, opcode::DELEGATECALL, opcode::CALLCODE] {
        let mut world = new_world();
        insert(
            &mut world,
            A,
            &fixtures::context_counter(op, B as u16),
            &[(0, 5)],
        );
        insert(
            &mut world,
            B,
            &fixtures::context_counter(op, A as u16),
            &[(0, 9)],
        );
        for count in [2, 6] {
            let input = entry(input(count));
            let observed = oracle::execute(&world, &input);
            assert_eq!(observed.kind, evm_abstract::analysis::OutcomeKind::Return);
            let caller = if op == opcode::DELEGATECALL {
                USER
            } else if op == opcode::CALLCODE {
                A
            } else {
                B
            };
            let value = if op == opcode::DELEGATECALL { 0 } else { 7 };
            for (offset, expected) in [(0, A), (32, caller), (64, value), (96, 5)] {
                assert_eq!(observed.output_word(offset), U256::from(expected));
            }
            let middle_owner = if op == opcode::CALL { B } else { A };
            let middle_caller = if op == opcode::DELEGATECALL { USER } else { A };
            assert!(observed.steps.iter().any(|s| s.code == address(B)
                && s.owner == address(middle_owner)
                && s.caller == address(middle_caller)
                && !s.is_static));
            for use_summaries in [true, false] {
                let config = ExecutionConfig {
                    use_summaries,
                    // Preserve the original input2/default policy. Input6
                    // repeats an active edge and needs a growing table bound.
                    max_work: if count == 2 {
                        ExecutionConfig::default().max_work
                    } else {
                        100_000_000
                    },
                    ..ExecutionConfig::default()
                };
                let analysis = run_config(world.clone(), input.clone(), config);
                if count == 2 {
                    complete(&analysis);
                    assert!(analysis.procedure_summaries().is_empty());
                } else {
                    complete_recursion(&analysis);
                }
                for outcome in returns(&analysis) {
                    for (offset, expected) in [(0, A), (32, caller), (64, value), (96, 5)] {
                        only(&output_word(outcome, offset), &[expected]);
                    }
                    only(&read(&outcome.store, A, 0), &[5]);
                    only(&read(&outcome.store, B, 0), &[9]);
                }
                oracle::unchanged(&analysis, &[A, B]);
                observed.covers(&analysis);
            }
        }
    }
}

#[test]
fn delegate_recursion_inherits_nonzero_value_and_the_original_caller() {
    const OUTER: u64 = 0x301;
    let mut wrapper = Assembly::default();
    wrapper
        .input()
        .mstore(0)
        .call(opcode::CALL, A as u16, 11, 32, 128)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 128);
    wrapper.label("failed").exit(opcode::REVERT, 0);
    let mut world = new_world();
    insert(&mut world, OUTER, &wrapper.finish(), &[]);
    insert(
        &mut world,
        A,
        &fixtures::context_counter(opcode::DELEGATECALL, B as u16),
        &[(0, 5)],
    );
    insert(
        &mut world,
        B,
        &fixtures::context_counter(opcode::DELEGATECALL, A as u16),
        &[(0, 9)],
    );
    for count in [2, 6] {
        let input = Entry::concrete(address(OUTER), address(USER), word(0), input(count));
        let observed = oracle::execute(&world, &input);
        assert_eq!(observed.output_word(0), U256::from(A));
        assert_eq!(observed.output_word(32), U256::from(OUTER));
        assert_eq!(observed.output_word(64), U256::from(11));
        let analysis = run_config(
            world.clone(),
            input,
            ExecutionConfig {
                max_work: if count == 2 {
                    ExecutionConfig::default().max_work
                } else {
                    100_000_000
                },
                ..ExecutionConfig::default()
            },
        );
        if count == 2 {
            complete(&analysis);
            assert!(analysis.procedure_summaries().is_empty());
        } else {
            complete_recursion(&analysis);
        }
        for outcome in returns(&analysis) {
            for (offset, expected) in [(0, A), (32, OUTER), (64, 11), (96, 5)] {
                only(&output_word(outcome, offset), &[expected]);
            }
            only(&outcome.store.read_balance(address(OUTER)), &[BALANCE - 11]);
            only(&outcome.store.read_balance(address(A)), &[BALANCE + 11]);
            only(&outcome.store.read_balance(address(B)), &[BALANCE]);
        }
        observed.covers(&analysis);
    }
}

#[test]
fn static_mutual_recursion_preserves_staticness_and_owner_storage() {
    let mut world = new_world();
    insert(
        &mut world,
        A,
        &fixtures::context_counter(opcode::STATICCALL, B as u16),
        &[(0, 5)],
    );
    insert(
        &mut world,
        B,
        &fixtures::context_counter(opcode::STATICCALL, A as u16),
        &[(0, 9)],
    );
    // The full128-byte symbolic context return needs about42M logical work.
    // Only the work allowance changes; resources checks the same input under
    // the complete default20M policy and requires its typed unfinished frontier.
    let analysis = run_config(
        world.clone(),
        entry(unknown_input()),
        ExecutionConfig {
            max_work: 100_000_000,
            ..ExecutionConfig::default()
        },
    );
    complete_recursion(&analysis);
    for n in [2, 3, 6] {
        let observed = oracle::execute(&world, &entry(input(n)));
        assert!(
            observed
                .steps
                .iter()
                .filter(|s| s.depth > 1)
                .all(|s| s.is_static)
        );
        assert_eq!(
            observed.output_word(96),
            U256::from(if n % 2 == 0 { 5 } else { 9 })
        );
        observed.covers(&analysis);
    }
    for outcome in analysis.outcomes() {
        only(&read(&outcome.store, A, 0), &[5]);
        only(&read(&outcome.store, B, 0), &[9]);
        assert!(outcome.store.possible_logs().is_empty());
    }
}

#[test]
fn identical_bytecode_at_distinct_owners_keeps_separate_storage() {
    let code = fixtures::same_code_two_owners(B as u16);
    eprintln!(
        "same-owner fixture: code=0x{} labels={:?}",
        hex::encode(&code.bytes),
        code.labels
    );
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 5)]);
    insert(&mut world, B, &code, &[(0, 9)]);
    let analysis = run_config(
        world.clone(),
        entry(unknown_input()),
        ExecutionConfig {
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
    );
    complete_recursion(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[5, 9]);
        only(&read(&outcome.store, A, 0), &[5]);
        only(&read(&outcome.store, B, 0), &[9]);
    }
    for n in [1, 2, 3, 6] {
        let observed = oracle::execute(&world, &entry(input(n)));
        assert_eq!(
            observed.output_word(0),
            U256::from(if n % 2 == 0 { 5 } else { 9 })
        );
        observed.covers(&analysis);
    }
}
