use super::{fixtures, oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{ExecutionConfig, OutcomeKind},
    domain::Domain,
};

#[test]
fn growing_recursive_storage_and_transient_state_close_without_losing_other_facts() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::growth(), &[(0, 0), (1, 77)]);
    // This symbolic Store fixed point needs about59M measured logical work.
    // Keep depth and every precision policy at the library defaults; resources
    // separately verifies its actual default20M unfinished frontier.
    let analysis = run_config(
        world.clone(),
        entry(unknown_input()),
        ExecutionConfig {
            max_work: 100_000_000,
            ..ExecutionConfig::default()
        },
    );
    complete_recursion(&analysis);
    // Include growth beyond the default finite-constant capacity of eight.
    for n in [0, 1, 2, 3, 6, 17] {
        let observed = oracle::execute(&world, &entry(input(n)));
        assert_eq!(observed.kind, OutcomeKind::Return);
        assert_eq!(observed.output_word(0), U256::from(n));
        assert_eq!(observed.output_word(32), U256::from(n));
        assert_eq!(observed.output_word(64), U256::from(77));
        assert_eq!(observed.slot(A, 0), U256::from(n));
        observed.covers(&analysis);
    }
    for outcome in analysis.outcomes() {
        only(&read(&outcome.store, A, 1), &[77]);
        only(&outcome.store.read_balance(address(A)), &[BALANCE]);
        assert!(outcome.store.possible_logs().is_empty());
    }
    oracle::unchanged(&analysis, &[A]);
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .flat_map(|r| &r.procedures)
            .any(|p| read(&p.input, A, 0).contains(U256::from(2)))
    );
}

fn child_rollback(exceptional: bool) {
    let mut world = new_world();
    insert(
        &mut world,
        A,
        &fixtures::rollback(exceptional, false),
        &[(0, 1)],
    );
    let analysis = run(world.clone(), entry(unknown_input()));
    complete_recursion(&analysis);
    for outcome in terminals(&analysis, OutcomeKind::Return) {
        for (offset, expected) in [(0, 0), (32, 5), (64, 7)] {
            only(&output_word(outcome, offset), &[expected]);
        }
        only(&read(&outcome.store, A, 0), &[5]);
        // RETURN reads the restored parent value 7 before transaction completion;
        // the terminal Store starts the next transaction with transient slots zero.
        only(
            &outcome
                .store
                .read_transient(address(A), &word(0), Domain::default()),
            &[0],
        );
        only(&outcome.store.read_balance(address(A)), &[BALANCE]);
        assert!(!outcome.store.logs_unknown());
        assert_eq!(outcome.store.possible_logs().len(), 1);
        let (site, log) = outcome.store.possible_logs().iter().next().unwrap();
        assert_eq!(site.address, address(A));
        assert_eq!(log.topics.len(), 1);
        only(&log.topics[0], &[700]);
        only(log.data.len(), &[0]);
    }
    oracle::unchanged(&analysis, &[A]);
    for n in [0, 1, 3, 6] {
        let observed = oracle::execute(&world, &entry(input(n)));
        assert_eq!(observed.kind, OutcomeKind::Return);
        assert_eq!(observed.output_word(0), U256::ZERO);
        assert_eq!(observed.output_word(32), U256::from(5));
        assert_eq!(observed.output_word(64), U256::from(7));
        assert_eq!(observed.logs.len(), 1);
        assert_eq!(observed.logs[0].1, vec![U256::from(700)]);
        observed.covers(&analysis);
    }
}

#[test]
fn recursive_revert_restores_child_effects_and_parent_continues() {
    child_rollback(false);
}

#[test]
fn recursive_exception_restores_child_effects_and_parent_continues() {
    child_rollback(true);
}

#[test]
fn outer_revert_restores_all_parent_and_recursive_child_effects() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::rollback(false, true), &[(0, 1)]);
    let analysis = run(world.clone(), entry(unknown_input()));
    complete_recursion(&analysis);
    for outcome in terminals(&analysis, OutcomeKind::Revert) {
        only(&read(&outcome.store, A, 0), &[1]);
        only(
            &outcome
                .store
                .read_transient(address(A), &word(0), Domain::default()),
            &[0],
        );
        only(&outcome.store.read_balance(address(A)), &[BALANCE]);
        assert!(!outcome.store.logs_unknown());
        assert!(outcome.store.possible_logs().is_empty());
        for (offset, expected) in [(0, 0), (32, 5), (64, 7)] {
            only(&output_word(outcome, offset), &[expected]);
        }
    }
    oracle::unchanged(&analysis, &[A]);
    for n in [0, 1, 3, 6] {
        let observed = oracle::execute(&world, &entry(input(n)));
        assert_eq!(observed.kind, OutcomeKind::Revert);
        assert_eq!(observed.slot(A, 0), U256::from(1));
        assert!(observed.logs.is_empty());
        observed.covers(&analysis);
    }
}
