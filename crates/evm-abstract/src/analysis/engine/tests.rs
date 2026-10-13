//! Cancellation and real exhaustion retain every queued input with its cause.

use super::{Cache, Counters, Engine, SummaryAttempt, run_world};
use crate::{
    analysis::{FrontierReason, Status, WorldAnalysis, machine::fixtures, summary::Certificate},
    domain::relational::QueryReason,
    resource::WorkBudget,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
) -> Engine<'a> {
    let ids: BTreeMap<_, _> = result
        .states
        .iter()
        .map(|state| (state.key.clone(), state.id))
        .collect();
    let pending: BTreeSet<_> = (0..result.states.len()).filter(|id| *id != child).collect();
    let alternatives = result
        .states
        .iter()
        .map(|state| super::EntryAlternatives::new(&state.entry))
        .collect();
    let count = result.states.len();
    Engine {
        domain: result.config.domain().unwrap(),
        result,
        budget,
        counters,
        storage_reads: super::transfer::StorageReadPolicy::Abstract,
        alternatives,
        selectors: BTreeSet::new(),
        ids,
        queue: VecDeque::from_iter(pending.iter().copied()),
        queued: pending,
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new(); count],
        cache: None,
        unknown: super::transfer::unknown::Table::default(),
    }
}

fn assert_pending(engine: &Engine<'_>, child: usize, reason: &FrontierReason) {
    let targets: BTreeSet<_> = engine
        .result
        .frontiers
        .iter()
        .map(|frontier| {
            assert_eq!(&frontier.reason, reason);
            frontier.target.clone().unwrap()
        })
        .collect();
    assert_eq!(targets.len(), engine.queued.len() + 1);
    for id in engine.queued.iter().copied().chain(std::iter::once(child)) {
        assert!(targets.contains(&engine.result.states[id].key));
    }
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
        let token = embedded_smt::Cancellation::new();
        embedded_smt::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(100_000_000);
            let mut counters = Counters::default();
            let mut engine = engine(result, child, &mut budget, &mut counters);
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
    let mut engine = engine(result, child, &mut budget, &mut counters);
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
    let token = embedded_smt::Cancellation::new();
    token.cancel();
    let cancelled = embedded_smt::with_cancellation(&token, || {
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
    let mut engine = engine(result, child, &mut budget, &mut counters);
    engine.queue.push_back(child);
    engine.queued.insert(child);
    assert_eq!(engine.next_pending().unwrap(), Some(child));
    engine.queued.remove(&child);
    for id in shallow {
        assert_eq!(engine.next_pending().unwrap(), Some(id));
        engine.queued.remove(&id);
    }
    assert_eq!(engine.next_pending().unwrap(), None);
    assert!(engine.budget.used() > 0);
}

#[test]
fn interrupted_queue_scan_retains_every_input_and_its_exact_cause() {
    for cancelled in [false, true] {
        let (result, child) = interrupted_fixture();
        let token = embedded_smt::Cancellation::new();
        embedded_smt::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(if cancelled { 100_000_000 } else { 0 });
            let mut counters = Counters::default();
            let mut engine = engine(result, child, &mut budget, &mut counters);
            engine.queue.push_back(child);
            engine.queued.insert(child);
            let queue = engine.queue.clone();
            let queued = engine.queued.clone();
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
            assert_eq!(engine.queue, queue);
            assert_eq!(engine.queued, queued);
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
