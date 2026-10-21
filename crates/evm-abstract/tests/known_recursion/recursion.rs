use super::{fixtures, oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{ExecutionConfig, OutcomeKind},
    world::EvmByteArray,
};
use revm_bytecode::opcode;

#[test]
fn direct_unknown_counter_closes_at_default_depth_and_preserves_42() {
    let code = fixtures::counter(A as u16);
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 5)]);
    for use_summaries in [true, false] {
        let analysis = run_config(
            world.clone(),
            entry(unknown_input()),
            ExecutionConfig {
                use_summaries,
                ..ExecutionConfig::default()
            },
        );
        complete_recursion(&analysis);
        for outcome in terminals(&analysis, OutcomeKind::Return) {
            only(outcome.data.len(), &[32]);
            only(&output_word(outcome, 0), &[42]);
            only(&read(&outcome.store, A, 0), &[5]);
        }
        for n in [0, 1, 3, 6] {
            let observed = oracle::execute(&world, &entry(input(n)));
            assert_eq!(observed.kind, OutcomeKind::Return);
            assert_eq!(observed.output_word(0), U256::from(42));
            if n > 0 {
                assert!(
                    observed
                        .steps
                        .iter()
                        .any(|step| step.code == address(A) && step.pc == code.labels["call"])
                );
                assert!(
                    observed
                        .steps
                        .iter()
                        .any(|step| step.depth > 1 && step.opcode == opcode::RETURN)
                );
            }
            observed.covers(&analysis);
        }
    }
}

#[test]
fn mutual_unknown_counter_closes_without_expanding_the_full_call_stack() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::counter(B as u16), &[(0, 5)]);
    insert(&mut world, B, &fixtures::counter(A as u16), &[(0, 9)]);
    for use_summaries in [true, false] {
        let analysis = run_config(
            world.clone(),
            entry(unknown_input()),
            ExecutionConfig {
                use_summaries,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
        );
        complete_recursion(&analysis);
        for outcome in returns(&analysis) {
            only(&output_word(outcome, 0), &[42]);
            only(&read(&outcome.store, A, 0), &[5]);
            only(&read(&outcome.store, B, 0), &[9]);
        }
        for n in [0, 1, 2, 3, 6] {
            let observed = oracle::execute(&world, &entry(input(n)));
            assert_eq!(observed.kind, OutcomeKind::Return);
            assert_eq!(observed.output_word(0), U256::from(42));
            if n >= 2 {
                assert!(observed.steps.iter().any(|s| s.code == address(B)
                    && s.owner == address(B)
                    && s.caller == address(A)));
                assert!(
                    observed
                        .steps
                        .iter()
                        .any(|s| s.code == address(A) && s.depth >= 3 && s.caller == address(B))
                );
            }
            observed.covers(&analysis);
        }
    }
}

#[test]
fn recursion_without_a_basic_success_branch_has_nonempty_revert_outputs() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::no_base_success(), &[(0, 5)]);
    let analysis = run(world.clone(), entry(EvmByteArray::<NumericValue>::empty()));
    complete_recursion(&analysis);
    for outcome in terminals(&analysis, OutcomeKind::Revert) {
        only(outcome.data.len(), &[32]);
        only(&output_word(outcome, 0), &[99]);
        only(&read(&outcome.store, A, 0), &[5]);
    }
    let observed = oracle::execute(&world, &entry(EvmByteArray::<NumericValue>::empty()));
    assert_eq!(observed.kind, OutcomeKind::Revert);
    assert_eq!(observed.output_word(0), U256::from(99));
    observed.covers(&analysis);
}

#[test]
fn real_evm_depth_failure_is_covered_without_an_analysis_depth_frontier() {
    // Cancun has the same physical CALL depth boundary and no Osaka transaction
    // gas cap. Sufficient forwarded gas makes revm reach CallTooDeep, not OOG.
    let mut world = evm_abstract::world::EvmWorld::<NumericValue>::new(
        Fork::Cancun,
        "issue123:physical-call-depth",
    );
    insert(&mut world, A, &fixtures::no_base_success(), &[(0, 5)]);
    let entry = entry(EvmByteArray::<NumericValue>::empty());
    let observed = oracle::execute_gas(&world, &entry, 1_000_000_000_000);
    assert_eq!(observed.kind, OutcomeKind::Revert);
    assert_eq!(observed.output_word(0), U256::from(99));
    assert_eq!(
        observed.depth_failures, 1,
        "concrete run did not reach the EVM CallTooDeep branch"
    );
    assert!(observed.steps.iter().map(|s| s.depth).max().unwrap() >= 1024);
    let analysis = run(world, entry);
    complete_recursion(&analysis);
    for outcome in terminals(&analysis, OutcomeKind::Revert) {
        only(&output_word(outcome, 0), &[99]);
    }
    observed.covers(&analysis);
}
