//! Procedure reports remain immutable across atomic graph import transactions.

use super::{Counters, FrontierReason, Status, WorkBudget, WorldAnalysis, engine, fixtures};
use crate::{
    analysis::{
        CallStack, Limit, OutcomeKind, RootFrame,
        summary::{Certificate, Node},
    },
    domain::relational::QueryReason,
};
use std::{collections::VecDeque, sync::Arc};

fn fixture() -> (WorldAnalysis, usize, usize) {
    // A static unknown call contributes full native procedure evidence before
    // the ordinary known callee whose graph will be imported below.
    let caller = "5f5f5f5f6103006207a120fa5060205f5f5f6102006207a120fa5000";
    let (world, entry) = fixtures::fixture(&[(0x101, caller), (0x200, "00")]);
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    let mut result = super::run_world(world, entry, config).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let record = result.procedure_summaries().first().unwrap();
    assert!(record.closed);
    let root = &record.procedures[record.root_procedure.unwrap()];
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        assert!(root.outputs.iter().any(|output| output.kind == kind));
    }
    let unknown = record.source_state;
    let child = result
        .states()
        .iter()
        .position(|state| {
            state.entry.call_stack.active_child().is_some()
                && state.active().code_address == fixtures::address(0x200)
        })
        .unwrap();
    result.config.use_summaries = true;
    (result, child, unknown)
}

fn certificate(result: &WorldAnalysis, child: usize) -> Certificate {
    let state = &result.states()[child];
    let relative = |payload: &crate::analysis::MachinePayload| {
        let mut payload = payload.clone();
        payload.call_stack = CallStack::new(RootFrame {
            state: payload.active().clone(),
        });
        payload.normalize();
        payload
    };
    let receipt = state.execution_evidence().unwrap();
    Certificate {
        record: 0,
        nodes: vec![Node {
            entry: relative(&state.entry),
            resume_status: state.resume_status,
            exit: relative(state.exit.as_ref().unwrap()),
            exit_stack: state.exit_stack.clone(),
            executed_pcs: state.executed_pcs.clone(),
            instruction_progress: receipt.instructions().to_vec(),
            instruction_values: Arc::clone(&receipt.instruction_values),
            diagnostics: Vec::new(),
            completed_calls: Vec::new(),
        }],
        edges: Vec::new(),
        terminals: Vec::new(),
    }
}

fn counters(result: &WorldAnalysis) -> Counters {
    Counters {
        states: result.states().len(),
        transfers: result.transfers(),
        ..Default::default()
    }
}

fn assert_reports_shared(left: &WorldAnalysis, right: &WorldAnalysis) {
    assert!(!left.procedure_summaries().is_empty());
    assert!(std::ptr::eq(
        left.procedure_summaries(),
        right.procedure_summaries(),
    ));
    assert_eq!(
        serde_json::to_value(left.procedure_summaries()).unwrap(),
        serde_json::to_value(right.procedure_summaries()).unwrap(),
    );
}

#[test]
fn retained_native_reports_share_clone_work_and_keep_the_serialized_array() {
    let (result, child, _) = fixture();
    let mut counters = counters(&result);
    let mut budget = WorkBudget::new(100_000_000);
    let mut engine = engine(result, child, &mut budget, &mut counters);
    let retained = std::mem::take(&mut engine.result.procedure_summaries);
    let empty_cost = engine.replay_copy_cost().unwrap();
    engine.result.procedure_summaries = retained;
    // Retaining all native input/output Stores, buffers, and closure metadata
    // must not turn replay's one owner copy into a report-size traversal.
    assert_eq!(engine.replay_copy_cost().unwrap(), empty_cost);
    let snapshot = engine.result.clone();
    assert_reports_shared(&engine.result, &snapshot);
    let native = serde_json::to_value(&snapshot).unwrap();
    let records = serde_json::to_value(snapshot.procedure_summaries()).unwrap();
    assert!(native["procedure_summaries"].is_array());
    assert_eq!(native["procedure_summaries"], records);
    let root = snapshot.procedure_summaries()[0].root_procedure.unwrap();
    let procedure = &native["procedure_summaries"][0]["procedures"][root];
    assert!(procedure["input"].is_object());
    assert!(
        procedure["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|output| { output["data"].is_object() && output["store"].is_object() })
    );
    assert_eq!(engine.budget.used(), 0);
}

#[test]
fn replay_commit_and_partial_import_refusal_release_the_evidence_writer() {
    for refuse in [false, true] {
        let (mut result, child, unknown) = fixture();
        let original_count = result.states().len();
        let mut certificate = certificate(&result, child);
        // Separate partitions allocate real temporary machine states. Refuse
        // after the first allocation to exercise rollback after partial import.
        for status in [Some(true), Some(false)] {
            let mut node = certificate.nodes[0].clone();
            node.resume_status = status;
            certificate.nodes.push(node);
        }
        if refuse {
            result.config.analysis.max_states = original_count + 1;
        }
        let mut counters = counters(&result);
        let mut budget = WorkBudget::new(100_000_000);
        let mut engine = engine(result, child, &mut budget, &mut counters);
        let snapshot = engine.result.clone();
        let reports = serde_json::to_value(snapshot.procedure_summaries()).unwrap();
        let queue = engine.queue.clone();
        let queued = engine.queued.clone();
        let ids = engine.ids.clone();
        let copy_cost = engine.replay_copy_cost().unwrap();
        let outcome = engine.replay(child, &certificate);
        assert_reports_shared(&engine.result, &snapshot);
        assert!(engine.budget.used() > copy_cost);
        if refuse {
            assert_eq!(outcome, Err(FrontierReason::Budget(Limit::States)));
            assert_eq!(engine.result.states().len(), original_count);
            assert_eq!(
                serde_json::to_value(engine.result.states()).unwrap(),
                serde_json::to_value(snapshot.states()).unwrap(),
            );
            for (state, before) in engine.result.states().iter().zip(snapshot.states()) {
                let receipt = state.execution_evidence().unwrap();
                let before = before.execution_evidence().unwrap();
                assert_eq!(receipt.is_current(), before.is_current());
                assert_eq!(receipt.instructions(), before.instructions());
                assert_eq!(receipt.instruction_values(), before.instruction_values());
                assert_eq!(receipt.successors(), before.successors());
            }
            assert_eq!(engine.queue, queue);
            assert_eq!(engine.queued, queued);
            assert_eq!(engine.ids, ids);
            assert_eq!(engine.result.summary_stats().imported_states, 0);
            assert_eq!(engine.counters.states, original_count + 1);
            let frontier = engine.result.frontiers().last().unwrap();
            assert_eq!(frontier.reason, FrontierReason::Budget(Limit::States));
            assert_eq!(
                frontier.payload.as_deref(),
                Some(&snapshot.states()[child].entry)
            );
        } else {
            outcome.unwrap();
            assert_eq!(engine.result.states().len(), original_count + 2);
            assert_eq!(engine.result.summary_stats().imported_states, 3);
        }
        // Public clones remain immutable. Once the retained test snapshot and
        // staging owner are gone, ordinary procedure execution can append again.
        drop(snapshot);
        let count = engine.result.procedure_summaries().len();
        engine.queue = VecDeque::from([unknown]);
        engine.queued.clear();
        engine.queued.insert(unknown);
        engine.run();
        assert!(engine.result.procedure_summaries().len() > count);
        assert_eq!(
            serde_json::to_value(&engine.result.procedure_summaries()[..count]).unwrap(),
            reports,
        );
        if refuse {
            assert_eq!(engine.result.status(), Status::Incomplete);
        }
    }
}

#[test]
fn replay_admission_and_cancellation_leave_retained_reports_and_inputs_untouched() {
    for cancelled in [false, true] {
        let (result, child, _) = fixture();
        let certificate = certificate(&result, child);
        let mut counters = counters(&result);
        let token = analysis_control::Cancellation::new();
        let (after, snapshot) = analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(100_000_000);
            let mut engine = engine(result, child, &mut budget, &mut counters);
            let snapshot = engine.result.clone();
            if cancelled {
                token.cancel();
            } else {
                let copy_cost = engine.replay_copy_cost().unwrap();
                *engine.budget = WorkBudget::new(copy_cost - 1);
            }
            let reason = engine.replay(child, &certificate).unwrap_err();
            assert_eq!(
                reason,
                if cancelled {
                    FrontierReason::Relations(QueryReason::Cancelled)
                } else {
                    FrontierReason::SummaryWork
                },
            );
            assert_eq!(engine.result.summary_stats().imported_states, 0);
            assert_eq!(engine.budget.used(), 0);
            assert_eq!(engine.result.frontiers().last().unwrap().reason, reason);
            (engine.result, snapshot)
        });
        assert_reports_shared(&after, &snapshot);
        assert_eq!(
            serde_json::to_value(after.states()).unwrap(),
            serde_json::to_value(snapshot.states()).unwrap(),
        );
    }
}
