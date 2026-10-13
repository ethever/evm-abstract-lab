//! Static envelopes include real reentry while preserving transaction effects.

use super::{oracle, support::*};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{FrameCode, OutcomeKind},
    domain::Domain,
    world::{Account, ByteArray, Existence},
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

#[test]
fn real_static_reentry_reads_known_owners_and_handles_a_failed_write() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .jump("run", true)
        .input(0)
        .push(1)
        .op(opcode::EQ)
        .jump("read", true)
        .label("write")
        .store(0, 99)
        .op(opcode::STOP);
    a.label("read").push(0).op(opcode::SLOAD).return_top();
    a.label("run")
        .store(0, 42)
        .call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 96))
        .mstore(96)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(128)
        .return_range(0, 160);
    let a = a.finish();
    let mut c = Assembly::default();
    c.label("read").push(0).op(opcode::SLOAD).return_top();
    let c = c.finish();
    let mut u = Assembly::default();
    u.push(1)
        .mstore(96)
        .call(opcode::CALL, A as u16, 0, (96, 32), (0, 32))
        .op(opcode::POP)
        .push(2)
        .mstore(96)
        .call(opcode::CALL, A as u16, 0, (96, 32), (0, 0))
        .mstore(32)
        .call(opcode::CALL, C as u16, 0, (0, 0), (64, 32))
        .op(opcode::POP)
        .return_range(0, 96);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 42)]);
    insert(&mut world, C, &c, &[(0, 5)]);
    world
        .insert(
            address(U),
            Account {
                balance: word(1_000_000),
                nonce: word(0),
                existence: Existence::Present,
                storage_unknown: false,
                ..Account::unknown()
            },
        )
        .unwrap();
    let input = entry(ByteArray::empty());
    let concrete = oracle::concrete_unknown(&world, U, &u.finish());
    let observed = oracle::execute(&concrete, &input);
    assert_eq!(observed.kind, OutcomeKind::Return);
    assert_eq!(observed.data.len(), 160);
    for (offset, expected) in [(0, 42), (32, 0), (64, 5), (96, 1), (128, 42)] {
        assert_eq!(observed.output_word(offset), U256::from(expected));
    }
    for (owner, pc) in [
        (A, a.labels["read"]),
        (A, a.labels["write"]),
        (C, c.labels["read"]),
    ] {
        assert!(observed.steps.iter().any(|step| {
            step.code == address(owner)
                && step.owner == address(owner)
                && step.caller == address(U)
                && step.pc == pc
                && step.is_static
        }));
    }
    assert!(observed.steps.iter().any(|step| {
        step.code == address(A) && step.opcode == opcode::SSTORE && step.is_static
    }));
    assert!(
        observed
            .steps
            .iter()
            .any(|step| { step.code == address(U) && step.owner == address(U) && step.is_static })
    );
    assert_eq!(observed.slot(A, 0), U256::from(42));
    assert_eq!(observed.slot(C, 0), U256::from(5));
    for owner in [A, U, C] {
        assert_eq!(observed.balances[&address(owner)], U256::from(1_000_000));
    }
    assert!(observed.logs.is_empty());
    assert!(observed.deleted.is_empty());

    let analysis = run(world, input);
    complete_unknown(&analysis);
    // Reentry is present in the concrete trace. The static effect envelope
    // covers its complete output and Store without claiming analyzed callback PCs.
    observed.covers(&analysis);
    for outcome in analysis.outcomes() {
        only(&read(&outcome.store, A, 0), &[42]);
        only(&read(&outcome.store, C, 0), &[5]);
        only(&read(&outcome.store, U, 0), &[0]);
        for owner in [A, U, C] {
            only(&outcome.store.read_balance(address(owner)), &[1_000_000]);
            only(&outcome.store.nonce(address(owner)), &[0]);
            only(
                &outcome
                    .store
                    .read_transient(address(owner), &word(0), Domain::default()),
                &[0],
            );
            assert_eq!(outcome.store.existence(address(owner)), Existence::Present);
            assert_eq!(
                outcome.store.created_in_transaction(address(owner)),
                Some(false)
            );
            assert_eq!(
                outcome.store.pending_destruction(address(owner)),
                Some(false)
            );
        }
        assert!(outcome.store.possible_logs().is_empty());
        assert!(!outcome.store.logs_unknown());
    }
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 128), &[42]);
    }
    let static_procedures: Vec<_> = analysis
        .unknown_summaries()
        .iter()
        .flat_map(|summary| &summary.procedures)
        .filter(|procedure| procedure.frame.mode == FrameCode::Unknown && procedure.frame.is_static)
        .collect();
    assert!(!static_procedures.is_empty());
    for procedure in static_procedures {
        assert_eq!(
            procedure
                .outputs
                .iter()
                .map(|output| output.kind)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                OutcomeKind::Return,
                OutcomeKind::Revert,
                OutcomeKind::Failure
            ])
        );
    }
}
