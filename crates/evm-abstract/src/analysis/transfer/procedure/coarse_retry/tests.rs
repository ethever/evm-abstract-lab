//! Source-only regressions; Root owns compilation and execution of these cases.

use super::super::{Solver, Table, execute_inner, form, instantiate, semantic_eq};

mod before_body;
mod controls;
mod snapshots;
use super::*;
use crate::{
    U256,
    analysis::{
        DiagnosticKind, ExecutionConfig, FrontierReason, InstructionProgress, Limit, OutcomeKind,
        engine::Counters,
        machine::fixtures::{self, address, word},
        transfer::{self, RetentionCause, StorageReadPolicy},
    },
    world::{Entry, World},
};

struct Harness {
    world: World,
    entry: Entry,
    config: ExecutionConfig,
    table: Table,
    budget: WorkBudget,
    counters: Counters,
}

impl Harness {
    fn new(code: &str) -> Self {
        let (world, entry) = fixtures::fixture(&[(0x101, code)]);
        let mut config = fixtures::config();
        let domain = config.domain().unwrap();
        let mut input =
            crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
        let byte = domain.join(&word(1), &word(2));
        input
            .active_mut()
            .memory
            .write_byte(&word(0), &byte, 4096, domain)
            .unwrap();
        input.active_mut().stack = vec![word(7), word(0)];
        input.normalize();
        let summary = form(&input, &entry, domain).unwrap();
        let live = instantiate(&summary, domain).unwrap();
        // The original root fits, but WriteWord adds previous + offset + value
        // + constructor. The explicitly coarse root drops the observed cells.
        config.analysis.relations.max_nodes = live.active().memory.state_root().nodes() + 2;
        let mut harness = Self {
            world,
            entry,
            budget: WorkBudget::new(config.max_work),
            config,
            table: Table::default(),
            counters: Counters::default(),
        };
        let procedure = harness.solver().procedure(input).unwrap();
        assert_eq!(procedure, 0);
        harness
    }

    fn solver(&mut self) -> Solver<'_> {
        self.table.solver(
            &self.world,
            &self.entry,
            &self.config,
            &mut self.budget,
            &mut self.counters,
            StorageReadPolicy::Abstract,
        )
    }

    fn first(&mut self) -> Execution {
        let domain = self.config.domain().unwrap();
        let input = instantiate(&self.table.nodes[0].payload, domain).unwrap();
        self.counters.transfers += 1;
        execute_inner(
            &self.world,
            &self.entry,
            input,
            &self.config,
            domain,
            &mut self.budget,
            StorageReadPolicy::Abstract,
            true,
        )
    }

    fn retry(&mut self, first: Execution) -> Execution {
        let original = self.table.nodes[0].payload.clone();
        self.solver().retry_runtime(0, &original, first)
    }

    fn preparation_work(&mut self) -> usize {
        let original = self.table.nodes[0].payload.clone();
        // Quote the actual, detached production stage on an independent ledger.
        // No retry bit, transfer count, source slot or retained bound is changed.
        let live_budget =
            std::mem::replace(&mut self.budget, WorkBudget::new(self.config.max_work));
        self.solver().prepare_runtime_retry(0, &original).unwrap();
        let work = self.budget.used();
        self.budget = live_budget;
        work
    }
}

#[test]
fn actual_runtime_mstore_cap_retries_to_a_known_successor_once() {
    let mut h = Harness::new("525b00");
    let original = h.table.nodes[0].payload.clone();
    let first = h.first();
    assert_eq!(first.retention_failures, [(0, RetentionCause::State)]);
    assert_eq!(
        first.instruction_progress,
        [InstructionProgress::OperandsConsumed]
    );
    assert!(first.successors.is_empty());
    assert!(
        first
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    let spent = h.budget.used();
    let retried = h.retry(first);
    assert!(retried.frontiers.is_empty(), "{:?}", retried.frontiers);
    assert_eq!(retried.successors.len(), 1);
    assert_eq!(
        retried.successors[0].payload.active().key.basic_block_index,
        1
    );
    assert_eq!(
        retried.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert!(retried.retention_failures.is_empty());
    assert_eq!(h.counters.transfers, 2);
    assert!(h.budget.used() > spent && !h.budget.exhausted());
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Coarse(_)
    ));
    assert!(!semantic_eq(&original, &h.table.nodes[0].payload));
    assert!(retried.diagnostics.contains(&(
        0,
        DiagnosticKind::RelationalPrecisionLost(
            crate::domain::relational::QueryReason::ExpressionLimit
        )
    )));
    let post = &retried.successors[0].payload;
    let crate::domain::symbolic::ByteStateKind::WriteWord {
        previous, value, ..
    } = post.active().memory.state_root().kind()
    else {
        panic!("must preserve the coarse input behind the actual write")
    };
    // A numerically exact operand may have an independent Fresh expression.
    // Read back its retained numeric facts without the original constructor cap
    // also limiting this additional inspection of the completed write.
    let written = post
        .active()
        .memory
        .read_word_checked(&word(0), Domain::default())
        .unwrap();
    assert_eq!(written.singleton(), Some(U256::from(7)));
    assert_eq!(written.expression(), Some(value));
    let before_byte = crate::domain::symbolic::ExprId::byte_read(
        previous.clone(),
        crate::domain::symbolic::ExprId::constant(U256::ZERO),
        Default::default(),
    )
    .unwrap();
    let domain = h.config.domain().unwrap();
    let current = post
        .store
        .read_checked(address(0x101), &word(0), domain)
        .unwrap();
    let saved = post
        .active()
        .saved_store
        .state()
        .read_checked(address(0x101), &word(0), domain)
        .unwrap();
    // The entire original finite input has two joint tuples. Use one source
    // assignment per tuple for the pre-write byte, post-write word and both
    // current/saved storage versions; no collection of marginal SAT checks.
    for byte in [1u64, 2] {
        let mut witness = snapshots::Witness::default();
        witness.assign(&before_byte, U256::from(byte));
        witness.assign(value, U256::from(7));
        witness.assign(current.expression().unwrap(), U256::ZERO);
        witness.assign(saved.expression().unwrap(), U256::ZERO);
    }
    h.solver().accept(0, retried);
    assert_eq!(h.table.nodes.len(), 2);
    assert!(h.table.procedures[0].nodes.keys().any(|(key, _)| {
        key.frames
            .last()
            .is_some_and(|frame| frame.basic_block_index == 1)
    }));
}

#[test]
fn real_worklist_counts_both_attempts_and_only_actually_covered_blocks() {
    let mut h = Harness::new("525b00");
    h.solver().run();
    assert!(h.table.frontiers.is_empty(), "{:?}", h.table.frontiers);
    assert_eq!(h.counters.transfers, 3);
    assert_eq!(h.table.nodes.len(), 2);
    assert_eq!(
        h.table.procedures[0]
            .covered_blocks
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(
        h.table.procedures[0]
            .outputs
            .contains_key(&OutcomeKind::Return)
    );
    assert!(h.table.runtime.relative_closed(&mut h.budget).unwrap());
    assert_eq!(
        h.table
            .nodes
            .iter()
            .filter(|node| matches!(&node.runtime_retry, RuntimeRetry::Coarse(_)))
            .count(),
        1
    );
}

#[test]
fn failed_coarse_transfer_keeps_original_input_receipts_and_failure_alternative() {
    let mut code = String::from("52");
    for offset in 1..=64u16 {
        code.push_str(&format!("600761{:04x}52", offset * 32));
    }
    code.push_str("5b00");
    let mut h = Harness::new(&code);
    let original = h.table.nodes[0].payload.clone();
    let first = h.first();
    assert_eq!(first.retention_failures, [(0, RetentionCause::State)]);
    let before_payload = first.payload.clone();
    let before_receipts = first.instruction_progress.clone();
    let before_values = first.instruction_values.clone();
    let before_pcs = first.executed_pcs.clone();
    let alternatives = first
        .outcomes
        .iter()
        .map(|outcome| (outcome.kind, outcome.data.clone(), outcome.store.clone()))
        .collect::<Vec<_>>();
    assert!(!alternatives.is_empty());
    let original_frontier = &first.frontiers[0];
    let original_boundary = (
        original_frontier.pc,
        original_frontier.reason.clone(),
        original_frontier.target.clone(),
        original_frontier.range.clone(),
        original_frontier.payload.clone(),
    );
    let failed = h.retry(first);
    assert!(semantic_eq(&original, &h.table.nodes[0].payload));
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Attempted
    ));
    assert_eq!(failed.payload, before_payload);
    assert_eq!(failed.instruction_progress, before_receipts);
    assert_eq!(failed.instruction_values, before_values);
    assert_eq!(failed.executed_pcs, before_pcs);
    assert_eq!(
        failed
            .outcomes
            .iter()
            .map(|outcome| (outcome.kind, outcome.data.clone(), outcome.store.clone()))
            .collect::<Vec<_>>(),
        alternatives
    );
    assert_eq!(failed.retention_failures, [(0, RetentionCause::State)]);
    assert_eq!(failed.frontiers.len(), 1);
    assert_eq!(
        (
            failed.frontiers[0].pc,
            failed.frontiers[0].reason.clone(),
            failed.frontiers[0].target.clone(),
            failed.frontiers[0].range.clone(),
            failed.frontiers[0].payload.clone(),
        ),
        original_boundary
    );
    assert_eq!(h.counters.transfers, 2);
    let again = h.retry(failed);
    assert_eq!(h.counters.transfers, 2);
    assert_eq!(again.frontiers.len(), 1);
}

#[test]
fn max_transfers_keeps_first_result_and_does_not_run_or_install_a_retry() {
    let mut h = Harness::new("525b00");
    let original = h.table.nodes[0].payload.clone();
    let first = h.first();
    h.config.analysis.max_transfers = h.counters.transfers;
    let used = h.budget.used();
    let before_payload = first.payload.clone();
    let before_receipts = first.instruction_progress.clone();
    let before_values = first.instruction_values.clone();
    let before_pcs = first.executed_pcs.clone();
    let stopped = h.retry(first);
    assert_eq!(h.counters.transfers, 1);
    // Retaining the rejected retry obligation can pay to reify the original
    // checkpoint even though transfer admission refuses before preparation.
    assert!(h.budget.used() >= used && !h.budget.exhausted());
    assert_eq!(stopped.payload, before_payload);
    assert_eq!(stopped.instruction_progress, before_receipts);
    assert_eq!(stopped.instruction_values, before_values);
    assert_eq!(stopped.executed_pcs, before_pcs);
    assert!(semantic_eq(&original, &h.table.nodes[0].payload));
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Attempted
    ));
    assert_eq!(stopped.frontiers.len(), 2);
    assert_eq!(
        stopped.frontiers[1].reason,
        FrontierReason::Budget(Limit::Transfers)
    );
    assert_eq!(
        stopped.frontiers[1].target.as_ref(),
        Some(&original.location)
    );
    let checkpoint = stopped.frontiers[1].payload.as_ref().unwrap();
    let retained = form(checkpoint, &h.entry, h.config.domain().unwrap()).unwrap();
    assert!(semantic_eq(&original, &retained));
    assert!(
        stopped
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
}
