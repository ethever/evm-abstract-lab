use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{
        ExecutionConfig, FrameCode, OutcomeKind, ProcedureSummary, ProcedureSummaryKind,
        ProcedureSummaryOutput, WorldAnalysis, analyze_world,
    },
    domain::{AbstractValue, Domain, DomainSpec},
    world::{Account, ByteArray, Code, Entry, Existence, World},
};
use revm_bytecode::opcode;
use std::{collections::BTreeMap, num::NonZeroUsize};

pub(super) const U: u64 = 0x201;
pub(super) const B: u64 = 0x301;
pub(super) const USER: u64 = 0x900;
pub(super) const BALANCE: u64 = 1_000_000;
pub(super) const NODE_CAP: usize = 28;

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

pub(super) fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

pub(super) fn policy() -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = NODE_CAP;
    assert_eq!(config.analysis.relations.max_depth, 64);
    assert_eq!(config.max_work, 20_000_000);
    config
}

pub(super) fn domain(config: &ExecutionConfig) -> Domain {
    let config = &config.analysis;
    Domain::from_spec(
        DomainSpec::new(
            config.domain_profile,
            NonZeroUsize::new(config.max_constants).unwrap(),
            NonZeroUsize::new(config.reduction_rounds).unwrap(),
            NonZeroUsize::new(config.max_facts).unwrap(),
        )
        .with_relations(config.relations),
    )
}

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
}

/// Resolve PUSH operands and labels; execution belongs to the two EVMs.
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

    pub(super) fn label(&mut self, name: &'static str) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.op(opcode::JUMPDEST)
    }

    pub(super) fn mark(&mut self, name: &'static str) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self
    }

    pub(super) fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self.op(if conditional {
            opcode::JUMPI
        } else {
            opcode::JUMP
        })
    }

    pub(super) fn store(&mut self, op: u8, slot: u16, value: u16) -> &mut Self {
        self.push(value).push(slot).op(op)
    }

    pub(super) fn exit(&mut self, op: u8, offset: u16) -> &mut Self {
        self.push(32).push(offset).op(op)
    }

    pub(super) fn finish(mut self) -> Fixture {
        for (offset, name) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[&name]).unwrap().to_be_bytes());
        }
        Fixture {
            bytes: self.bytes,
            labels: self.labels,
        }
    }
}

pub(super) fn world(code: &Fixture, unknown: Option<&Fixture>) -> World {
    let mut world = World::new(Fork::Osaka, "runtime-coarse-retry:offline-callback");
    let mut account = Account::from_hex(&hex::encode(&code.bytes), world.fork()).unwrap();
    account.balance = word(BALANCE);
    account.storage.insert(U256::from(0), word(7));
    account.storage.insert(U256::from(1), word(5));
    world.insert(address(B), account).unwrap();
    let unknown = if let Some(code) = unknown {
        let mut account = Account::from_hex(&hex::encode(&code.bytes), world.fork()).unwrap();
        account.balance = word(BALANCE);
        account
    } else {
        Account {
            balance: word(BALANCE),
            nonce: word(0),
            existence: Existence::Present,
            storage_unknown: false,
            ..Account::unknown()
        }
    };
    world.insert(address(U), unknown).unwrap();
    world
}

pub(super) fn entry() -> Entry {
    Entry::concrete(address(U), address(USER), word(0), ByteArray::empty())
}

pub(super) fn run(code: &Fixture, config: ExecutionConfig) -> WorldAnalysis {
    analyze_world(world(code, None), entry(), config).unwrap()
}

pub(super) fn callback<'a>(analysis: &'a WorldAnalysis, code: &Fixture) -> &'a ProcedureSummary {
    let [record] = analysis.procedure_summaries() else {
        panic!("one unknown root relation required");
    };
    assert_eq!(record.kind, ProcedureSummaryKind::UnknownEnvironment);
    assert_eq!(record.closed, analysis.frontiers().is_empty());
    let callbacks: Vec<_> = record
        .procedures
        .iter()
        .filter(|procedure| {
            procedure.frame.address == address(B) && procedure.frame.code_address == address(B)
        })
        .collect();
    assert_eq!(callbacks.len(), 1);
    let callback = callbacks[0];
    assert_eq!(callback.frame.mode, FrameCode::Runtime);
    assert!(!callback.frame.is_static);
    assert!(!callback.covered_blocks.is_empty());
    let Code::Runtime(program) = &analysis.world().account(address(B)).unwrap().code else {
        panic!("known callback lost its captured program");
    };
    assert_eq!(program.bytes(), code.bytes);
    assert!(
        callback
            .covered_blocks
            .iter()
            .all(|index| program.blocks().get(*index).is_some())
    );
    assert!(callback.covered_blocks.iter().any(|index| {
        program.blocks()[*index]
            .instructions
            .iter()
            .any(|instruction| instruction.pc == code.labels["refused"])
    }));
    assert!(record.transfers >= callback.covered_blocks.len());
    callback
}

pub(super) fn only_output(
    procedure: &ProcedureSummary,
    kind: OutcomeKind,
) -> &ProcedureSummaryOutput {
    let outputs: Vec<_> = procedure
        .outputs
        .iter()
        .filter(|output| output.kind == kind)
        .collect();
    assert_eq!(outputs.len(), 1);
    outputs[0]
}

pub(super) fn bounded_current_evidence(analysis: &WorldAnalysis) {
    assert!(analysis.work() <= analysis.config().max_work);
    assert!(analysis.transfers() <= analysis.config().analysis.max_transfers);
    assert!(analysis.states().len() <= analysis.config().analysis.max_states);
    for record in analysis.procedure_summaries() {
        assert!(record.state_count <= analysis.config().analysis.max_states);
        let state = &analysis.states()[record.source_state];
        assert_eq!(state.active().mode, FrameCode::Unknown);
        assert!(state.executed_pcs.is_empty());
        let receipt = state.execution_evidence().expect("summary source receipt");
        assert!(receipt.is_current());
        assert!(receipt.instructions().is_empty());
        assert!(receipt.instruction_values().unwrap().is_empty());
    }
}
