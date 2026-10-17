use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{
        ExecutionConfig, MachineOutcome, OutcomeKind, ProcedureSummaryKind, Status, WorldAnalysis,
        analyze_world,
    },
    domain::{AbstractValue, Domain},
    world::{Account, ByteArray, Entry, Store, World},
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

pub(super) fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
}

/// Resolve bytecode labels only; both executors retain their own EVM semantics.
#[derive(Default)]
pub(super) struct Assembly {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Assembly {
    pub(super) fn op(&mut self, op: u8) -> &mut Self {
        self.bytes.push(op);
        self
    }

    pub(super) fn push(&mut self, value: u16) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.bytes.extend(value.to_be_bytes());
        self
    }

    pub(super) fn label(&mut self, label: &'static str) -> &mut Self {
        assert!(self.labels.insert(label, self.bytes.len()).is_none());
        self.op(opcode::JUMPDEST)
    }

    pub(super) fn jump(&mut self, label: &'static str) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self.op(opcode::JUMPI)
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

    pub(super) fn finish(mut self) -> Fixture {
        for (position, label) in self.fixups {
            self.bytes[position..position + 2]
                .copy_from_slice(&u16::try_from(self.labels[&label]).unwrap().to_be_bytes());
        }
        Fixture {
            bytes: self.bytes,
            labels: self.labels,
        }
    }
}

pub(super) fn new_world() -> World {
    World::new(Fork::Osaka, "issue123:known-recursion:offline-bytecode")
}

pub(super) fn insert(world: &mut World, owner: u64, fixture: &Fixture, slots: &[(u64, u64)]) {
    let mut account = Account::from_hex(&hex::encode(&fixture.bytes), world.fork()).unwrap();
    account.balance = word(BALANCE);
    for (slot, value) in slots {
        account.storage.insert(U256::from(*slot), word(*value));
    }
    world.insert(address(owner), account).unwrap();
}

pub(super) fn input(n: u64) -> ByteArray {
    ByteArray::exact(&U256::from(n).to_be_bytes::<32>())
}

pub(super) fn unknown_input() -> ByteArray {
    ByteArray::unknown()
}

pub(super) fn entry(calldata: ByteArray) -> Entry {
    Entry::concrete(address(A), address(USER), word(0), calldata)
}

pub(super) fn run(world: World, entry: Entry) -> WorldAnalysis {
    run_config(world, entry, ExecutionConfig::default())
}

pub(super) fn run_config(world: World, entry: Entry, config: ExecutionConfig) -> WorldAnalysis {
    let started = Instant::now();
    let analysis = analyze_world(world, entry, config).unwrap();
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

pub(super) fn complete(analysis: &WorldAnalysis) {
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    assert!(
        !analysis.outcomes().is_empty(),
        "empty results do not prove recursion closure"
    );
}

pub(super) fn complete_recursion(analysis: &WorldAnalysis) {
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

pub(super) fn returns(analysis: &WorldAnalysis) -> Vec<&MachineOutcome> {
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
pub(super) fn terminals(analysis: &WorldAnalysis, kind: OutcomeKind) -> Vec<&MachineOutcome> {
    assert!(matches!(kind, OutcomeKind::Return | OutcomeKind::Revert));
    let initial = Store::new(analysis.world());
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
    finalized.finalize_transaction(Domain::from_spec(analysis.domain_spec()));
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

pub(super) fn read(store: &Store, owner: u64, slot: u64) -> AbstractValue {
    store.read(address(owner), &word(slot), Domain::default())
}

pub(super) fn only(value: &AbstractValue, values: &[u64]) {
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

pub(super) fn output_word(outcome: &MachineOutcome, offset: u64) -> AbstractValue {
    outcome.data.read_word(&word(offset), Domain::default())
}
