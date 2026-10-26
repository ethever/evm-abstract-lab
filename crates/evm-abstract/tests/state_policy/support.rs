use crate::native;
use alloy_primitives::{Address, U256, hex};
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::domain::{NumericValue, ProductAlgebra};
use evm_abstract::{
    Fork,
    analysis::{
        BoundedJoin, CoarseSummaries, ContextPolicy, CorrelatedJoin, EagerAcceleration,
        EnginePolicies, EvmMachineState, EvmWorldAnalysis, ExecutionConfig, FeedbackAcceleration,
        FirstResumeStatus, ForgetJumpHistory, PartitionPolicy, ProjectedSummaries,
        RecentDistinctJumps, SemanticPolicies, StatePolicy, Status, StructuralPartition,
    },
    domain::EvmValue,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use std::collections::BTreeMap;

pub(super) const ROOT: u64 = 0x101;
pub(super) const UNKNOWN: u64 = 0x201;
pub(super) const USER: u64 = 0x900;
pub(super) const BALANCE: u64 = 1_000_000;

pub(super) fn address(n: u64) -> Address {
    Address::from_word(U256::from(n).into())
}

pub(super) fn word(n: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(n))
}

pub(super) fn input(n: u64) -> EvmByteArray<NumericValue> {
    EvmByteArray::<NumericValue>::exact(&U256::from(n).to_be_bytes::<32>())
}

pub(super) fn entry(calldata: EvmByteArray<NumericValue>) -> EvmEntry<NumericValue> {
    EvmEntry::<NumericValue>::concrete(address(ROOT), address(USER), word(0), calldata)
}

pub(super) fn config() -> ExecutionConfig {
    ExecutionConfig {
        // Witnesses must be present in executed outcomes, independently of the
        // optional conservative root-terminal relation.
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    }
}

pub(super) fn run<C, P, S>(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    config: ExecutionConfig,
    policies: &EnginePolicies<C, P, S>,
) -> EvmWorldAnalysis<NumericValue>
where
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<ProductAlgebra, P::Class>,
{
    {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&native::product(), policies, &mut extension).run(
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            config,
            &Control::default(),
        )
    }
    .unwrap()
}

pub(super) fn default_run(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    config: ExecutionConfig,
) -> EvmWorldAnalysis<NumericValue> {
    run(
        world,
        entry,
        config,
        &EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        ),
    )
}

pub(super) fn alternate_run(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    config: ExecutionConfig,
) -> EvmWorldAnalysis<NumericValue> {
    run(
        world,
        entry,
        config,
        &EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
            SemanticPolicies::new(
                ForgetJumpHistory,
                BoundedJoin,
                EagerAcceleration,
                CoarseSummaries,
            ),
        ),
    )
}

pub(super) fn complete(graph: &EvmWorldAnalysis<NumericValue>) {
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(graph.frontiers().is_empty());
    for record in graph.procedure_summaries() {
        assert!(
            record.closed,
            "an unfinished procedure cannot claim closure"
        );
        let root = record.root_procedure.expect("closed relation has an entry");
        assert!(!record.procedures[root].outputs.is_empty());
    }
}

pub(super) fn starts_at(state: &EvmMachineState<NumericValue>, pc: usize) -> bool {
    state.program().is_some_and(|program| {
        program
            .blocks()
            .get(state.active().basic_block_index)
            .is_some_and(|block| block.start_pc == pc)
    })
}

pub(super) struct Fixture {
    pub(super) bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
}

/// Resolve labels only. EVM instructions and rollback belong to the executors.
#[path = "../support/assembly.rs"]
mod bytecode;

#[derive(Default)]
pub(super) struct Assembly {
    raw: bytecode::Assembly,
}

impl Assembly {
    pub(super) fn op(&mut self, op: u8) -> &mut Self {
        self.raw.op(op);
        self
    }

    pub(super) fn push(&mut self, value: u16) -> &mut Self {
        self.raw.push(value);
        self
    }

    pub(super) fn label(&mut self, name: &'static str) -> &mut Self {
        self.raw.label(name);
        self
    }

    pub(super) fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.raw.jump(name, conditional);
        self
    }

    pub(super) fn input(&mut self) -> &mut Self {
        self.push(0).op(opcode::CALLDATALOAD)
    }

    pub(super) fn mstore(&mut self, offset: u16) -> &mut Self {
        self.push(offset).op(opcode::MSTORE)
    }

    pub(super) fn store(&mut self, slot: u16, n: u16) -> &mut Self {
        self.push(n).push(slot).op(opcode::SSTORE)
    }

    pub(super) fn exit(&mut self, op: u8, size: u16) -> &mut Self {
        self.push(size).push(0).op(op)
    }

    pub(super) fn call(&mut self, target: u16, input: u16, output: u16) -> &mut Self {
        self.push(output)
            .push(0)
            .push(input)
            .push(0)
            .push(0)
            .push(target)
            .op(opcode::GAS)
            .op(opcode::CALL)
    }

    pub(super) fn finish(self) -> Fixture {
        let assembled = self.raw.finish();
        Fixture {
            bytes: assembled.bytes,
            labels: assembled.labels,
        }
    }
}

pub(super) fn insert(
    world: &mut EvmWorld<NumericValue>,
    owner: u64,
    fixture: &Fixture,
    slots: &[(u64, u64)],
) {
    let mut account =
        EvmAccount::<NumericValue>::from_hex(&hex::encode(&fixture.bytes), world.fork()).unwrap();
    account.balance = word(BALANCE);
    for &(slot, n) in slots {
        account.storage.insert(U256::from(slot), word(n));
    }
    world.insert(address(owner), account).unwrap();
}

pub(super) fn world(fixture: &Fixture) -> EvmWorld<NumericValue> {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "public state-policy witnesses");
    insert(&mut world, ROOT, fixture, &[(0, 0), (1, 77)]);
    world
}

/// Three incoming histories, but only two zero/nonzero operand classes.
/// Each path puts the same value on the stack, in memory and in storage.
pub(super) fn branches(compare_memory: bool) -> Fixture {
    let mut a = Assembly::default();
    a.input()
        .op(opcode::DUP1)
        .op(opcode::ISZERO)
        .jump("zero", true)
        .push(1)
        .op(opcode::EQ)
        .jump("one", true);
    write_tuple(&mut a, 22);
    a.jump("join", false).label("one");
    write_tuple(&mut a, 11);
    a.jump("join", false).label("zero").op(opcode::POP);
    write_tuple(&mut a, 0);
    a.jump("join", false).label("join").op(opcode::DUP1);
    if compare_memory {
        a.push(0).op(opcode::MLOAD).op(opcode::XOR);
    }
    a.mstore(32).op(opcode::POP).exit(opcode::RETURN, 64);
    a.finish()
}

fn write_tuple(a: &mut Assembly, n: u16) {
    a.push(n)
        .op(opcode::DUP1)
        .push(0)
        .op(opcode::SSTORE)
        .op(opcode::DUP1)
        .mstore(0);
}

pub(super) fn recursive_counter(growing: bool) -> Fixture {
    let mut a = Assembly::default();
    a.input().op(opcode::ISZERO).jump("base", true);
    if growing {
        for (load, store) in [
            (opcode::SLOAD, opcode::SSTORE),
            (opcode::TLOAD, opcode::TSTORE),
        ] {
            a.push(0).op(load).push(1).op(opcode::ADD).push(0).op(store);
        }
    }
    a.push(1)
        .input()
        .op(opcode::SUB)
        .mstore(0)
        .label("recursive_call")
        .call(ROOT as u16, 32, 0)
        .op(opcode::POP)
        .label("base");
    if growing {
        a.push(0)
            .op(opcode::SLOAD)
            .mstore(0)
            .push(0)
            .op(opcode::TLOAD)
            .mstore(32)
            .push(1)
            .op(opcode::SLOAD)
            .mstore(64)
            .exit(opcode::RETURN, 96);
    } else {
        a.push(42).mstore(0).exit(opcode::RETURN, 32);
    }
    a.finish()
}

pub(super) fn recursive_rollback() -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("child", true)
        .store(0, 5)
        .push(7)
        .push(0)
        .op(opcode::TSTORE)
        .input()
        .mstore(0)
        .call(ROOT as u16, 32, 0)
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(0)
        .push(0)
        .op(opcode::TLOAD)
        .mstore(32)
        .exit(opcode::RETURN, 64)
        .label("child")
        .store(0, 9)
        .push(11)
        .push(0)
        .op(opcode::TSTORE)
        .input()
        .op(opcode::ISZERO)
        .jump("child_exit", true)
        .push(1)
        .input()
        .op(opcode::SUB)
        .mstore(0)
        .label("recursive_call")
        .call(ROOT as u16, 32, 0)
        .op(opcode::POP)
        .label("child_exit")
        .push(99)
        .mstore(0)
        .exit(opcode::REVERT, 32);
    a.finish()
}

/// Successful recursive frames return the child's actual data. Failure reverts
/// instead of returning the caller's old input buffer as an unrelated witness.
pub(super) fn recursive_returning() -> Fixture {
    let mut a = Assembly::default();
    a.input()
        .op(opcode::ISZERO)
        .jump("base", true)
        .push(1)
        .input()
        .op(opcode::SUB)
        .mstore(0)
        .label("recursive_call")
        .call(ROOT as u16, 32, 32)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .exit(opcode::RETURN, 32)
        .label("base")
        .push(42)
        .mstore(0)
        .exit(opcode::RETURN, 32)
        .label("failed")
        .exit(opcode::REVERT, 0);
    a.finish()
}

pub(super) fn callback_parent(outer_revert: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .jump("run", true)
        .input()
        .op(opcode::ISZERO)
        .jump("callback_fail", true)
        .label("callback_success")
        .store(1, 11)
        .push(11)
        .push(1)
        .op(opcode::TSTORE)
        .op(opcode::STOP)
        .label("callback_fail")
        .store(1, 13)
        .push(13)
        .push(1)
        .op(opcode::TSTORE)
        .push(0x55)
        .mstore(0)
        .exit(opcode::REVERT, 32)
        .label("run")
        .store(0, 7)
        .push(7)
        .push(0)
        .op(opcode::TSTORE)
        .call(UNKNOWN as u16, 0, 0)
        .label("resumed")
        .mstore(0)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .push(1)
        .op(opcode::SLOAD)
        .mstore(64)
        .push(1)
        .op(opcode::TLOAD)
        .mstore(96)
        .exit(
            if outer_revert {
                opcode::REVERT
            } else {
                opcode::RETURN
            },
            128,
        );
    a.finish()
}

pub(super) fn callback_implementation(selector: u16, revert: bool) -> Fixture {
    let mut a = Assembly::default();
    a.push(selector)
        .mstore(0)
        .call(ROOT as u16, 32, 0)
        .op(opcode::POP)
        .exit(
            if revert {
                opcode::REVERT
            } else {
                opcode::RETURN
            },
            0,
        );
    a.finish()
}
