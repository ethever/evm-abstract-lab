use super::{fixtures, oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::OutcomeKind,
    domain::Domain,
    world::{Account, ByteArray, Existence, World},
};
use revm_bytecode::opcode;

fn parent(outer_revert: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .jump("run", true)
        .input(0)
        .op(opcode::DUP1)
        .push(1)
        .op(opcode::EQ)
        .jump("callback_success", true)
        .push(2)
        .op(opcode::EQ)
        .jump("callback_fail", true)
        .op(opcode::STOP);
    a.label("run")
        .store(0, 7)
        .push(7)
        .push(0)
        .op(opcode::TSTORE)
        .push(0)
        .push(0)
        .label("parent_log")
        .op(opcode::LOG0)
        .call(opcode::CALL, U as u16, 7, (0, 0), (0, 0))
        .op(opcode::DUP1)
        .op(opcode::ISZERO)
        .jump("child_failed", true)
        .label("child_succeeded");
    finish_parent(&mut a, outer_revert);
    a.label("child_failed");
    finish_parent(&mut a, outer_revert);
    a.label("callback_success")
        .op(opcode::POP)
        .store(1, 11)
        .push(11)
        .push(1)
        .op(opcode::TSTORE)
        .push(0)
        .push(0)
        .op(opcode::LOG0)
        .op(opcode::STOP);
    a.label("callback_fail")
        .store(1, 13)
        .push(13)
        .push(1)
        .op(opcode::TSTORE)
        .push(0)
        .push(0)
        .op(opcode::LOG0)
        .push(0x55)
        .mstore(0)
        .push(32)
        .push(0)
        .op(opcode::REVERT);
    a.finish()
}

fn finish_parent(a: &mut Assembly, outer_revert: bool) {
    a.mstore(0)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .push(1)
        .op(opcode::SLOAD)
        .mstore(64)
        .push(1)
        .op(opcode::TLOAD)
        .mstore(96)
        .push(128)
        .push(0)
        .op(if outer_revert {
            opcode::REVERT
        } else {
            opcode::RETURN
        });
}

fn world(outer_revert: bool) -> World {
    let mut world = new_world();
    insert(&mut world, A, &parent(outer_revert), &[(0, 5), (1, 3)]);
    let mut unknown = Account::unknown();
    unknown.balance = word(1_000_000);
    unknown.existence = Existence::Present;
    world.insert(address(U), unknown).unwrap();
    world
}

#[test]
fn failed_callback_can_be_handled_by_unknown_u_that_later_succeeds() {
    let world = world(false);
    let analysis = run(world.clone(), entry(ByteArray::empty()));
    complete_unknown(&analysis);
    for (selectors, expected) in [(vec![2], 3), (vec![1, 2], 11)] {
        // Validate the concrete fixture before asking the abstract graph to
        // cover it: a failing callback must not become a whole-U failure.
        let concrete =
            oracle::concrete_unknown(&world, U, &fixtures::callback_sequence(&selectors, false));
        let observation = oracle::execute(&concrete, &entry(ByteArray::empty()));
        assert_eq!(observation.kind, OutcomeKind::Return);
        assert_eq!(observation.output_word(0), U256::ONE);
        assert_eq!(observation.output_word(32), U256::from(7));
        assert_eq!(observation.output_word(64), U256::from(expected));
        assert_eq!(
            observation.output_word(96),
            U256::from(if expected == 11 { 11 } else { 0 })
        );
        assert_eq!(observation.slot(A, 0), U256::from(7));
        assert_eq!(observation.slot(A, 1), U256::from(expected));
        assert_eq!(observation.balances[&address(A)], U256::from(999_993));
        assert_eq!(observation.balances[&address(U)], U256::from(1_000_007));
        assert_eq!(observation.logs.len(), if expected == 11 { 2 } else { 1 });
        observation.covers(&analysis);
    }
}

#[test]
fn successful_callback_is_rolled_back_when_unknown_u_reverts_without_losing_parent_write() {
    let world = world(false);
    let analysis = run(world.clone(), entry(ByteArray::empty()));
    complete_unknown(&analysis);
    let concrete = oracle::concrete_unknown(&world, U, &fixtures::callback_sequence(&[1], true));
    let observation = oracle::execute(&concrete, &entry(ByteArray::empty()));
    assert_eq!(observation.output_word(0), U256::ZERO);
    assert_eq!(observation.output_word(32), U256::from(7));
    assert_eq!(observation.output_word(64), U256::from(3));
    assert_eq!(observation.output_word(96), U256::ZERO);
    assert_eq!(
        observation.logs.len(),
        1,
        "only the existing parent log commits"
    );
    assert_eq!(observation.slot(A, 0), U256::from(7));
    assert_eq!(observation.slot(A, 1), U256::from(3));
    assert_eq!(observation.balances[&address(A)], U256::from(1_000_000));
    assert_eq!(observation.balances[&address(U)], U256::from(1_000_000));
    observation.covers(&analysis);
    let before_finalization: Vec<_> = analysis
        .states()
        .iter()
        .filter(|state| {
            state.key.frames.len() == 1
                && state.active().code_address == address(A)
                && state
                    .executed_pcs
                    .contains(&parent(false).labels["child_failed"])
        })
        .collect();
    assert!(
        !before_finalization.is_empty(),
        "child failure has no executed caller continuation"
    );
    for state in before_finalization {
        only(
            &state
                .entry
                .store
                .read_transient(address(A), &word(0), Domain::default()),
            &[7],
        );
        only(
            &state
                .entry
                .store
                .read_transient(address(A), &word(1), Domain::default()),
            &[0],
        );
    }
    let failures: Vec<_> = returns(&analysis)
        .into_iter()
        .filter(|o| output_word(o, 0).singleton() == Some(U256::ZERO))
        .collect();
    assert!(
        !failures.is_empty(),
        "bytecode's child-failed continuation is missing"
    );
    for outcome in failures {
        only(&read(&outcome.store, A, 0), &[7]);
        only(&read(&outcome.store, A, 1), &[3]);
        only(
            &outcome
                .store
                .read_transient(address(A), &word(0), Domain::default()),
            &[0],
        );
        only(
            &outcome
                .store
                .read_transient(address(A), &word(1), Domain::default()),
            &[0],
        );
        only(&outcome.store.read_balance(address(A)), &[1_000_000]);
        only(&outcome.store.read_balance(address(U)), &[1_000_000]);
        assert!(
            !outcome.store.logs_unknown(),
            "U rollback retained uncommitted unknown logs"
        );
        assert!(
            outcome
                .store
                .possible_logs()
                .keys()
                .any(|site| site.address == address(A)
                    && site.pc == parent(false).labels["parent_log"] + 1),
            "U rollback swallowed the existing parent log"
        );
        assert!(
            outcome
                .store
                .possible_logs()
                .keys()
                .all(|site| site.address == address(A)
                    && site.pc < parent(false).labels["child_succeeded"])
        );
    }
}

#[test]
fn exceptional_unknown_u_failure_also_restores_only_its_own_subtree() {
    let world = world(false);
    let analysis = run(world.clone(), entry(ByteArray::empty()));
    complete_unknown(&analysis);
    let mut u = fixtures::callback_sequence(&[1], false);
    *u.bytes.last_mut().unwrap() = opcode::INVALID;
    let concrete = oracle::concrete_unknown(&world, U, &u);
    let observation = oracle::execute(&concrete, &entry(ByteArray::empty()));
    assert_eq!(observation.output_word(0), U256::ZERO);
    assert_eq!(observation.output_word(32), U256::from(7));
    assert_eq!(observation.output_word(64), U256::from(3));
    observation.covers(&analysis);
}

#[test]
fn successful_unknown_u_effects_remain_inside_outer_a_rollback_scope() {
    let world = world(true);
    let analysis = run(world.clone(), entry(ByteArray::empty()));
    complete_unknown(&analysis);
    let concrete = oracle::concrete_unknown(&world, U, &fixtures::callback_sequence(&[1], false));
    let observation = oracle::execute(&concrete, &entry(ByteArray::empty()));
    assert_eq!(observation.kind, OutcomeKind::Revert);
    assert_eq!(observation.output_word(0), U256::ONE);
    assert_eq!(observation.output_word(32), U256::from(7));
    assert_eq!(observation.output_word(64), U256::from(11));
    assert_eq!(observation.slot(A, 0), U256::from(5));
    assert_eq!(observation.slot(A, 1), U256::from(3));
    assert!(observation.logs.is_empty());
    observation.covers(&analysis);
    assert!(!analysis.outcomes().is_empty());
    for outcome in analysis.outcomes() {
        assert_ne!(outcome.kind, OutcomeKind::Return);
        only(&read(&outcome.store, A, 0), &[5]);
        only(&read(&outcome.store, A, 1), &[3]);
        only(&outcome.store.read_balance(address(A)), &[1_000_000]);
        only(&outcome.store.read_balance(address(U)), &[1_000_000]);
        assert!(outcome.store.possible_logs().is_empty());
        assert!(!outcome.store.logs_unknown());
    }
}
