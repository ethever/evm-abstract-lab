//! Source-only regressions; Root owns compilation and execution of these cases.

use super::super::{Solver, Table, form, semantic_eq};
use super::*;
use crate::{
    U256,
    analysis::{
        DiagnosticKind, ExecutionConfig, FrontierReason, InstructionProgress, Limit, OutcomeKind,
        engine::Counters,
        machine::fixtures::{self, address, word},
        transfer::{self, RetentionCause, StorageReadPolicy, execute_inner},
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
        let mut input = transfer::initial(&world, &entry, domain).unwrap();
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
        let domain = self.config.domain().unwrap();
        Solver {
            table: &mut self.table,
            world: &self.world,
            entry: &self.entry,
            config: &self.config,
            domain,
            budget: &mut self.budget,
            counters: &mut self.counters,
            storage_reads: StorageReadPolicy::Abstract,
        }
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
    h.solver().accept(0, retried);
    assert_eq!(h.table.nodes.len(), 2);
    assert!(
        h.table.procedures[0]
            .nodes
            .keys()
            .any(|(key, _)| key.basic_block_index == 1)
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
    let before_pcs = first.executed_pcs.clone();
    let alternatives = first.outcomes.len();
    assert!(alternatives > 0);
    let failed = h.retry(first);
    assert!(semantic_eq(&original, &h.table.nodes[0].payload));
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Attempted
    ));
    assert_eq!(failed.payload, before_payload);
    assert_eq!(failed.instruction_progress, before_receipts);
    assert_eq!(failed.executed_pcs, before_pcs);
    assert_eq!(failed.outcomes.len(), alternatives);
    assert_eq!(failed.retention_failures, [(0, RetentionCause::State)]);
    assert_eq!(failed.frontiers.len(), 1);
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
    let stopped = h.retry(first);
    assert_eq!(h.counters.transfers, 1);
    assert_eq!(h.budget.used(), used);
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
    assert!(
        stopped
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
}
