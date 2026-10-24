//! Final source currency uses actual World evaluations and native obligations.

use super::{Counters, Engine, WorkBudget, WorldAnalysis, engine, fixtures};
use crate::{
    analysis::{
        EnginePolicies, FrontierReason, JoinBoundary, MachineError, Status, WholeStatePolicy,
        engine::kernel_failure,
    },
    domain::{ProductAlgebra, ValueAlgebra},
    test_instrumentation::Counter,
};
use analysis_kernel::{Change, SlotMeta};

// Hold final presentation certification once to inspect a live, completed World
// observation before a later native wake. The error probe models a non-budget
// input-preparation refusal, without changing the admitted semantic payload.
static HOLD: Counter<bool> = Counter::new(false);
static READ_REFUSAL: Counter<Option<usize>> = Counter::new(None);
static DEPENDENCY: Counter<Option<usize>> = Counter::new(None);
static DEPENDENCY_SLOT: Counter<Option<SlotMeta<usize>>> = Counter::new(None);

pub(in crate::analysis::engine) fn hold_certification() -> bool {
    let mut held = false;
    HOLD.update(|armed| held = std::mem::take(armed));
    held
}

pub(in crate::analysis::engine) fn world_read_probe(id: usize) -> Result<(), MachineError> {
    let mut refuse = false;
    READ_REFUSAL.update(|armed| {
        if *armed == Some(id) {
            *armed = None;
            refuse = true;
        }
    });
    if refuse {
        Err(MachineError::Shape)
    } else {
        Ok(())
    }
}

pub(in crate::analysis::engine) fn attach_dependency(id: usize) -> bool {
    let mut attach = false;
    DEPENDENCY.update(|armed| {
        if *armed == Some(id) {
            *armed = None;
            attach = true;
        }
    });
    attach
}

pub(in crate::analysis::engine) fn remember_dependency(slot: SlotMeta<usize>) {
    DEPENDENCY_SLOT.update(|pending| *pending = Some(slot));
}

fn fixture() -> (WorldAnalysis, usize) {
    let caller = "5f5f5f5f6103006207a120fa5000";
    let (world, mut entry) = fixtures::fixture(&[(0x101, caller)]);
    entry.environment.caller = crate::world::AddressInput::unknown_caller();
    entry.environment.value = crate::analysis::testing::TestValue::top();
    entry.environment.calldata = crate::analysis::testing::ByteArray::unknown();
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    let result = crate::analysis::testing::execute(
        world,
        entry,
        config,
        crate::domain::DomainSpec::default(),
    )
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let record = result.procedure_summaries().last().unwrap();
    assert!(record.closed && record.source_current);
    let source = record.source_state;
    (result, source)
}

fn rerun_source(engine: &mut Engine<'_>, source: usize) {
    engine.reset_pending(&[source]);
    engine.run();
}

fn input_version(engine: &mut Engine<'_>, source: usize) -> analysis_kernel::Version {
    let (_, mut driver) = engine.runtime.test_driver(engine.budget).unwrap();
    let version = engine
        .runtime
        .with_run(&mut driver, engine.budget, |run| {
            run.slot_version(&engine.inputs[source])
                .map_err(kernel_failure)
        })
        .unwrap();
    engine.runtime.finish(driver, engine.budget).unwrap();
    version
}

#[test]
fn final_success_certifies_only_its_latest_observation_and_not_global_closure() {
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    let history = engine.result.procedure_summaries().len();
    rerun_source(&mut engine, source);
    let records = engine.result.procedure_summaries();
    assert!(records.len() > history);
    assert!(
        records[..history]
            .iter()
            .all(|record| record.closed && !record.source_current)
    );
    let latest = records.len() - 1;
    assert!(records[latest].closed && records[latest].source_current);
    assert_eq!(engine.source_records[source], Some(latest));
    assert_eq!(
        records
            .iter()
            .filter(|record| record.source_current)
            .count(),
        1
    );
    // A source certificate is independent of a later unrelated global boundary.
    engine.frontier(None, None, FrontierReason::RecursiveNoTerminal);
    engine.result.status = Status::Incomplete;
    assert!(engine.result.procedure_summaries()[latest].source_current);
}

#[test]
fn actual_same_version_world_preparation_failure_cannot_certify_closed_history() {
    let _hold = HOLD.scope();
    let _failure = READ_REFUSAL.scope();
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    HOLD.set(true);
    rerun_source(&mut engine, source);
    let record = engine.source_records[source].unwrap();
    assert!(engine.result.procedure_summaries()[record].closed);
    let version = input_version(&mut engine, source);
    let payload = engine.result.states[source].entry.clone();
    let rows = engine.result.procedure_summaries().len();
    let transfers = engine.counters.transfers;
    engine.runtime.requeue_world(source, engine.budget).unwrap();
    READ_REFUSAL.set(Some(source));
    engine.run();
    assert_eq!(READ_REFUSAL.get(), None);
    assert_eq!(engine.result.procedure_summaries().len(), rows);
    assert_eq!(engine.counters.transfers, transfers);
    assert_eq!(engine.result.states[source].entry, payload);
    assert_eq!(input_version(&mut engine, source), version);
    assert!(!engine.budget.exhausted());
    assert_eq!(engine.source_records[source], None);
    assert!(
        !engine.result.states[source]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    assert!(engine.result.procedure_summaries()[record].closed);
    assert!(!engine.result.procedure_summaries()[record].source_current);
    assert!(
        engine
            .result
            .frontiers()
            .iter()
            .any(|frontier| { frontier.reason == FrontierReason::StateJoin(JoinBoundary::Shape) })
    );
}

#[test]
fn equal_input_version_dependency_wake_is_rejected_by_final_native_pending_check() {
    let _hold = HOLD.scope();
    let _dependency = DEPENDENCY.scope();
    let _dependency_slot = DEPENDENCY_SLOT.scope();
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    HOLD.set(true);
    DEPENDENCY.set(Some(source));
    rerun_source(&mut engine, source);
    assert_eq!(DEPENDENCY.get(), None);
    let record = engine.source_records[source].unwrap();
    let slot = DEPENDENCY_SLOT.replace(None).unwrap();
    let version = input_version(&mut engine, source);
    let (_, mut driver) = engine.runtime.test_driver(engine.budget).unwrap();
    let mut premise = 0usize;
    engine
        .runtime
        .with_run(&mut driver, engine.budget, |run| {
            run.publish_prepared(&slot, &mut premise, 1usize, Change::Changed, 1)
                .map_err(kernel_failure)
        })
        .unwrap();
    engine.runtime.finish(driver, engine.budget).unwrap();
    assert_eq!(input_version(&mut engine, source), version);
    assert!(
        engine.result.states[source]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    assert_eq!(engine.source_records[source], Some(record));
    assert!(engine.runtime.pending_world().any(|id| id == source));
    engine.certify_procedure_sources();
    assert!(engine.result.procedure_summaries()[record].closed);
    assert!(!engine.result.procedure_summaries()[record].source_current);
}

#[test]
fn input_growth_and_feedback_clear_the_observation_association() {
    let _hold = HOLD.scope();
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    HOLD.set(true);
    rerun_source(&mut engine, source);
    assert!(engine.source_records[source].is_some());
    let version = input_version(&mut engine, source);
    let mut incoming = engine.result.states[source].entry.clone();
    incoming.store.write(
        fixtures::address(0x101),
        &fixtures::word(0),
        &fixtures::word(1),
        algebra.borrowed(),
    );
    let checkpoint = incoming.store.snapshot();
    incoming.active_mut().saved_store = checkpoint;
    let (_, driver) = engine.runtime.test_driver(engine.budget).unwrap();
    engine.active = Some(driver);
    let metadata_work =
        <crate::analysis::SemanticPolicies as WholeStatePolicy<ProductAlgebra>>::metadata_work(
            &engine.policies.state,
            &engine.alternatives[source],
        );
    engine
        .admit_existing(source, &incoming, metadata_work)
        .unwrap();
    assert_eq!(engine.source_records[source], None);
    assert!(
        !engine.result.states[source]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    engine
        .runtime
        .finish(engine.active.take().unwrap(), engine.budget)
        .unwrap();
    assert!(input_version(&mut engine, source) > version);
    HOLD.set(true);
    engine.run();
    assert!(
        engine.source_records[source].is_some(),
        "new actual observation after growth"
    );
    let (_, driver) = engine.runtime.test_driver(engine.budget).unwrap();
    engine.active = Some(driver);
    engine.mark_feedback(source).unwrap();
    assert_eq!(engine.source_records[source], None);
    assert!(
        !engine.result.states[source]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    engine
        .runtime
        .finish(engine.active.take().unwrap(), engine.budget)
        .unwrap();
    engine.certify_procedure_sources();
    assert!(
        engine
            .result
            .procedure_summaries()
            .iter()
            .all(|record| !record.source_current)
    );
}

#[test]
fn popped_unbegun_world_admission_is_pending_even_after_worklist_removal() {
    use crate::analysis::{engine::TaskKind, worklist::MachineMeter};
    let _hold = HOLD.scope();
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    HOLD.set(true);
    rerun_source(&mut engine, source);
    let record = engine.source_records[source].unwrap();
    engine.runtime.requeue_world(source, engine.budget).unwrap();

    // Price only native deepest selection in an unpublished fork. Spend the
    // retained ledger down to exactly that fee, so the real Runtime.next pops
    // the World job and then refuses task-handle admission. No ledger is reset.
    let mut staged = engine.runtime.fork(engine.budget).unwrap();
    let before = engine.budget.used();
    let selected = staged
        .kernel()
        .next_deepest(&mut MachineMeter(engine.budget), |task| {
            usize::from(matches!(&task.0, TaskKind::World { node, .. } if *node == source))
        })
        .unwrap();
    assert!(selected.is_some());
    let selection_fee = engine.budget.used() - before;
    drop(staged);
    let remaining = 100_000_000 - engine.budget.used();
    assert!(engine.budget.charge(remaining - selection_fee));
    assert!(matches!(
        engine.runtime.next(false, engine.budget),
        Err(MachineError::Work)
    ));
    assert!(!engine.runtime.contains_world(&source));
    let pending: Vec<_> = engine.runtime.pending_world().collect();
    assert!(
        pending.contains(&source),
        "the popped admission remains a native obligation"
    );
    assert!(engine.active.is_none() && engine.active_node.is_none());
    assert_eq!(engine.source_records[source], Some(record));
    assert!(
        engine.result.states[source]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    let empty = vec![false; engine.result.states.len()];
    assert_eq!(engine.current_source_record(source, &empty), Some(record));
    let mut native = empty;
    for id in pending {
        native[id] = true;
    }
    assert_eq!(engine.current_source_record(source, &native), None);
    // The real final publication also remains false on the refused ledger; the
    // eligibility assertion above isolates admission from that funding guard.
    let consumed = engine.budget.used();
    engine.certify_procedure_sources();
    assert_eq!(engine.budget.used(), consumed);
    assert!(!engine.result.procedure_summaries()[record].source_current);
}

#[test]
fn replay_commits_only_small_source_metadata_and_rolls_it_back_with_the_graph() {
    use crate::analysis::engine::{EvmEngine, RegionalSummaries, SemanticPolicies};
    let _hold = HOLD.scope();
    for refuse in [false, true] {
        let (result, child, _) = super::replay_evidence::fixture();
        let mut certificate = super::replay_evidence::certificate(&result, child);
        for status in [Some(true), Some(false)] {
            let mut node = certificate.nodes[0].clone();
            node.resume_status = status;
            node.partition = status;
            certificate.nodes.push(node);
        }
        let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
        let ordinary = EnginePolicies::default();
        let regional = EnginePolicies::default().with_state(SemanticPolicies::new(
            crate::analysis::StructuralPartition,
            crate::analysis::CorrelatedJoin,
            crate::analysis::FeedbackAcceleration,
            RegionalSummaries::with_early_known_admission(),
        ));
        let mut budget = WorkBudget::new(100_000_000);
        let mut counters = Counters {
            states: result.states.len(),
            transfers: result.transfers(),
            ..Default::default()
        };
        let mut original = engine(
            result,
            child,
            &mut budget,
            &mut counters,
            &algebra,
            &ordinary,
        );
        original.reset_pending(&[child]);
        // The summary policy changes type while the concrete World keys and
        // admission metadata stay identical. Move the owned fields explicitly;
        // no report or Store cloning is needed to reassemble this test engine.
        let EvmEngine {
            result,
            domain,
            budget,
            counters,
            storage_reads,
            alternatives,
            selectors,
            ids,
            partitions,
            updates,
            feedback,
            runtime,
            cooperation,
            inputs,
            active,
            active_node,
            test_driver,
            edges,
            diagnostics,
            completed_calls,
            source_records,
            cache,
            procedures,
            reuse,
            terminal_boundary,
            policies: _,
        } = original;
        let mut engine = EvmEngine {
            policies: &regional,
            result,
            domain,
            budget,
            counters,
            storage_reads,
            alternatives,
            selectors,
            ids,
            partitions,
            updates,
            feedback,
            runtime,
            cooperation,
            inputs,
            active,
            active_node,
            test_driver,
            edges,
            diagnostics,
            completed_calls,
            source_records,
            cache,
            procedures,
            reuse,
            terminal_boundary,
        };
        HOLD.set(true);
        engine.run();
        let record = engine.source_records[child].expect("real early-known World observation");
        assert_eq!(
            engine.result.procedure_summaries()[record].source_state,
            child
        );
        assert!(engine.result.procedure_summaries()[record].closed);
        if refuse {
            engine.result.config.analysis.max_states = engine.counters.states + 1;
        }
        let sources = engine.source_records.clone();
        let snapshot = engine.result.clone();
        let reports = serde_json::to_value(snapshot.procedure_summaries()).unwrap();
        let before = engine.replay_copy_cost().unwrap();
        let retained = std::mem::take(&mut engine.result.procedure_summaries);
        assert_eq!(
            engine.replay_copy_cost().unwrap(),
            before,
            "report size is not replay copy work"
        );
        engine.result.procedure_summaries = retained;
        let imported = engine.replay(child, &certificate);
        assert!(std::ptr::eq(
            snapshot.procedure_summaries(),
            engine.result.procedure_summaries()
        ));
        assert_eq!(
            serde_json::to_value(engine.result.procedure_summaries()).unwrap(),
            reports
        );
        if refuse {
            assert!(matches!(imported, Err(FrontierReason::Budget(_))));
            assert_eq!(
                engine.source_records, sources,
                "partial staged invalidation rolled back"
            );
        } else {
            imported.unwrap();
            assert_eq!(
                engine.source_records[child], None,
                "graph import is not a region observation"
            );
            assert_eq!(engine.source_records.len(), engine.result.states.len());
        }
        drop(snapshot);
        assert!(engine.result.procedure_summaries.is_unique());
    }
}

#[test]
fn cooperative_records_are_uncertified_even_after_successful_native_completion() {
    use crate::analysis::{
        AnalysisEngine, AnalysisInput,
        control::Control,
        cooperation::{Extension, Subject},
    };
    use analysis_kernel::{Analyzer, LocalClosure};
    use std::convert::Infallible;
    struct Observe;
    impl<Cx> Analyzer<Cx> for Observe {
        type Task = Subject;
        type Error = Infallible;
        fn evaluate(&mut self, _: &Subject, _: &mut Cx) -> Result<LocalClosure, Infallible> {
            Ok(LocalClosure::Stable)
        }
    }
    let (result, _) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut extension = Extension::new(Observe);
    let current = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run(
            AnalysisInput::new(result.world.clone(), result.entry.clone()).unwrap(),
            result.config.clone(),
            &Control::default(),
        )
        .unwrap();
    assert_eq!(
        current.status(),
        Status::Converged,
        "{:?}",
        current.frontiers()
    );
    assert!(
        current
            .procedure_summaries()
            .iter()
            .any(|record| record.closed)
    );
    assert!(
        current
            .procedure_summaries()
            .iter()
            .all(|record| !record.source_current)
    );
}

#[test]
fn optional_certification_does_not_walk_or_mutate_shared_records_or_exhaust_funds() {
    let _hold = HOLD.scope();
    let (result, source) = fixture();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = EnginePolicies::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        source,
        &mut budget,
        &mut counters,
        &algebra,
        &policies,
    );
    HOLD.set(true);
    rerun_source(&mut engine, source);
    let snapshot = engine.result.clone();
    let before = engine.budget.used();
    engine.certify_procedure_sources();
    assert_eq!(engine.budget.used(), before);
    assert!(std::ptr::eq(
        snapshot.procedure_summaries(),
        engine.result.procedure_summaries()
    ));
    assert!(
        snapshot
            .procedure_summaries()
            .iter()
            .all(|record| !record.source_current)
    );
    drop(snapshot);
    // Consume the existing ledger's remaining allowance without causing a
    // refusal. Optional certification must leave it unpoisoned and flags false.
    let remaining = 100_000_000 - engine.budget.used();
    assert!(engine.budget.charge(remaining));
    engine.certify_procedure_sources();
    assert!(!engine.budget.exhausted());
    assert_eq!(engine.budget.used(), 100_000_000);
    assert!(
        engine
            .result
            .procedure_summaries()
            .iter()
            .all(|record| !record.source_current)
    );
}
