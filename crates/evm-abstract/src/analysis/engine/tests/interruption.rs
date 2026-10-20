//! One interrupted evaluation and its queued successor are separate obligations.

use super::{Engine, engine, interrupted_fixture};
use crate::{
    analysis::{
        EdgeKind, FrontierReason, InstructionProgress, MachineEdgeKind, OutcomeKind, Status,
        engine::{EnginePolicies, WorldWork},
        machine::fixtures,
        transfer::EvmSuccessor,
        worklist::{MachineMeter, kernel_failure},
    },
    domain::{AbstractValue, ValueAlgebra, relational::QueryReason},
    resource::WorkBudget,
};
use analysis_kernel::{Change, Closure};

#[test]
fn completed_stop_keeps_its_outcome_when_only_kernel_finish_is_unpaid() {
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    config.analysis.relations.enabled = false;
    let complete = fixtures::run("00", config.clone());
    assert_eq!(complete.status(), Status::Converged);
    // This one-block STOP has no optional cache or successor work. Its last
    // payment is kernel finish, after the real terminal was already installed.
    config.max_work = complete.work() - 1;
    let interrupted = fixtures::run("00", config);
    assert_eq!(interrupted.status(), Status::Incomplete);
    assert_eq!(interrupted.states()[0].executed_pcs, [0]);
    assert_eq!(
        interrupted.states()[0]
            .execution_evidence()
            .unwrap()
            .instructions(),
        [InstructionProgress::Dispatched]
    );
    assert!(
        interrupted
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    let [frontier] = interrupted.frontiers() else {
        panic!("the unpaid epilogue must retain its own generic boundary")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (None, None));
    assert_eq!(frontier.target.as_ref(), Some(&interrupted.states()[0].key));
}

#[derive(Clone, Copy, Debug)]
enum Wake {
    None,
    Dependency,
    Input,
}

const WORK: usize = 100_000_000;

fn admit_child(engine: &mut Engine<'_>, child: usize) -> WorldWork<crate::domain::NumericValue> {
    engine.reset_pending(&[child, 0]);
    engine
        .runtime
        .finish(engine.active.take().unwrap(), engine.budget)
        .unwrap();
    engine.test_driver = None;
    let work = engine.next_input().unwrap().unwrap();
    assert_eq!(work.id, child);
    work
}

#[test]
fn precise_gap_keeps_distinct_queued_inputs_and_self_dirty_evaluations() {
    for wake in [Wake::None, Wake::Dependency, Wake::Input] {
        for cancelled in [false, true] {
            let cancellation = analysis_control::Cancellation::new();
            analysis_control::with_cancellation(&cancellation, || {
                let (result, child) = interrupted_fixture();
                let algebra = ValueAlgebra::new(result.config.domain().unwrap());
                let policies: EnginePolicies = Default::default();
                let mut budget = WorkBudget::new(WORK);
                let mut counters = super::Counters::default();
                let mut engine = engine(
                    result,
                    child,
                    &mut budget,
                    &mut counters,
                    &algebra,
                    &policies,
                );
                let WorldWork { id, payload } = admit_child(&mut engine, child);
                let original_value = payload.active().call_value.clone();
                let mut output = 0usize;
                let scope = engine.runtime.world;
                let output_slot = engine
                    .runtime
                    .kernel()
                    .slot(scope, &mut MachineMeter(engine.budget))
                    .unwrap();
                match wake {
                    Wake::None => {}
                    Wake::Dependency => {
                        engine
                            .runtime
                            .with_run(engine.active.as_mut().unwrap(), engine.budget, |run| {
                                run.read_slot(&output_slot, &output, 1)
                                    .map_err(kernel_failure)?;
                                run.publish_prepared(
                                    &output_slot,
                                    &mut output,
                                    1,
                                    Change::Changed,
                                    1,
                                )
                                .map_err(kernel_failure)?;
                                Ok(())
                            })
                            .unwrap();
                    }
                    Wake::Input => {
                        let mut incoming = payload.clone();
                        incoming.active_mut().call_value = AbstractValue::top();
                        assert_eq!(
                            engine.successor(
                                id,
                                EvmSuccessor {
                                    payload: incoming,
                                    kind: MachineEdgeKind::Intraprocedural(EdgeKind::Fallthrough),
                                },
                                FrontierReason::Work,
                            ),
                            Some(id)
                        );
                        assert_ne!(
                            engine.result.states[id].entry.active().call_value,
                            original_value
                        );
                    }
                }
                let rerun = !matches!(wake, Wake::None);
                assert_eq!(engine.runtime.contains_world(&id), rerun);
                // Spend the remaining prefix on the SAME ledger. The real
                // transfer then refuses its next positive fee and emits its PC.
                assert!(engine.budget.charge(WORK - engine.budget.used()));
                let execution = engine.ordinary_execution(id, payload).unwrap();
                assert!(
                    execution
                        .frontiers
                        .iter()
                        .any(|f| f.reason == FrontierReason::Work)
                );
                let reported_work = execution
                    .frontiers
                    .iter()
                    .any(|frontier| frontier.reason == FrontierReason::Work);
                engine.execution(id, execution, None);
                assert!(engine.budget.exhausted());
                if cancelled {
                    cancellation.cancel();
                }
                let spent = engine.budget.used();
                let error = engine
                    .runtime
                    .finish(engine.active.take().unwrap(), engine.budget)
                    .unwrap_err();
                let reason = error.frontier_reason();
                assert_eq!(
                    reason,
                    if cancelled {
                        FrontierReason::Relations(QueryReason::Cancelled)
                    } else {
                        FrontierReason::Work
                    }
                );
                let closure = engine.runtime.view().closure();
                let queued: Vec<_> = engine.runtime.pending_world().collect();
                assert_eq!(queued.contains(&id), rerun);
                assert!(queued.contains(&0));
                engine.stop_after_execution(id, reported_work, reason.clone());
                assert_eq!(engine.runtime.pending_world().collect::<Vec<_>>(), queued);
                assert_eq!(engine.runtime.view().closure(), closure);
                assert!(matches!(closure, Closure::Open(_)));
                assert_eq!(engine.budget.used(), spent);
                assert!(engine.budget.exhausted());
                let generic: Vec<_> = engine
                    .result
                    .frontiers
                    .iter()
                    .filter(|frontier| frontier.pc.is_none())
                    .collect();
                assert_eq!(
                    generic.len(),
                    1 + usize::from(rerun) + usize::from(cancelled)
                );
                assert!(generic.iter().all(|frontier| frontier.reason == reason));
                assert!(
                    generic
                        .iter()
                        .any(|frontier| frontier.target.as_ref()
                            == Some(&engine.result.states[0].key))
                );
                assert_eq!(
                    generic
                        .iter()
                        .filter(|frontier| frontier.target.as_ref()
                            == Some(&engine.result.states[id].key))
                        .count(),
                    usize::from(rerun) + usize::from(cancelled),
                    "{wake:?}, cancelled={cancelled}"
                );
            });
        }
    }
}

#[test]
fn a_prior_precise_frontier_does_not_cover_a_later_completed_evaluation() {
    let (result, child) = interrupted_fixture();
    let algebra = ValueAlgebra::new(result.config.domain().unwrap());
    let policies: EnginePolicies = Default::default();
    let mut budget = WorkBudget::new(WORK);
    let mut counters = super::Counters::default();
    let mut engine = engine(
        result,
        child,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    let WorldWork { id, payload } = admit_child(&mut engine, child);
    engine
        .result
        .frontiers
        .push(crate::analysis::EvmMachineFrontier {
            from: Some(id),
            pc: Some(0),
            target: Some(payload.key()),
            reason: FrontierReason::Work,
            payload: None,
            range: None,
        });
    let execution = engine.ordinary_execution(id, payload).unwrap();
    assert!(execution.frontiers.is_empty());
    assert!(!execution.successors.is_empty());
    let reported_work = execution
        .frontiers
        .iter()
        .any(|f| f.reason == FrontierReason::Work);
    engine.execution(id, execution, None);
    assert!(engine.budget.charge(WORK - engine.budget.used() - 1));
    let error = engine
        .runtime
        .finish(engine.active.take().unwrap(), engine.budget)
        .unwrap_err();
    engine.stop_after_execution(id, reported_work, error.frontier_reason());
    assert!(engine.result.frontiers.iter().any(|frontier| {
        frontier.pc.is_none() && frontier.target.as_ref() == Some(&engine.result.states[id].key)
    }));
    assert!(!engine.runtime.closed());
    assert!(engine.budget.exhausted());
}
