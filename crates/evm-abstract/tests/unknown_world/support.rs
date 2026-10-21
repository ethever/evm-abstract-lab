use alloy_primitives::{Address, U256, hex, keccak256};
use evm_abstract::{
    Fork,
    analysis::{
        ExecutionConfig, MachineOutcome, OutcomeKind, Status, WorldAnalysis, analyze_world,
    },
    domain::{AbstractValue, Domain},
    world::{Account, ByteArray, Entry, Store, World},
};
use revm_bytecode::opcode;
use std::{collections::BTreeMap, time::Instant};

pub(super) const A: u64 = 0x101;
pub(super) const U: u64 = 0x201;
pub(super) const C: u64 = 0x301;
pub(super) const V: u64 = 0x401;
pub(super) const USER: u64 = 0x900;

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

pub(super) fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

/// Only PUSH operands and JUMPDEST labels are resolved here. EVM execution
/// always belongs to analyze_world or the independent revm oracle.
#[derive(Default)]
pub(super) struct Assembly {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
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

    pub(super) fn push_word(&mut self, value: U256) -> &mut Self {
        self.bytes.push(opcode::PUSH32);
        self.bytes.extend(value.to_be_bytes::<32>());
        self
    }

    pub(super) fn label(&mut self, name: &'static str) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.op(opcode::JUMPDEST)
    }

    pub(super) fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.push_label(name).op(if conditional {
            opcode::JUMPI
        } else {
            opcode::JUMP
        })
    }

    pub(super) fn push_label(&mut self, name: &'static str) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self
    }

    pub(super) fn data(&mut self, name: &'static str, bytes: &[u8]) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.bytes.extend(bytes);
        self
    }

    pub(super) fn input(&mut self, offset: u16) -> &mut Self {
        self.push(offset).op(opcode::CALLDATALOAD)
    }

    pub(super) fn store(&mut self, slot: u16, value: u16) -> &mut Self {
        self.push(value).push(slot).op(opcode::SSTORE)
    }

    pub(super) fn mstore(&mut self, offset: u16) -> &mut Self {
        self.push(offset).op(opcode::MSTORE)
    }

    pub(super) fn return_range(&mut self, offset: u16, size: u16) -> &mut Self {
        self.push(size).push(offset).op(opcode::RETURN)
    }

    pub(super) fn return_top(&mut self) -> &mut Self {
        self.mstore(0).return_range(0, 32)
    }

    pub(super) fn call(
        &mut self,
        op: u8,
        target: u16,
        value: u16,
        input: (u16, u16),
        output: (u16, u16),
    ) -> &mut Self {
        self.call_arguments(op, value, input, output)
            .push(target)
            .push_word(U256::from(1_000_000))
            .op(op)
    }

    pub(super) fn call_arguments(
        &mut self,
        op: u8,
        value: u16,
        input: (u16, u16),
        output: (u16, u16),
    ) -> &mut Self {
        self.push(output.1)
            .push(output.0)
            .push(input.1)
            .push(input.0);
        if matches!(op, opcode::CALL | opcode::CALLCODE) {
            self.push(value);
        }
        self
    }

    pub(super) fn finish(mut self) -> Fixture {
        for (offset, label) in self.fixups {
            let pc = u16::try_from(self.labels[&label]).unwrap();
            self.bytes[offset..offset + 2].copy_from_slice(&pc.to_be_bytes());
        }
        Fixture {
            bytes: self.bytes,
            labels: self.labels,
        }
    }
}

pub(super) fn new_world() -> World {
    World::new(Fork::Osaka, "issue124:unknown-world:offline-bytecode")
}

pub(super) fn insert(world: &mut World, owner: u64, code: &Fixture, storage: &[(u64, u64)]) {
    let mut account = Account::from_hex(&hex::encode(&code.bytes), world.fork()).unwrap();
    account.balance = word(1_000_000);
    for (slot, value) in storage {
        account.storage.insert(U256::from(*slot), word(*value));
    }
    world.insert(address(owner), account).unwrap();
}

pub(super) fn entry(calldata: ByteArray) -> Entry {
    Entry::concrete(address(A), address(USER), word(0), calldata)
}

pub(super) fn calldata(selector: u64) -> ByteArray {
    ByteArray::exact(&U256::from(selector).to_be_bytes::<32>())
}

/// Selected nested/cell/rollback functional fixtures need 20.15M..24.89M
/// under the complete tariffs. Their default-ledger coverage runs stay separate.
pub(super) fn callback_closure_config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    }
}

/// Check the original root relation on the real default ledger, independently
/// of funding a functional closure. A cheaper complete execution is also valid.
pub(super) fn default_callback_coverage(world: World, entry: Entry) {
    use evm_abstract::analysis::{FrontierReason, RootTerminalScope, RootTerminalTrigger};
    let config = ExecutionConfig::default();
    assert_eq!(config.max_work, 20_000_000);
    assert!(config.root_terminal_coverage);
    let analysis = run_config(world, entry, config.clone());
    assert_eq!(analysis.config().max_work, config.max_work);
    assert!(analysis.work() <= config.max_work);
    if analysis.status() == Status::Converged {
        complete_unknown(&analysis);
        return;
    }
    assert_eq!(analysis.status(), Status::Incomplete);
    let coverage = analysis
        .root_terminal_coverage()
        .expect("default work refusal retains its prepaid root relation");
    let reason = match coverage.trigger() {
        RootTerminalTrigger::Work => FrontierReason::Work,
        RootTerminalTrigger::SummaryWork => FrontierReason::SummaryWork,
        other => panic!("expected an actual shared-ledger work stop: {other:?}"),
    };
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == reason)
    );
    assert_eq!(coverage.scope(), RootTerminalScope::AllInputStates);
    assert_eq!(
        coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
        [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ]
    );
    let input = coverage.input();
    let original = &analysis.states()[0].entry;
    assert_eq!(input.entry(), analysis.entry());
    assert_eq!(input.frame(), &original.active().key);
    assert_eq!(input.program(), original.active().program.as_ref().unwrap());
    assert_eq!(input.calldata(), &original.active().calldata);
    assert_eq!(input.call_value(), &original.active().call_value);
    assert_eq!(input.saved_store(), &original.active().saved_store);
    assert_eq!(input.initial_store(), &original.store);
    assert_eq!(input.domain_spec(), analysis.domain_spec());
    assert_eq!(
        serde_json::to_value(input.config()).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
    assert_eq!(
        coverage.outcomes()[2].data().len().singleton(),
        Some(U256::ZERO)
    );
}

pub(super) fn run(world: World, entry: Entry) -> WorldAnalysis {
    run_config(world, entry, ExecutionConfig::default())
}

pub(super) fn run_config(world: World, entry: Entry, config: ExecutionConfig) -> WorldAnalysis {
    let started = Instant::now();
    let analysis = analyze_world(world, entry, config).unwrap();
    eprintln!(
        "issue124 evidence: status={:?} states={} transfers={} work={} outcomes={} summaries={} summary_hits={} elapsed_us={} frontiers={:?}",
        analysis.status(),
        analysis.states().len(),
        analysis.transfers(),
        analysis.work(),
        analysis.outcomes().len(),
        analysis.summaries().len(),
        analysis.summary_stats().hits,
        started.elapsed().as_micros(),
        analysis
            .frontiers()
            .iter()
            .map(|f| (&f.reason, f.pc))
            .collect::<Vec<_>>()
    );
    analysis
}

pub(super) fn complete(analysis: &WorldAnalysis) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "frontiers={:?}",
        frontier_causes(analysis)
    );
    assert!(
        analysis.frontiers().is_empty(),
        "{:?}",
        frontier_causes(analysis)
    );
    assert!(
        !analysis.outcomes().is_empty(),
        "closed graph must have outcomes"
    );
}

pub(super) fn complete_unknown(analysis: &WorldAnalysis) {
    complete(analysis);
    assert!(
        !analysis.procedure_summaries().is_empty(),
        "complete unknown execution has no coverage records"
    );
    for summary in analysis.procedure_summaries() {
        assert!(
            summary.closed,
            "unknown relation at source {} remains open",
            summary.source_state
        );
    }
}

pub(super) fn returns(analysis: &WorldAnalysis) -> Vec<&MachineOutcome> {
    let values: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(
        !values.is_empty(),
        "unknown execution did not return to the parent: {:?}",
        frontier_causes(analysis)
    );
    values
}

pub(super) fn read(store: &Store, owner: u64, slot: u64) -> AbstractValue {
    store.read(address(owner), &word(slot), Domain::default())
}

pub(super) fn output_word(outcome: &MachineOutcome, offset: u64) -> AbstractValue {
    outcome.data.read_word(&word(offset), Domain::default())
}

pub(super) fn only(value: &AbstractValue, allowed: &[u64]) {
    // An interval plus congruence may denote the same small set as the finite
    // component. Assert the denotation rather than one product-domain layout.
    let covered = if let Some(values) = value.constants() {
        !values.is_empty()
            && values
                .iter()
                .all(|v| allowed.iter().any(|n| *v == U256::from(*n)))
    } else {
        let (lower, upper) = value.interval().unsigned_bounds();
        if let (Ok(lower), Ok(upper)) = (u64::try_from(lower), u64::try_from(upper)) {
            upper >= lower
                && upper - lower <= 256
                && (lower..=upper).all(|n| !value.contains(U256::from(n)) || allowed.contains(&n))
                && allowed.iter().any(|n| value.contains(U256::from(*n)))
        } else {
            false
        }
    };
    assert!(
        covered,
        "expected values within {allowed:?}, got constants={:?} bounds={:?}",
        value.constants(),
        value.interval().unsigned_bounds()
    );
}

pub(super) fn reachable(analysis: &WorldAnalysis, owner: u64, pc: usize) -> bool {
    if analysis.states().iter().any(|state| {
        state.active().code_address == address(owner) && state.executed_pcs.contains(&pc)
    }) {
        return true;
    }
    let Some(program) = analysis.world().runtime(address(owner)) else {
        return false;
    };
    let Some(block) = program.blocks().iter().find(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| instruction.pc == pc)
    }) else {
        return false;
    };
    analysis.procedure_summaries().iter().any(|summary| {
        summary.procedures.iter().any(|procedure| {
            procedure.frame.code_address == address(owner)
                && procedure.frame.code_hash == keccak256(program.bytes())
                && procedure.covered_blocks.contains(&block.id)
        })
    })
}

pub(super) fn frontier_causes(
    analysis: &WorldAnalysis,
) -> Vec<(&evm_abstract::analysis::FrontierReason, Option<usize>)> {
    analysis
        .frontiers()
        .iter()
        .map(|f| (&f.reason, f.pc))
        .collect()
}
