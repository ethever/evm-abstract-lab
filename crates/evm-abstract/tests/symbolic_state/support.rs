use crate::native;
use alloy_primitives::{Address, U256, hex};
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{Config, EvmWorldAnalysis, ExecutionConfig, OutcomeKind, Status},
    domain::{
        EvmValue,
        relational::{CheckResult, EvmValueQuery, RelationLimits, RelationState},
    },
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmEnvironmentValues, EvmWorld},
};
use revm_bytecode::opcode;
use std::{collections::BTreeMap, time::Instant};

pub(super) const ROOT: u64 = 0x101;
pub(super) const USER: u64 = 0x900;

pub(super) fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

pub(super) fn word(number: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(number))
}

/// Small assembler: labels resolve only PUSH2 jump operands, not EVM semantics.
#[path = "../support/assembly.rs"]
mod bytecode;

#[derive(Default)]
pub(super) struct Assembly {
    raw: bytecode::Assembly,
}

pub(super) struct Fixture {
    pub(super) code: String,
    pub(super) labels: BTreeMap<&'static str, usize>,
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

    pub(super) fn push_word(&mut self, value: U256) -> &mut Self {
        self.raw.push_word(value);
        self
    }

    pub(super) fn load_input(&mut self, offset: u16) -> &mut Self {
        self.push(offset).op(opcode::CALLDATALOAD)
    }

    pub(super) fn label(&mut self, name: &'static str) -> &mut Self {
        self.raw.label(name);
        self
    }

    pub(super) fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.raw.jump(name, conditional);
        self
    }

    pub(super) fn return_top(&mut self) -> &mut Self {
        self.op(opcode::PUSH0)
            .op(opcode::MSTORE)
            .push(32)
            .op(opcode::PUSH0)
            .op(opcode::RETURN)
    }

    pub(super) fn return_word(&mut self, value: u16) -> &mut Self {
        self.push(value).return_top()
    }

    /// Check a word in the EVM itself while retaining it for the final return.
    pub(super) fn require_top(&mut self, expected: u16) -> &mut Self {
        self.op(opcode::DUP1)
            .push(expected)
            .op(opcode::EQ)
            .op(opcode::ISZERO)
            .jump("bad", true)
    }

    pub(super) fn call(
        &mut self,
        op: u8,
        target: u16,
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
        self.push(target).push_word(U256::from(1_000_000)).op(op)
    }

    pub(super) fn finish(self) -> Fixture {
        let assembled = self.raw.finish();
        Fixture {
            code: hex::encode(assembled.bytes),
            labels: assembled.labels,
        }
    }
}

pub(super) fn numeric_spec() -> evm_abstract::domain::DomainSpec {
    native::spec(8, 4, 256).with_relations(RelationLimits {
        max_nodes: 8192,
        max_depth: 128,
        max_constraints: 256,
        ..RelationLimits::default()
    })
}

pub(super) fn config() -> ExecutionConfig {
    ExecutionConfig {
        analysis: Config {
            context_depth: 0,
            max_states: 512,
            max_transfers: 4096,
        },
        max_memory_bytes: 4096,
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    }
}

pub(super) fn construct_world(fork: Fork, accounts: &[(u64, &str)]) -> EvmWorld<NumericValue> {
    let mut world = EvmWorld::<NumericValue>::new(fork, "issue111:offline-behavior-acceptance");
    for (number, code) in accounts {
        let mut account = EvmAccount::<NumericValue>::from_hex(code, fork).unwrap();
        account.balance = word(1_000_000);
        world.insert(address(*number), account).unwrap();
    }
    world
}

pub(super) fn entry(calldata: EvmByteArray<NumericValue>) -> EvmEntry<NumericValue> {
    EvmEntry {
        address: address(ROOT),
        environment: EvmEnvironmentValues {
            to: address(ROOT).into(),
            caller: address(USER).into(),
            value: word(0),
            calldata,
            is_static: false,
            ..EvmEnvironmentValues::<NumericValue>::default()
        },
    }
}

pub(super) fn run_world(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
    config: ExecutionConfig,
) -> EvmWorldAnalysis<NumericValue> {
    let policy = config.clone();
    let started = Instant::now();
    let analysis = native::run(
        &native::product_with(numeric_spec()),
        AnalysisInput::new(world, entry).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap();
    let max_memory_work = analysis
        .states()
        .iter()
        .flat_map(|state| std::iter::once(&state.entry).chain(state.exit.iter()))
        .map(|payload| payload.active().memory.work_size())
        .max()
        .unwrap_or(0);
    let max_expression_nodes = analysis
        .states()
        .iter()
        .flat_map(|state| {
            state
                .entry
                .active()
                .stack
                .iter()
                .chain(state.exit_stack.iter())
        })
        .filter_map(EvmValue::<NumericValue>::expression)
        .map(|expression| expression.nodes())
        .max()
        .unwrap_or(0);
    let max_relation_work = analysis
        .states()
        .iter()
        .flat_map(|state| std::iter::once(&state.entry).chain(state.exit.iter()))
        .map(|payload| payload.relations.work_size())
        .max()
        .unwrap_or(0);
    let max_store_work = analysis
        .states()
        .iter()
        .flat_map(|state| std::iter::once(&state.entry).chain(state.exit.iter()))
        .map(|payload| payload.store.work_size())
        .max()
        .unwrap_or(0);
    eprintln!(
        "issue111 evidence: fork={} status={:?} states={} transfers={} work={} max_memory_work={} max_stack_expr_nodes={} max_relation_work={} max_store_work={} summary_hits={} summary_misses={} elapsed_us={} node_limit={} depth_limit={} constraint_limit={} memory_limit={} work_limit={} state_limit={} transfer_limit={} summaries={} frontiers={:?}",
        analysis.world().fork(),
        analysis.status(),
        analysis.states().len(),
        analysis.transfers(),
        analysis.work(),
        max_memory_work,
        max_expression_nodes,
        max_relation_work,
        max_store_work,
        analysis.summary_stats().hits,
        analysis.summary_stats().misses,
        started.elapsed().as_micros(),
        analysis.domain_spec().relations().max_nodes,
        analysis.domain_spec().relations().max_depth,
        analysis.domain_spec().relations().max_constraints,
        policy.max_memory_bytes,
        policy.max_work,
        policy.analysis.max_states,
        policy.analysis.max_transfers,
        policy.use_summaries,
        analysis
            .frontiers()
            .iter()
            .map(|frontier| (&frontier.reason, frontier.from, frontier.pc))
            .collect::<Vec<_>>(),
    );
    analysis
}

pub(super) fn run(code: &str) -> EvmWorldAnalysis<NumericValue> {
    run_world(
        construct_world(Fork::Osaka, &[(ROOT, code)]),
        entry(EvmByteArray::<NumericValue>::unknown()),
        config(),
    )
}

pub(super) fn complete(analysis: &EvmWorldAnalysis<NumericValue>) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "frontiers={:?}",
        analysis.frontiers()
    );
    assert!(
        analysis.frontiers().is_empty(),
        "{:?}",
        analysis.frontiers()
    );
    assert!(!analysis.states().is_empty());
    assert!(!analysis.outcomes().is_empty());
}

pub(super) fn reachable(analysis: &EvmWorldAnalysis<NumericValue>, owner: u64, pc: usize) -> bool {
    analysis.states().iter().any(|state| {
        state.active().code_address == address(owner) && state.executed_pcs.contains(&pc)
    })
}

/// Labels immediately before effects make the actual opcode, not just a nearby
/// JUMPDEST, part of the execution witness.
pub(super) fn executed_opcode(
    analysis: &EvmWorldAnalysis<NumericValue>,
    fixture: &Fixture,
    label: &str,
    opcode: u8,
) -> usize {
    let pc = fixture.labels[label] + 1;
    assert_eq!(hex::decode(&fixture.code).unwrap()[pc], opcode);
    assert!(reachable(analysis, ROOT, pc), "unexecuted {label} at {pc}");
    pc
}

/// Keep each boundary sample in the numeric bound and check that the retained
/// relation does not locally exclude it. This does not claim a satisfying witness.
pub(super) fn possible_lengths(
    relations: &RelationState,
    length: &EvmValue<NumericValue>,
    samples: &[u64],
) {
    assert!(length.singleton().is_none());
    let expression = length.expression().expect("symbolic input length");
    let projected = relations.local_value(
        expression,
        &numeric_spec().relations(),
        native::product_with(native::spec(8, 4, 256).with_relations(numeric_spec().relations()))
            .borrowed(),
    );
    let EvmValueQuery::<NumericValue>::Bounds(bounds) = projected else {
        panic!("boundary lengths need completed local bounds, got {projected:?}");
    };
    for sample in samples {
        assert!(length.contains(U256::from(*sample)), "missing {sample}");
        assert!(
            bounds.contains(U256::from(*sample)),
            "local bounds excluded length {sample}"
        );
        assert_eq!(
            relations.can_equal(
                expression,
                U256::from(*sample),
                &numeric_spec().relations(),
                native::product_with(
                    native::spec(8, 4, 256).with_relations(numeric_spec().relations())
                )
                .borrowed()
            ),
            CheckResult::MayBeFeasible,
            "local reasoning must not exclude length {sample}"
        );
    }
}

pub(super) fn no_bad_branch(
    analysis: &EvmWorldAnalysis<NumericValue>,
    fixture: &Fixture,
    owner: u64,
) {
    assert!(
        !reachable(analysis, owner, fixture.labels["bad"]),
        "a later EVM branch failed to consume the required state relation at pc {}",
        fixture.labels["bad"],
    );
}

pub(super) fn return_words(analysis: &EvmWorldAnalysis<NumericValue>, expected: &[u64]) {
    complete(analysis);
    let outcomes: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!outcomes.is_empty(), "no successful return");
    for outcome in outcomes {
        assert_eq!(outcome.data.len().singleton(), Some(U256::from(32)));
        let value = outcome
            .data
            .read_word(&word(0), native::product().borrowed());
        assert!(
            value.constants().is_some_and(|values| !values.is_empty()
                && values
                    .iter()
                    .all(|value| { expected.iter().any(|n| *value == U256::from(*n)) })),
            "successful return must contain only {expected:?}; got {value:?}",
        );
    }
}

/// Unknown compound correlations may retain the fixture's 0xffff guard return.
/// Required normal results must still occur; arbitrary or empty returns fail.
pub(super) fn conservative_guarded_returns_include(
    analysis: &EvmWorldAnalysis<NumericValue>,
    required: &[u64],
) {
    let mut allowed = required.to_vec();
    allowed.push(0xffff);
    return_words(analysis, &allowed);
    for expected in required {
        assert!(
            analysis.outcomes().iter().any(|outcome| {
                outcome.kind == OutcomeKind::Return
                    && outcome
                        .data
                        .read_word(&word(0), native::product().borrowed())
                        .contains(U256::from(*expected))
            }),
            "conservative guard evaluation lost normal return {expected}"
        );
    }
}
