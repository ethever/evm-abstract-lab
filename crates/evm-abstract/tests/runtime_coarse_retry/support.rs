use crate::native;
use alloy_primitives::{Address, U256, hex};
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;
use evm_abstract::domain::{NumericValue, ProductAlgebra};
use evm_abstract::{
    Fork,
    analysis::{
        EvmProcedureSummary, EvmProcedureSummaryOutput, EvmWorldAnalysis, ExecutionConfig,
        FrameCode, OutcomeKind, ProcedureSummaryKind,
    },
    domain::{EvmValue, ValueAlgebra},
    world::{Code, EvmAccount, EvmByteArray, EvmEntry, EvmWorld, Existence},
};
use revm_bytecode::opcode;
use std::collections::BTreeMap;

pub(super) const U: u64 = 0x201;
pub(super) const B: u64 = 0x301;
pub(super) const USER: u64 = 0x900;
pub(super) const BALANCE: u64 = 1_000_000;
pub(super) const NODE_CAP: usize = 28;

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

pub(super) fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(value))
}

pub(super) fn policy() -> ExecutionConfig {
    let config = ExecutionConfig::default();
    assert_eq!(domain(&config).spec().relations().max_depth, 64);
    assert_eq!(config.max_work, 20_000_000);
    config
}

pub(super) fn domain(_config: &ExecutionConfig) -> ValueAlgebra<ProductAlgebra> {
    let mut limits = native::product().spec().relations();
    limits.max_nodes = NODE_CAP;
    native::product_with(native::spec(8, 4, 256).with_relations(limits.to_owned()))
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

pub(super) fn world(code: &Fixture, unknown: Option<&Fixture>) -> EvmWorld<NumericValue> {
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, "runtime-coarse-retry:offline-callback");
    let mut account =
        EvmAccount::<NumericValue>::from_hex(&hex::encode(&code.bytes), world.fork()).unwrap();
    account.balance = word(BALANCE);
    account.storage.insert(U256::from(0), word(7));
    account.storage.insert(U256::from(1), word(5));
    world.insert(address(B), account).unwrap();
    let unknown = if let Some(code) = unknown {
        let mut account =
            EvmAccount::<NumericValue>::from_hex(&hex::encode(&code.bytes), world.fork()).unwrap();
        account.balance = word(BALANCE);
        account
    } else {
        EvmAccount {
            balance: word(BALANCE),
            nonce: word(0),
            existence: Existence::Present,
            storage_unknown: false,
            ..EvmAccount::<NumericValue>::unknown()
        }
    };
    world.insert(address(U), unknown).unwrap();
    world
}

pub(super) fn entry() -> EvmEntry<NumericValue> {
    EvmEntry::<NumericValue>::concrete(
        address(U),
        address(USER),
        word(0),
        EvmByteArray::<NumericValue>::empty(),
    )
}

pub(super) fn run(code: &Fixture, config: ExecutionConfig) -> EvmWorldAnalysis<NumericValue> {
    native::run(
        &domain(&config),
        AnalysisInput::new(world(code, None), entry()).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap()
}

pub(super) fn callback(
    analysis: &EvmWorldAnalysis<NumericValue>,
    code: &Fixture,
) -> EvmProcedureSummary<NumericValue> {
    let [record] = analysis.procedure_summaries() else {
        panic!("one unknown root relation required");
    };
    assert_eq!(record.kind, ProcedureSummaryKind::UnknownEnvironment);
    assert_eq!(record.closed, analysis.frontiers().is_empty());
    let report = record.materialize(
        domain(analysis.config()).borrowed(),
        &mut native::postprocessing(),
    );
    assert_eq!(
        report.outcome,
        evm_abstract::analysis::ProcedureReportOutcome::Complete
    );
    let mut callbacks: Vec<_> = report
        .procedures
        .into_iter()
        .filter(|procedure| {
            procedure.frame.address == address(B) && procedure.frame.code_address == address(B)
        })
        .collect();
    assert_eq!(callbacks.len(), 1);
    let callback = callbacks.pop().unwrap();
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
    procedure: &EvmProcedureSummary<NumericValue>,
    kind: OutcomeKind,
) -> &EvmProcedureSummaryOutput<NumericValue> {
    let outputs: Vec<_> = procedure
        .outputs
        .iter()
        .filter(|output| output.kind == kind)
        .collect();
    assert_eq!(outputs.len(), 1);
    outputs[0]
}

pub(super) fn bounded_current_evidence(analysis: &EvmWorldAnalysis<NumericValue>) {
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
