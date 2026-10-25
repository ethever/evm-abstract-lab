use crate::native;
use alloy_primitives::{Address, U256, hex};
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{
        EvmMachineOutcome, EvmWorldAnalysis, ExecutionConfig, OutcomeKind, ProcedureSummaryKind,
        Status,
    },
    domain::EvmValue,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmStore, EvmWorld},
};
use revm_bytecode::opcode;
use std::{collections::BTreeMap, time::Instant};

pub(super) const A: u64 = 0x101;
pub(super) const B: u64 = 0x201;
pub(super) const USER: u64 = 0x900;
pub(super) const BALANCE: u64 = 1_000_000;

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

pub(super) fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(value))
}

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
}

/// Resolve bytecode labels only; both executors retain their own EVM semantics.
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

    pub(super) fn jump(&mut self, name: &'static str) -> &mut Self {
        self.raw.jump(name, true);
        self
    }

    pub(super) fn input(&mut self) -> &mut Self {
        self.push(0).op(opcode::CALLDATALOAD)
    }

    pub(super) fn decrement_input(&mut self) -> &mut Self {
        self.push(1).input().op(opcode::SUB).mstore(0)
    }

    pub(super) fn store(&mut self, slot: u16, value: u16) -> &mut Self {
        self.push(value).push(slot).op(opcode::SSTORE)
    }

    pub(super) fn tstore(&mut self, slot: u16, value: u16) -> &mut Self {
        self.push(value).push(slot).op(opcode::TSTORE)
    }

    pub(super) fn mstore(&mut self, offset: u16) -> &mut Self {
        self.push(offset).op(opcode::MSTORE)
    }

    pub(super) fn exit(&mut self, kind: u8, size: u16) -> &mut Self {
        self.push(size).push(0).op(kind)
    }

    pub(super) fn return_word(&mut self, value: u16) -> &mut Self {
        self.push(value).mstore(0).exit(opcode::RETURN, 32)
    }

    pub(super) fn call(
        &mut self,
        op: u8,
        target: u16,
        value: u16,
        input: u16,
        output: u16,
    ) -> &mut Self {
        self.push(output).push(0).push(input).push(0);
        if matches!(op, opcode::CALL | opcode::CALLCODE) {
            self.push(value);
        }
        self.push(target).op(opcode::GAS).op(op)
    }

    pub(super) fn call_address(&mut self, input: u16, output: u16) -> &mut Self {
        self.push(output)
            .push(0)
            .push(input)
            .push(0)
            .push(0)
            .op(opcode::ADDRESS)
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

pub(super) fn new_world() -> EvmWorld<NumericValue> {
    EvmWorld::<NumericValue>::new(Fork::Osaka, "issue123:known-recursion:offline-bytecode")
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
    for (slot, value) in slots {
        account.storage.insert(U256::from(*slot), word(*value));
    }
    world.insert(address(owner), account).unwrap();
}

pub(super) fn input(n: u64) -> EvmByteArray<NumericValue> {
    EvmByteArray::<NumericValue>::exact(&U256::from(n).to_be_bytes::<32>())
}

pub(super) fn unknown_input() -> EvmByteArray<NumericValue> {
    EvmByteArray::<NumericValue>::unknown()
}

pub(super) fn entry(calldata: EvmByteArray<NumericValue>) -> EvmEntry<NumericValue> {
    EvmEntry::<NumericValue>::concrete(address(A), address(USER), word(0), calldata)
}

pub(super) fn run(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
) -> EvmWorldAnalysis<NumericValue> {
    run_config(world, entry, ExecutionConfig::default())
}

pub(super) fn run_config(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
    config: ExecutionConfig,
) -> EvmWorldAnalysis<NumericValue> {
    let started = Instant::now();
    let analysis = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap();
    eprintln!(
        "known-recursion evidence: test={} status={:?} states={} transfers={} work={} outcomes={} procedures={} elapsed_us={} frontiers={:?}",
        std::thread::current().name().unwrap_or("probe"),
        analysis.status(),
        analysis.states().len(),
        analysis.transfers(),
        analysis.work(),
        analysis.outcomes().len(),
        analysis.procedure_summaries().len(),
        started.elapsed().as_micros(),
        analysis
            .frontiers()
            .iter()
            .map(|f| (&f.reason, f.pc))
            .collect::<Vec<_>>()
    );
    for record in analysis.procedure_summaries() {
        eprintln!(
            "known-recursion procedure: test={} source={} closed={} states={} transfers={} propagations={} reused={} entries={:?}",
            std::thread::current().name().unwrap_or("probe"),
            record.source_state,
            record.closed,
            record.state_count,
            record.transfers,
            record.propagations,
            record.reused,
            record
                .procedures
                .iter()
                .map(|p| (
                    p.frame.code_address,
                    p.frame.address,
                    p.frame.is_static,
                    p.input_updates,
                    p.output_updates,
                    p.outputs.len(),
                    p.covered_blocks.len()
                ))
                .collect::<Vec<_>>()
        );
    }
    analysis
}

pub(super) fn complete(analysis: &EvmWorldAnalysis<NumericValue>) {
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    assert!(
        !analysis.outcomes().is_empty(),
        "empty results do not prove recursion closure"
    );
}

pub(super) fn complete_recursion(analysis: &EvmWorldAnalysis<NumericValue>) {
    complete(analysis);
    let records = analysis.procedure_summaries();
    assert!(
        !records.is_empty(),
        "known recursive execution has no procedure evidence"
    );
    assert!(
        records
            .iter()
            .all(|record| record.kind == ProcedureSummaryKind::KnownRecursion)
    );
    for record in records {
        assert!(
            record.closed,
            "open recursive relation at {}",
            record.source_state
        );
        assert!(!record.procedures.is_empty());
        let root = record
            .root_procedure
            .expect("closed relation lacks its entry procedure");
        assert!(root < record.procedures.len());
        assert!(
            !record.procedures[root].outputs.is_empty(),
            "closed root relation has no terminal behavior"
        );
        for procedure in &record.procedures {
            assert_eq!(
                procedure.frame.mode,
                evm_abstract::analysis::FrameCode::Runtime
            );
            assert!(
                !procedure.covered_blocks.is_empty(),
                "known bytecode was not actually analyzed"
            );
        }
    }
}

pub(super) fn returns(
    analysis: &EvmWorldAnalysis<NumericValue>,
) -> Vec<&EvmMachineOutcome<NumericValue>> {
    let values: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!values.is_empty());
    values
}

/// Gas failure may precede entry or occur at an executed memory operation.
/// These fixtures must restore the whole root checkpoint in either case.
pub(super) fn terminals(
    analysis: &EvmWorldAnalysis<NumericValue>,
    kind: OutcomeKind,
) -> Vec<&EvmMachineOutcome<NumericValue>> {
    assert!(matches!(kind, OutcomeKind::Return | OutcomeKind::Revert));
    let initial = EvmStore::<NumericValue>::new(analysis.world(), native::product().borrowed());
    let checkpoint = analysis.states()[0]
        .entry
        .call_stack
        .root()
        .state
        .saved_store
        .state();
    assert_eq!(checkpoint, &initial);
    // Root completion also finalizes transaction state. These fixtures have no
    // retained RIPEMD touch; that exception cannot excuse other Store changes.
    let mut finalized = checkpoint.clone();
    finalized
        .finalize_transaction(native::product_with(analysis.domain_spec()).borrowed())
        .unwrap();
    assert_eq!(finalized, initial);
    // The engine seeds this outcome before appending executed completions.
    let entry_failure = analysis.outcomes().first().unwrap();
    assert_eq!(entry_failure.kind, OutcomeKind::Failure);
    assert_eq!(entry_failure.state, 0);
    let mut values = Vec::new();
    for outcome in analysis.outcomes() {
        if outcome.kind == kind {
            values.push(outcome);
        } else {
            assert_eq!(outcome.kind, OutcomeKind::Failure);
            let source = analysis
                .states()
                .get(outcome.state)
                .expect("failure has no source state");
            assert_eq!(source.id, outcome.state);
            assert_eq!(source.key, source.entry.key());
            only(outcome.data.len(), &[0]);
            assert_eq!(
                outcome.store, initial,
                "root failure leaked effects or changed the checkpoint"
            );
        }
    }
    assert!(
        !values.is_empty(),
        "gas failure alone is not a recursive completion"
    );
    values
}

pub(super) fn read(
    store: &EvmStore<NumericValue>,
    owner: u64,
    slot: u64,
) -> EvmValue<NumericValue> {
    store.read(address(owner), &word(slot), native::product().borrowed())
}

pub(super) fn only(value: &EvmValue<NumericValue>, values: &[u64]) {
    let (lo, hi) = value.interval().unsigned_bounds();
    let covered = if let Some(constants) = value.constants() {
        !constants.is_empty()
            && constants
                .iter()
                .all(|v| values.iter().any(|n| *v == U256::from(*n)))
    } else if let (Ok(lo), Ok(hi)) = (u64::try_from(lo), u64::try_from(hi)) {
        hi >= lo
            && hi - lo <= 256
            && (lo..=hi).all(|n| !value.contains(U256::from(n)) || values.contains(&n))
            && values.iter().any(|n| value.contains(U256::from(*n)))
    } else {
        false
    };
    assert!(
        covered,
        "expected {values:?}, got constants={:?} interval={:?}",
        value.constants(),
        value.interval().unsigned_bounds()
    );
}

pub(super) fn output_word(
    outcome: &EvmMachineOutcome<NumericValue>,
    offset: u64,
) -> EvmValue<NumericValue> {
    outcome
        .data
        .read_word(&word(offset), native::product().borrowed())
}
