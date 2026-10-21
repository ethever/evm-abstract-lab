//! Cancellation and real exhaustion retain every queued input with its cause.

use super::{Cache, Counters, Engine, SummaryAttempt, run_world};
use crate::{
    analysis::{FrontierReason, Status, WorldAnalysis, machine::fixtures, summary::Certificate},
    domain::relational::QueryReason,
    resource::WorkBudget,
};
use std::collections::BTreeSet;
mod bounded_promotion;
mod decided_cache;
mod feedback;
mod installation;
mod interruption;
mod loop_bound;
mod replay_evidence;

#[test]
fn call_success_preserves_non_singleton_output_with_and_without_exact_reuse() {
    let caller = "5f355f5260005f5f5f5f6102006207a120f15060205f5f5f5f6102006207a120f11560295760205ff35b5f5ffd";
    let (empty, mut entry) = fixtures::fixture(&[]);
    entry.environment.calldata = crate::world::ByteArray::unknown();
    let mut world = crate::world::World::new(empty.fork(), "call result partition regression");
    let domain = crate::domain::Domain::default();
    for (address, code) in [(0x101, caller), (0x200, "5f545f5260205ff3")] {
        let mut account = crate::world::Account::from_hex(code, world.fork()).unwrap();
        account.balance = fixtures::word(1_000_000);
        account.storage.insert(
            alloy_primitives::U256::ZERO,
            domain.join(&fixtures::word(5), &fixtures::word(9)),
        );
        world.insert(fixtures::address(address), account).unwrap();
    }
    for use_summaries in [true, false] {
        let analysis = crate::analysis::analyze_world(
            world.clone(),
            entry.clone(),
            crate::analysis::ExecutionConfig {
                use_summaries,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        assert!(analysis.procedure_summaries().is_empty());
        if use_summaries {
            assert!(
                analysis.summary_stats().hits > 0,
                "{:?}",
                analysis.summary_stats()
            );
        }
        let returns = analysis
            .outcomes()
            .iter()
            .filter(|output| output.kind == crate::analysis::OutcomeKind::Return)
            .collect::<Vec<_>>();
        assert!(!returns.is_empty());
        for output in returns {
            let value = output.data.read_word(&fixtures::word(0), domain);
            assert_eq!(
                output.data.len().singleton(),
                Some(alloy_primitives::U256::from(32))
            );
            assert_eq!(
                value.constants(),
                domain
                    .join(&fixtures::word(5), &fixtures::word(9))
                    .constants()
            );
        }
        let graph = crate::ssa::build_world(&analysis).unwrap();
        graph.verify(&analysis).unwrap();
    }
}

fn interrupted_fixture() -> (WorldAnalysis, usize) {
    let (world, entry) =
        fixtures::fixture(&[(0x101, "60205f5f5f6102006207a120fa5000"), (0x200, "00")]);
    let mut result = run_world(world, entry, fixtures::config()).unwrap();
    assert_eq!(result.status(), Status::Converged);
    let child = result
        .states
        .iter()
        .position(|state| state.entry.call_stack.active_child().is_some())
        .unwrap();
    assert!(result.states.len() >= 3);
    result.config.use_summaries = true;
    (result, child)
}

fn engine<'a>(
    result: WorldAnalysis,
    child: usize,
    budget: &'a mut WorkBudget,
    counters: &'a mut Counters,
    domain: &'a crate::domain::ValueAlgebra<crate::domain::Domain>,
    policies: &'a super::EnginePolicies,
) -> Engine<'a> {
    let mut setup = WorkBudget::new(usize::MAX);
    let mut ids = super::KeyIndex::new();
    for state in &result.states {
        ids.insert_with((state.key.clone(), state.resume_status), &mut setup, |_| {
            Ok(state.id)
        })
        .unwrap();
    }
    let alternatives = result
        .states
        .iter()
        .map(|state| {
            super::WholeStateMetadata::initialize(
                &state.entry,
                &mut super::PolicyContext::new(domain.borrowed(), &result.entry, &mut setup),
            )
            .unwrap()
        })
        .collect();
    let count = result.states.len();
    let partitions = result
        .states
        .iter()
        .map(|state| state.resume_status)
        .collect();
    let runtime = super::Runtime::new(&mut setup).unwrap();
    let mut engine = Engine {
        domain: domain.borrowed(),
        policies,
        result,
        budget,
        counters,
        storage_reads: super::transfer::StorageReadPolicy::Abstract,
        alternatives,
        selectors: BTreeSet::new(),
        ids,
        runtime,
        cooperation: Some(super::Cooperation::new(super::Empty::default())),
        inputs: Vec::new(),
        active: None,
        active_node: None,
        test_driver: None,
        partitions,
        updates: vec![0; count],
        feedback: vec![false; count],
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new(); count],
        cache: None,
        procedures: super::transfer::procedure::EvmTable::default(),
        reuse: None,
        terminal_boundary: super::TerminalBoundary::default(),
    };
    let order: Vec<_> = (0..count).filter(|id| *id != child).collect();
    engine.reset_pending(&order);
    engine
}

fn assert_pending(engine: &Engine<'_>, child: usize, reason: &FrontierReason) {
    let mut targets: Vec<_> = engine
        .result
        .frontiers
        .iter()
        .map(|frontier| {
            assert_eq!(&frontier.reason, reason);
            frontier.target.clone().unwrap()
        })
        .collect();
    let mut expected: Vec<_> = engine
        .runtime
        .pending_world()
        .chain(std::iter::once(child))
        .map(|id| engine.result.states[id].key.clone())
        .collect();
    // Success/failure partitions can share a public machine key. Every queued
    // input still needs a frontier, including both occurrences of that key.
    targets.sort();
    expected.sort();
    assert_eq!(targets, expected);
    assert_eq!(engine.result.summary_stats.imported_states, 0);
    assert_eq!(engine.result.summary_stats.hits, 0);
    assert!(engine.result.summaries.is_empty());
}

#[test]
fn cancelled_summary_and_replay_preserve_the_cause_for_every_queued_input() {
    for import in [false, true] {
        let (result, child) = interrupted_fixture();
        let entries: Vec<_> = result
            .states
            .iter()
            .map(|state| state.entry.clone())
            .collect();
        let history = result.edges.clone();
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(100_000_000);
            let mut counters = Counters::default();
            let numeric_policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());

            let precision_policies: super::EnginePolicies = Default::default();

            let mut engine = engine(
                result,
                child,
                &mut budget,
                &mut counters,
                &numeric_policy,
                &precision_policies,
            );
            let mut cache = Cache::new();
            cache.call_entries.insert(child);
            token.cancel();
            let reason = if import {
                let certificate = Certificate {
                    record: 0,
                    nodes: Vec::new(),
                    edges: Vec::new(),
                    terminals: Vec::new(),
                };
                // Cancellation is observed before any certificate import.
                engine.replay(child, &certificate).unwrap_err()
            } else {
                let SummaryAttempt::Interrupted(reason) = engine.try_summary(child, &mut cache)
                else {
                    panic!("cancelled summary input must interrupt execution");
                };
                reason
            };
            engine.stop_queued(reason.clone());
            assert_eq!(reason, FrontierReason::Relations(QueryReason::Cancelled));
            assert_pending(&engine, child, &reason);
            assert_eq!(engine.result.edges, history);
            for (state, before) in engine.result.states.iter().zip(&entries) {
                assert_eq!(&state.entry, before);
            }
        });
    }
}

#[test]
fn actual_summary_registration_exhaustion_remains_summary_work() {
    let (result, child) = interrupted_fixture();
    let mut cache = Cache::new();
    cache.call_entries.insert(child);
    // Prime the optional fingerprint, then measure only complete input cloning.
    let mut namespace = WorkBudget::new(100_000_000);
    cache
        .input(&result, child, &mut namespace)
        .unwrap()
        .unwrap();
    let mut measure = WorkBudget::new(100_000_000);
    let input = cache.input(&result, child, &mut measure).unwrap().unwrap();
    let mut budget = WorkBudget::new(measure.used() + input.work_size() - 1);
    let mut counters = Counters::default();
    let numeric_policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());

    let precision_policies: super::EnginePolicies = Default::default();

    let mut engine = engine(
        result,
        child,
        &mut budget,
        &mut counters,
        &numeric_policy,
        &precision_policies,
    );
    let SummaryAttempt::Interrupted(reason) = engine.try_summary(child, &mut cache) else {
        panic!("registration exceeded the actual remaining work");
    };
    engine.stop_queued(reason.clone());
    assert_eq!(reason, FrontierReason::SummaryWork);
    assert_pending(&engine, child, &reason);
    assert_eq!(engine.result.summary_stats.misses, 1);
    assert!(engine.budget.exhausted());
}

#[test]
fn initial_lift_distinguishes_cancellation_from_real_work_exhaustion() {
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let token = analysis_control::Cancellation::new();
    token.cancel();
    let cancelled = analysis_control::with_cancellation(&token, || {
        run_world(world.clone(), entry.clone(), fixtures::config()).unwrap()
    });
    assert_eq!(cancelled.status(), Status::Incomplete);
    assert_eq!(
        cancelled.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::Cancelled)
    );
    let mut config = fixtures::config();
    config.max_work = 1;
    let exhausted = run_world(world, entry, config).unwrap();
    assert_eq!(exhausted.status(), Status::Incomplete);
    assert_eq!(exhausted.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn pending_frames_prefer_deeper_calls_and_keep_fifo_within_a_depth() {
    let (result, child) = interrupted_fixture();
    let shallow: Vec<_> = (0..result.states.len()).filter(|id| *id != child).collect();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let numeric_policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());

    let precision_policies: super::EnginePolicies = Default::default();

    let mut engine = engine(
        result,
        child,
        &mut budget,
        &mut counters,
        &numeric_policy,
        &precision_policies,
    );
    engine.test_enqueue(child);
    assert_eq!(engine.next_pending().unwrap(), Some(child));
    for id in shallow {
        assert_eq!(engine.next_pending().unwrap(), Some(id));
    }
    assert_eq!(engine.next_pending().unwrap(), None);
    assert!(engine.budget.used() > 0);
}

#[test]
fn interrupted_queue_scan_retains_every_input_and_its_exact_cause() {
    for cancelled in [false, true] {
        let (result, child) = interrupted_fixture();
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(if cancelled { 100_000_000 } else { 0 });
            let mut counters = Counters::default();
            let numeric_policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());

            let precision_policies: super::EnginePolicies = Default::default();

            let mut engine = engine(
                result,
                child,
                &mut budget,
                &mut counters,
                &numeric_policy,
                &precision_policies,
            );
            engine.test_enqueue(child);
            let worklist = engine.runtime.queue_snapshot();
            let entries: Vec<_> = engine
                .result
                .states
                .iter()
                .map(|s| s.entry.clone())
                .collect();
            if cancelled {
                token.cancel();
            }
            engine.run();
            assert_eq!(engine.result.status(), Status::Incomplete);
            assert_eq!(engine.runtime.queue_snapshot(), worklist);
            assert_eq!(engine.result.frontiers.len(), entries.len());
            let expected = if cancelled {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            };
            for (state, entry) in engine.result.states.iter().zip(entries) {
                assert_eq!(state.entry, entry);
                assert!(
                    engine
                        .result
                        .frontiers
                        .iter()
                        .any(|f| { f.target.as_ref() == Some(&state.key) && f.reason == expected })
                );
            }
            assert_eq!(engine.result.summary_stats.imported_states, 0);
        });
    }
}

impl Engine<'_> {
    fn reset_pending(&mut self, order: &[usize]) {
        let mut setup = WorkBudget::new(usize::MAX);
        let mut runtime = super::Runtime::new(&mut setup).unwrap();
        let scope = runtime.world;
        let mut inputs = Vec::new();
        for _ in &self.result.states {
            inputs.push(
                runtime
                    .kernel()
                    .slot(scope, &mut super::MachineMeter(&mut setup))
                    .unwrap(),
            );
        }
        for &id in order {
            runtime
                .register_world(
                    id,
                    self.result.states[id].entry.call_stack.depth(),
                    &mut setup,
                )
                .unwrap();
        }
        for state in &self.result.states {
            if !order.contains(&state.id) {
                runtime
                    .register_world(state.id, state.entry.call_stack.depth(), &mut setup)
                    .unwrap();
                runtime
                    .satisfy_world(
                        state.id,
                        &inputs[state.id],
                        &state.entry,
                        state.entry.work_size(),
                        &mut setup,
                    )
                    .unwrap();
            }
        }
        let (driver, token) = runtime.test_driver(&mut setup).unwrap();
        self.runtime = runtime;
        self.inputs = inputs;
        self.active = Some(token);
        self.active_node = None;
        self.test_driver = Some(driver);
    }

    fn test_enqueue(&mut self, id: usize) {
        self.runtime
            .requeue_world(id, &mut WorkBudget::new(usize::MAX))
            .unwrap();
    }

    fn next_pending(&mut self) -> Result<Option<usize>, crate::analysis::MachineError> {
        let Some((task, token)) = self.runtime.next(false, self.budget)? else {
            return Ok(None);
        };
        let super::TaskKind::World { node, .. } = &task.0 else {
            return Ok(None);
        };
        let node = *node;
        self.runtime.finish(token, self.budget)?;
        Ok(Some(node))
    }
}

#[test]
fn popped_input_work_refusal_keeps_its_own_frontier() {
    let (result, child) = interrupted_fixture();
    let policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());
    let policies: super::EnginePolicies = Default::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        child,
        &mut budget,
        &mut counters,
        &policy,
        &policies,
    );
    engine.reset_pending(&[child]);
    let scan = engine.runtime.scan_work();
    *engine.budget = WorkBudget::new(scan + 2); // task handle + begin fit, input read does not.
    engine.run();
    assert_eq!(engine.result.status(), Status::Incomplete);
    assert_eq!(engine.result.frontiers.len(), 1);
    assert_eq!(
        engine.result.frontiers[0].target.as_ref(),
        Some(&engine.result.states[child].key)
    );
    assert_eq!(engine.result.frontiers[0].reason, FrontierReason::Work);
    assert!(!engine.runtime.closed());
}

#[test]
fn covered_successors_preserve_admission_history_and_execution_receipts() {
    let (result, child) = interrupted_fixture();
    let policy = crate::domain::ValueAlgebra::new(result.config.domain().unwrap());
    let policies: super::EnginePolicies = Default::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(
        result,
        child,
        &mut budget,
        &mut counters,
        &policy,
        &policies,
    );
    let history = engine.alternatives[child].work_size();
    let entry = engine.result.states[child].entry.clone();
    let receipt = engine.result.states[child]
        .execution_evidence()
        .unwrap()
        .is_current();
    for _ in 0..4 {
        assert_eq!(
            engine.successor(
                0,
                super::Successor {
                    payload: entry.clone(),
                    kind: crate::analysis::MachineEdgeKind::Call
                },
                FrontierReason::Work
            ),
            Some(child)
        );
        assert_eq!(engine.alternatives[child].work_size(), history);
        assert_eq!(engine.result.states[child].entry, entry);
        assert_eq!(
            engine.result.states[child]
                .execution_evidence()
                .unwrap()
                .is_current(),
            receipt
        );
        assert!(!engine.runtime.contains_world(&child));
    }
}
