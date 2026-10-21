use super::{fixtures, oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::{analysis::OutcomeKind, world::EvmByteArray};
use revm_bytecode::opcode;

/// The abstract U stays absent. Each independent oracle world refines it
/// with different actual bytecode and checks the same abstract analysis.
#[test]
fn finite_zero_one_multiple_callbacks_are_included_and_two_obeys_its_guard() {
    let a = fixtures::busy_x(false);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 0), (1, 0)]);
    let input = entry(calldata(0));
    let analysis = run(world.clone(), input.clone());
    complete_unknown(&analysis);
    for (selectors, expected) in [
        (vec![], 0),
        (vec![2], 0),
        (vec![1], 1),
        (vec![1, 2], 2),
        (vec![1, 2, 2, 1, 2], 2),
        (vec![1, 2, 1], 1),
    ] {
        let concrete =
            oracle::concrete_unknown(&world, U, &fixtures::callback_sequence(&selectors, false));
        let observed = oracle::execute(&concrete, &input);
        assert_eq!(observed.kind, OutcomeKind::Return);
        assert_eq!(observed.output_word(0), U256::from(expected));
        assert_eq!(observed.output_word(32), U256::ONE);
        assert_eq!(observed.slot(A, 0), U256::ZERO);
        assert_eq!(observed.slot(A, 1), U256::from(expected));
        for selector in selectors {
            let label = if selector == 1 { "one" } else { "two" };
            assert!(observed.steps.iter().any(|s| s.code == address(A)
                && s.pc == a.labels[label]
                && s.caller == address(U)));
        }
        observed.covers(&analysis);
    }
}

#[test]
fn actual_nested_and_indirect_unknown_callbacks_are_included() {
    let a = fixtures::busy_x(true);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 0), (1, 0)]);
    let input = entry(calldata(0));
    let analysis = run_config(world.clone(), input.clone(), callback_closure_config());
    complete_unknown(&analysis);
    let mut concrete =
        oracle::concrete_unknown(&world, U, &fixtures::callback_sequence(&[3, 2], false));
    insert(
        &mut concrete,
        V,
        &fixtures::callback_sequence(&[1], false),
        &[],
    );
    let observed = oracle::execute(&concrete, &input);
    assert_eq!(observed.output_word(0), U256::from(2));
    assert!(
        observed
            .steps
            .iter()
            .any(|s| s.code == address(A) && s.pc == a.labels["three"] && s.caller == address(U))
    );
    assert!(
        observed
            .steps
            .iter()
            .any(|s| s.code == address(A) && s.pc == a.labels["one"] && s.caller == address(V))
    );
    observed.covers(&analysis);
}

#[test]
fn actual_unknown_to_known_c_write_and_readonly_guard_match_revm() {
    let a = fixtures::external_reader(false);
    let c = fixtures::external_cell();
    let mut u = Assembly::default();
    u.push(1)
        .mstore(0)
        .call(opcode::CALL, C as u16, 0, (0, 32), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let u = u.finish();
    for enabled in [0, 1] {
        let mut world = new_world();
        insert(&mut world, A, &a, &[]);
        insert(&mut world, C, &c, &[(0, 5), (1, enabled)]);
        let input = entry(EvmByteArray::<NumericValue>::empty());
        let analysis = run_config(
            world.clone(),
            input.clone(),
            evm_abstract::analysis::ExecutionConfig {
                max_work: if enabled == 1 {
                    callback_closure_config().max_work
                } else {
                    evm_abstract::analysis::ExecutionConfig::default().max_work
                },
                root_terminal_coverage: false,
                ..Default::default()
            },
        );
        complete_unknown(&analysis);
        let concrete = oracle::concrete_unknown(&world, U, &u);
        let observed = oracle::execute(&concrete, &input);
        assert_eq!(observed.output_word(0), U256::from(5));
        assert_eq!(
            observed.output_word(32),
            U256::from(if enabled == 1 { 9 } else { 5 })
        );
        assert!(
            observed.steps.iter().any(|s| s.code == address(C)
                && s.pc == c.labels["write"]
                && s.caller == address(U))
        );
        observed.covers(&analysis);
    }
}

#[test]
fn unknown_delegating_known_code_still_covers_its_guarded_cross_owner_call() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .jump("run", true)
        .op(opcode::CALLER)
        .push(U as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("denied", true)
        .label("allowed")
        .store(0, 9)
        .op(opcode::STOP);
    a.label("run")
        .store(0, 5)
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .return_top();
    a.label("denied").op(opcode::STOP);
    let a = a.finish();
    let mut c = Assembly::default();
    c.store(0, 77)
        .call(opcode::CALL, A as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let c = c.finish();
    let mut u = Assembly::default();
    u.call(opcode::DELEGATECALL, C as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 5)]);
    insert(&mut world, C, &c, &[(0, 3)]);
    let input = entry(EvmByteArray::<NumericValue>::empty());
    let concrete = oracle::concrete_unknown(&world, U, &u.finish());
    let observed = oracle::execute(&concrete, &input);
    assert_eq!(observed.output_word(0), U256::from(9));
    assert_eq!(observed.slot(A, 0), U256::from(9));
    assert_eq!(observed.slot(C, 0), U256::from(3));
    assert!(
        observed
            .steps
            .iter()
            .any(|step| step.code == address(C) && step.owner == address(U))
    );
    assert!(observed.steps.iter().any(|step| step.code == address(A)
        && step.owner == address(A)
        && step.caller == address(U)
        && step.pc == a.labels["allowed"]));
    let analysis = run(world, input);
    complete_unknown(&analysis);
    observed.covers(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[5, 9]);
        only(&read(&outcome.store, C, 0), &[3, 77]);
    }
    assert!(reachable(&analysis, A, a.labels["allowed"]));
}

#[test]
fn nested_callbacks_keep_default_ledger_root_coverage() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::busy_x(true), &[(0, 0), (1, 0)]);
    default_callback_coverage(world, entry(calldata(0)));
}
