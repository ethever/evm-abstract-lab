//! Real-table sharing, source revisions and optional report refusal controls.
use super::super::testing::{Table, initial};
use super::super::{Counters, FrontierReason, OutcomeKind, StorageReadPolicy};
use super::*;
use crate::U256;
use crate::{
    analysis::{
        ExecutionConfig, ProcedureReportOutcome,
        machine::fixtures::{address, fixture, word},
    },
    domain::{ProductAlgebra, ValueAlgebra},
};

mod opaque;

fn config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 100_000_000,
        root_terminal_coverage: false,
        ..Default::default()
    }
}

#[test]
fn unchanged_sources_share_whole_snapshots_and_growth_keeps_history_immutable() {
    let (world, entry) = fixture(&[(0x101, "60005460005260206000f3")]);
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    for use_summaries in [true, false] {
        let config = ExecutionConfig {
            use_summaries,
            ..config()
        };
        let mut table = Table::default();
        let mut counters = Counters::default();
        let mut execution_budget = WorkBudget::new(config.max_work);
        let mut records = Vec::new();
        for (source, value) in [(0, 5), (0, 5), (1, 9)] {
            let mut input = initial(&world, &entry, algebra.borrowed()).unwrap();
            input
                .store
                .write(address(0x101), &word(0), &word(value), algebra.borrowed());
            let (execution, record) = table.execute(
                source,
                &world,
                &entry,
                input,
                &config,
                algebra.borrowed(),
                &mut execution_budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            );
            assert!(record.closed, "{:?}", execution.frontiers);
            assert!(execution.frontiers.is_empty());
            let root = record.root_procedure.unwrap();
            let row = &record.procedures[root];
            assert_eq!(
                row.version.namespace,
                if use_summaries { 0 } else { source + 1 }
            );
            let mut postprocess = WorkBudget::new(100_000_000);
            let report = record.materialize(algebra.borrowed(), &mut postprocess);
            assert_eq!(report.outcome, ProcedureReportOutcome::Complete);
            let returned = report.procedures[root]
                .outputs
                .iter()
                .find(|o| o.kind == OutcomeKind::Return)
                .unwrap();
            assert!(
                returned
                    .data
                    .read_word(&word(0), algebra.borrowed())
                    .contains(U256::from(value))
            );
            records.push(record);
        }
        assert!(
            records[0]
                .procedures
                .shares_storage_with(&records[1].procedures)
        );
        assert!(
            records[0]
                .regional_cfg
                .shares_storage_with(&records[1].regional_cfg)
        );
        assert_eq!(records[0].version, records[1].version);
        let old_root = records[0].root_procedure.unwrap();
        let new_root = records[2].root_procedure.unwrap();
        assert_eq!(old_root == new_root, use_summaries);
        if use_summaries {
            assert!(
                !records[0].procedures[old_root]
                    .shares_storage_with(&records[2].procedures[new_root])
            );
            assert!(
                records[2].procedures[new_root].version.entry_revision
                    > records[0].procedures[old_root].version.entry_revision
            );
            assert!(
                records[2].procedures[new_root].version.output_revision
                    > records[0].procedures[old_root].version.output_revision
            );
        } else {
            assert!(
                records[0].procedures[old_root]
                    .shares_storage_with(&records[2].procedures[old_root])
            );
        }
        let mut postprocess = WorkBudget::new(100_000_000);
        let old = records[0].materialize(algebra.borrowed(), &mut postprocess);
        let returned = old.procedures[old_root]
            .outputs
            .iter()
            .find(|o| o.kind == OutcomeKind::Return)
            .unwrap();
        assert_eq!(
            returned
                .data
                .read_word(&word(0), algebra.borrowed())
                .singleton(),
            Some(U256::from(5))
        );
        assert!(records[2].version.generation > records[0].version.generation);
    }
}

#[test]
fn optional_refusal_is_atomic_and_cannot_spend_or_rescue_execution_work() {
    let (world, entry) = fixture(&[(0x101, "602a5f55602a5f5260205ff3")]);
    let config = config();
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut table = Table::default();
    let mut counters = Counters::default();
    let mut execution_budget = WorkBudget::new(config.max_work);
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let (_, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        algebra.borrowed(),
        &mut execution_budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed);
    let unrequested =
        crate::analysis::EvmProcedureReport::<crate::domain::NumericValue>::not_requested(
            record.version,
        );
    assert_eq!(unrequested.outcome, ProcedureReportOutcome::NotRequested);
    assert_eq!(unrequested.postprocess_work, 0);
    assert!(unrequested.procedures.is_empty());
    let before = serde_json::to_vec(&record).unwrap();
    let execution_used = execution_budget.used();
    let mut measured = WorkBudget::new(100_000_000);
    let complete = record.materialize(algebra.borrowed(), &mut measured);
    assert_eq!(complete.outcome, ProcedureReportOutcome::Complete);
    assert_eq!(complete.postprocess_work, measured.used());
    assert!(measured.used() > 1);
    let mut postprocess = WorkBudget::new(measured.used() - 1);
    let refused = record.materialize(algebra.borrowed(), &mut postprocess);
    assert!(matches!(
        refused.outcome,
        ProcedureReportOutcome::Refused(_)
    ));
    assert!(
        refused.procedures.is_empty(),
        "partial rich rows cannot escape"
    );
    assert!(postprocess.exhausted());
    assert!(refused.postprocess_work > 0);
    let used = postprocess.used();
    let again = record.materialize(algebra.borrowed(), &mut postprocess);
    assert!(matches!(again.outcome, ProcedureReportOutcome::Refused(_)));
    assert_eq!(postprocess.used(), used);
    assert_eq!(again.postprocess_work, 0);
    assert_eq!(execution_budget.used(), execution_used);
    assert!(!execution_budget.exhausted());
    assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    // Continue actual relative execution using the ORIGINAL cumulative ledger.
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let transfers = counters.transfers;
    let (execution, next) = table.execute(
        1,
        &world,
        &entry,
        input,
        &config,
        algebra.borrowed(),
        &mut execution_budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty());
    assert!(next.closed);
    assert_eq!(counters.transfers, transfers);
    assert!(execution_budget.used() > execution_used);
    assert!(next.procedures.shares_storage_with(&record.procedures));
    assert_eq!(serde_json::to_vec(&record).unwrap(), before);
}

#[test]
fn rich_cancellation_checks_both_owners_without_rewriting_closed_evidence() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = config();
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut table = Table::default();
    let mut counters = Counters::default();
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let (_, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        algebra.borrowed(),
        &mut WorkBudget::new(config.max_work),
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed);
    let before = serde_json::to_vec(&record).unwrap();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut postprocess =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000_000));
        if cancel_captured {
            captured.cancel();
        } else {
            ambient.cancel();
        }
        let rich = analysis_control::with_cancellation(&ambient, || {
            record.materialize(algebra.borrowed(), &mut postprocess)
        });
        assert_eq!(
            rich.outcome,
            ProcedureReportOutcome::Refused(FrontierReason::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert_eq!(rich.postprocess_work, 0);
        assert!(rich.procedures.is_empty());
        assert_eq!(serde_json::to_vec(&record).unwrap(), before);
    }
}

#[test]
fn minimal_snapshot_refusal_retains_the_semantic_obligation() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = config();
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut table = Table::default();
    let mut counters = Counters::default();
    let mut budget = WorkBudget::new(config.max_work);
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    solver.procedure(input).unwrap();
    solver.run();
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    // Remove only the presentation cache, retaining all native inputs/outputs.
    // A zero semantic allowance must still refuse the required minimum.
    let remaining = config.max_work - solver.budget.used();
    assert!(solver.budget.charge(remaining));
    assert!(solver.semantic_snapshot().is_err());
    assert!(solver.budget.exhausted());
    assert!(solver.table.model_snapshot.is_none());
    assert!(!solver.table.procedures[0].outputs.is_empty());
    assert!(solver.semantic_snapshot().is_err());
}
