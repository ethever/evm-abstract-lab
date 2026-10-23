use super::{
    carrier::ExcludingWord,
    policy::{Calls, Policy},
};
use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{EvmWorldAnalysis, ExecutionConfig, Status, cooperation::Input},
    domain::{EvmValue, ValueAlgebra, WordElement},
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;

pub const OWNER: u64 = 0x101;
pub const CALLEE: u64 = 0x201;
pub fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}
pub fn word(value: u64) -> EvmValue<ExcludingWord> {
    EvmValue::constant(U256::from(value))
}

/// Keep each caller's original ledger while requiring executed terminal evidence.
pub fn config(max_work: usize) -> ExecutionConfig {
    ExecutionConfig {
        max_work,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    }
}

/// External feedback, rather than the numeric EQ shortcut, must supply the proof.
/// Retain the native exclusions: removing those is a separate production control.
pub fn feedback_algebra(calls: &Calls) -> ValueAlgebra<Policy<'_>> {
    let mut policy = Policy::new(calls);
    policy.exclusion_queries = false;
    ValueAlgebra::new(policy)
}

pub fn input_pc(input: &Input<ExcludingWord>) -> Option<usize> {
    let frame = input.payload().active();
    Some(
        frame
            .program
            .as_ref()?
            .blocks()
            .get(frame.key.basic_block_index)?
            .start_pc,
    )
}

pub fn complete<N: WordElement>(result: &EvmWorldAnalysis<N>) {
    assert_eq!(
        result.status(),
        Status::Converged,
        "work={}, states={}, frontiers={}, first={:?}",
        result.work(),
        result.states().len(),
        result.frontiers().len(),
        result.frontiers().first().map(|frontier| &frontier.reason)
    );
    assert!(result.frontiers().is_empty());
}

pub fn world(code: &[u8]) -> EvmWorld<ExcludingWord> {
    let mut world = EvmWorld::new(Fork::Osaka, "custom-exclusion-carrier:offline");
    let mut account = EvmAccount::from_hex(&hex::encode(code), Fork::Osaka).unwrap();
    account.balance = word(1_000_000);
    account.storage_unknown = false;
    account.storage.insert(
        U256::ZERO,
        EvmValue::from_numeric(ExcludingWord::except(U256::from(42))),
    );
    world.insert(address(OWNER), account).unwrap();
    world
}
pub fn entry() -> EvmEntry<ExcludingWord> {
    EvmEntry::concrete(
        address(OWNER),
        address(0x900),
        word(0),
        EvmByteArray::empty(),
    )
}

pub struct RollbackFixture {
    pub root: Vec<u8>,
    pub child: Vec<u8>,
    pub impossible: usize,
}
pub fn rollback() -> RollbackFixture {
    let mut root = vec![
        opcode::PUSH0,
        opcode::SLOAD,
        opcode::DUP1,
        opcode::PUSH1,
        1,
        opcode::SSTORE,
        opcode::DUP1,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH0,
        opcode::TSTORE,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH2,
        2,
        1,
        opcode::GAS,
        opcode::DELEGATECALL,
        opcode::POP,
    ];
    let mut branches = Vec::new();
    for load in [opcode::SLOAD, opcode::TLOAD] {
        root.extend([
            opcode::PUSH0,
            load,
            opcode::PUSH1,
            42,
            opcode::EQ,
            opcode::PUSH2,
        ]);
        branches.push(root.len());
        root.extend([0, 0, opcode::JUMPI]);
    }
    root.extend([
        opcode::PUSH1,
        1,
        opcode::PUSH1,
        64,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH1,
        64,
        opcode::RETURN,
    ]);
    let impossible = root.len();
    root.extend([opcode::JUMPDEST, opcode::INVALID]);
    for position in branches {
        root[position..position + 2].copy_from_slice(&(impossible as u16).to_be_bytes());
    }
    let child = vec![
        opcode::PUSH1,
        42,
        opcode::PUSH0,
        opcode::SSTORE,
        opcode::PUSH1,
        42,
        opcode::PUSH0,
        opcode::TSTORE,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::LOG0,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::REVERT,
    ];
    RollbackFixture {
        root,
        child,
        impossible,
    }
}
pub fn rollback_world(fixture: &RollbackFixture) -> EvmWorld<ExcludingWord> {
    let mut world = world(&fixture.root);
    let mut child = EvmAccount::from_hex(&hex::encode(&fixture.child), Fork::Osaka).unwrap();
    child.balance = word(1_000_000);
    child.storage_unknown = false;
    world.insert(address(CALLEE), child).unwrap();
    world
}
