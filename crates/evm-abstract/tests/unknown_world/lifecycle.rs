use super::{oracle, support::*};
use alloy_primitives::{U256, hex};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    analysis::{FrameCode, FrontierReason, Status},
    world::{EvmAccount, EvmByteArray},
};
use revm_bytecode::opcode;

#[test]
fn created_owner_unknown_delegate_selfdestruct_is_covered_or_has_explicit_creation_gap() {
    let mut child = Assembly::default();
    child
        .op(opcode::CALLER)
        .push(A as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .store(0, 7)
        .call(opcode::DELEGATECALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .return_top();
    child.label("callback").op(opcode::STOP);
    let child = child.finish();
    let runtime_size = u16::try_from(child.bytes.len()).unwrap();
    let mut init = Assembly::default();
    init.push(runtime_size)
        .push_label("runtime")
        .push(0)
        .op(opcode::CODECOPY)
        .return_range(0, runtime_size)
        .data("runtime", &child.bytes);
    let init = init.finish();
    let init_size = u16::try_from(init.bytes.len()).unwrap();
    let destination = address(A).create(0);
    let mut factory = Assembly::default();
    factory
        .op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .push(init_size)
        .push_label("initcode")
        .push(0)
        .op(opcode::CODECOPY)
        .push(init_size)
        .push(0)
        .push(7)
        .op(opcode::CREATE)
        .op(opcode::DUP1)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .mstore(128)
        .call_arguments(opcode::CALL, 0, (0, 0), (32, 32))
        .push(128)
        .op(opcode::MLOAD)
        .push_word(U256::from(1_000_000))
        .op(opcode::CALL)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .push(128)
        .op(opcode::MLOAD)
        .op(opcode::EXTCODESIZE)
        .mstore(0)
        .return_range(0, 64);
    factory.label("callback").op(opcode::STOP);
    factory.label("failed").push(0).push(0).op(opcode::REVERT);
    factory.data("initcode", &init.bytes);
    let mut world = new_world();
    insert(&mut world, A, &factory.finish(), &[]);
    world
        .insert(destination, EvmAccount::<NumericValue>::absent())
        .unwrap();
    let mut u = Assembly::default();
    u.push(V as u16).op(opcode::SELFDESTRUCT);
    let concrete = oracle::concrete_unknown(&world, U, &u.finish());
    let input = entry(EvmByteArray::<NumericValue>::empty());
    let observed = oracle::execute(&concrete, &input);
    assert_eq!(observed.output_word(0), U256::from(runtime_size));
    assert_eq!(observed.output_word(32), U256::from(7));
    assert!(observed.deleted.contains(&destination));
    let removed = &observed.accounts[&destination];
    assert!(!removed.exists);
    assert_eq!(removed.nonce, 0);
    assert_eq!(removed.balance, U256::ZERO);
    assert_eq!(removed.code.as_deref(), Some([].as_slice()));
    assert_eq!(removed.storage[&U256::ZERO], U256::ZERO);
    assert!(
        observed
            .steps
            .iter()
            .any(|step| step.is_init && step.owner == destination)
    );
    assert!(observed.steps.iter().any(|step| step.code == address(U)
        && step.owner == destination
        && step.opcode == opcode::SELFDESTRUCT));
    let analysis = run(world, input);
    eprintln!(
        "issue124 lifecycle witness: covered={} status={:?} causes={:?}",
        observed.is_covered(&analysis),
        analysis.status(),
        frontier_causes(&analysis)
    );
    // Callback CREATE can have unresolved creator/nonce/collision/initcode.
    // An unsupported lifecycle must retain that typed gap, never claim closure.
    if !observed.is_covered(&analysis) {
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(
            analysis
                .frontiers()
                .iter()
                .any(|f| matches!(f.reason, FrontierReason::Creation(_))),
            "missing created-owner destruction has no lifecycle gap: {:?}",
            frontier_causes(&analysis)
        );
        return;
    }
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[u64::from(runtime_size)]);
    }
    // Relative unknown outputs precede transaction finalization. The concrete
    // SELFDESTRUCT witness must not make code disappear at a delegate return.
    for procedure in analysis
        .procedure_summaries()
        .iter()
        .flat_map(|s| &s.procedures)
        .filter(|p| p.frame.mode == FrameCode::Unknown && p.frame.address == destination)
    {
        for output in &procedure.outputs {
            assert_eq!(
                output.store.raw_account_code(destination),
                Some(child.bytes.clone()),
                "code was deleted before transaction finalization: {}",
                hex::encode(&child.bytes)
            );
        }
    }
}
