use super::{complete, native, oracle, run, support};
use alloy_primitives::U256;
use evm_abstract::analysis::OutcomeKind;
use revm_bytecode::opcode;
use std::collections::BTreeSet;
use support::{A, Assembly, U, V, address, insert, new_world, word};

#[test]
fn same_source_and_owner_keep_static_zero_value_separate_from_ordinary_call_value() {
    let mut caller = Assembly::default();
    caller
        .call(opcode::CALL, U as u16, 7, (0, 0), (0, 32))
        .op(opcode::POP)
        .call(opcode::STATICCALL, U as u16, 0, (0, 0), (32, 32))
        .op(opcode::POP)
        .return_range(0, 64);
    let mut callee = Assembly::default();
    callee.op(opcode::CALLVALUE).return_top();
    let mut world = new_world();
    insert(&mut world, A, &caller.finish(), &[]);
    insert(&mut world, U, &callee.finish(), &[]);
    let report = run(world.clone(), true);
    let procedures = &report.procedure_summaries().last().unwrap().procedures;
    assert_eq!(procedures.len(), 2);
    assert!(procedures.iter().all(|p| p.frame.code_address == address(U)
        && p.frame.address == address(U)
        && p.frame.caller == address(A).into()));
    assert_eq!(
        procedures
            .iter()
            .map(|p| p.frame.code_hash)
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
    assert_eq!(
        procedures
            .iter()
            .map(|p| p.frame.is_static)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([false, true])
    );
    let mut returns = 0;
    for outcome in report
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        returns += 1;
        assert_eq!(
            outcome
                .data
                .read_word(&word(32), native::product().borrowed())
                .singleton(),
            Some(U256::ZERO)
        );
    }
    assert!(returns > 0);
    let observed = oracle::execute(&world, &support::entry(support::calldata(0)));
    assert_eq!(observed.output_word(0), U256::from(7));
    assert_eq!(observed.output_word(32), U256::ZERO);
    observed.covers(&report);
}

#[test]
fn same_callee_source_keeps_delegate_owners_caller_and_value_binding_separate() {
    let mut a = Assembly::default();
    a.call(opcode::DELEGATECALL, U as u16, 0, (0, 0), (0, 128))
        .op(opcode::POP)
        .call(opcode::CALL, V as u16, 7, (0, 0), (128, 128))
        .op(opcode::POP)
        .return_range(0, 256);
    let mut v = Assembly::default();
    v.call(opcode::DELEGATECALL, U as u16, 0, (0, 0), (0, 128))
        .op(opcode::POP)
        .return_range(0, 128);
    let mut callee = Assembly::default();
    callee
        .op(opcode::ADDRESS)
        .mstore(0)
        .op(opcode::CALLER)
        .mstore(32)
        .op(opcode::CALLVALUE)
        .mstore(64)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(96)
        .return_range(0, 128);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[(0, 5)]);
    insert(&mut world, V, &v.finish(), &[(0, 9)]);
    insert(&mut world, U, &callee.finish(), &[(0, 13)]);
    let report = run(world.clone(), true);
    let frames = report
        .procedure_summaries()
        .iter()
        .flat_map(|r| {
            r.procedures
                .iter()
                .filter(|p| p.frame.code_address == address(U))
                .map(|p| &p.frame)
        })
        .collect::<Vec<_>>();
    let owners = frames
        .iter()
        .map(|frame| frame.address)
        .collect::<BTreeSet<_>>();
    assert_eq!(owners, BTreeSet::from([address(A), address(V)]));
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.code_hash)
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
    assert!(frames.iter().all(|frame| !frame.is_static));
    for frame in frames.iter().filter(|frame| frame.address == address(V)) {
        assert_eq!(frame.caller, address(A).into());
    }
    let observed = oracle::execute(&world, &support::entry(support::calldata(0)));
    for (offset, expected) in [(0, A), (96, 5), (128, V), (160, A), (192, 7), (224, 9)] {
        assert_eq!(observed.output_word(offset), U256::from(expected));
    }
    observed.covers(&report);
    for outcome in report
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
    {
        // Call-entry failures can skip either child. Whenever a full result is
        // returned, each owner's definite storage is still untouched.
        for (owner, value) in [(A, 5), (V, 9), (U, 13)] {
            assert_eq!(
                outcome
                    .store
                    .read(address(owner), &word(0), native::product().borrowed())
                    .singleton(),
                Some(U256::from(value))
            );
        }
    }
}

fn counter(target: u16) -> support::Fixture {
    let mut code = Assembly::default();
    code.input(0).op(opcode::ISZERO).jump("base", true);
    code.push(1)
        .input(0)
        .op(opcode::SUB)
        .mstore(0)
        .call(opcode::CALL, target, 0, (0, 32), (0, 0))
        .op(opcode::POP);
    code.label("base").push(42).return_top();
    code.finish()
}

#[test]
fn early_direct_and_mutual_recursion_keep_unknown_root_inputs_and_close() {
    for mutual in [false, true] {
        let mut world = new_world();
        insert(
            &mut world,
            A,
            &counter(if mutual { U } else { A } as u16),
            &[(0, 5)],
        );
        if mutual {
            insert(&mut world, U, &counter(A as u16), &[(0, 9)]);
        }
        let report = run(world.clone(), true);
        assert!(
            report
                .states()
                .iter()
                .all(|state| state.entry.call_stack.depth() <= 2)
        );
        assert!(report.procedure_summaries().iter().any(|record| {
            record
                .procedures
                .iter()
                .enumerate()
                .any(|(start, procedure)| {
                    let mut pending = procedure.dependencies.clone();
                    let mut visited = BTreeSet::new();
                    while let Some(next) = pending.pop() {
                        if next == start {
                            return true;
                        }
                        if visited.insert(next) {
                            pending.extend(&record.procedures[next].dependencies);
                        }
                    }
                    false
                })
        }));
        for outcome in report
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
        {
            assert_eq!(
                outcome
                    .data
                    .read_word(&word(0), native::product().borrowed())
                    .singleton(),
                Some(U256::from(42))
            );
        }
        for count in [0, 1, 3, 6] {
            let observed = oracle::execute(&world, &support::entry(support::calldata(count)));
            assert_eq!(observed.kind, OutcomeKind::Return);
            assert_eq!(observed.output_word(0), U256::from(42));
            observed.covers(&report);
        }
    }
}

#[test]
fn unknown_indirect_reentry_stays_inside_its_outer_rollback_checkpoint() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(V as u16)
        .op(opcode::EQ)
        .jump("callback", true);
    a.store(0, 7)
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .push(0)
        .op(opcode::REVERT);
    a.label("callback").store(0, 99).op(opcode::STOP);
    let a = a.finish();
    let mut v = Assembly::default();
    v.call(opcode::CALL, A as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 5)]);
    insert(&mut world, V, &v.finish(), &[]);
    assert!(!world.accounts().contains_key(&address(U)));
    let report = run(world.clone(), true);
    complete(&report);
    assert!(report.procedure_summaries().iter().any(|record| {
        record
            .procedures
            .iter()
            .any(|p| p.frame.mode == evm_abstract::analysis::FrameCode::Unknown)
    }));
    let mut reverts = 0;
    for outcome in report
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Revert)
    {
        reverts += 1;
        assert_eq!(
            outcome
                .store
                .read(address(A), &word(0), native::product().borrowed())
                .singleton(),
            Some(U256::from(5))
        );
    }
    assert!(reverts > 0);
    for reverts in [false, true] {
        let mut u = Assembly::default();
        u.call(opcode::CALL, V as u16, 0, (0, 0), (0, 0))
            .op(opcode::POP);
        if reverts {
            u.push(0).push(0).op(opcode::REVERT);
        } else {
            u.op(opcode::STOP);
        }
        let concrete = oracle::concrete_unknown(&world, U, &u.finish());
        let observed = oracle::execute(&concrete, &support::entry(support::calldata(0)));
        assert_eq!(observed.kind, OutcomeKind::Revert);
        assert_eq!(observed.slot(A, 0), U256::from(5));
        assert!(observed.steps.iter().any(|step| step.code == address(A)
            && step.caller == address(V)
            && step.pc == a.labels["callback"]));
        observed.covers(&report);
    }
}
