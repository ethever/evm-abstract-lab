//! The two engine installation paths share admission, partition and scheduling.

use super::*;
use crate::{
    analysis::{
        EdgeKind, MachineEdge, MachineEdgeKind, MachinePayload, OutcomeKind, RootFrame,
        engine::ExecutionReuse,
        layers::{CodeCatalog, FactDependency},
        summary::{Node, Prepared},
        transfer::Successor,
    },
    domain::Domain,
    world::ByteArray,
};
use alloy_primitives::U256;
use std::sync::Arc;

fn with_word(base: &MachinePayload, word: u64, domain: Domain) -> MachinePayload {
    let mut payload = base.clone();
    payload.active_mut().stack = vec![fixtures::word(word)];
    payload.active_mut().memory = ByteArray::memory();
    payload
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(word), 4096, domain)
        .unwrap();
    payload.normalize();
    payload
}

fn fixture() -> (WorldAnalysis, usize, [MachinePayload; 3]) {
    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    config.analysis.relations.max_constraints = 2;
    let domain = config.domain().unwrap();
    let mut result = fixtures::run("6003565b00", config);
    let target = 1;
    assert_eq!(result.states[target].active().basic_block_index, 1);
    let inputs = [1, 2, 3].map(|n| with_word(&result.states[target].entry, n, domain));
    result.states[target].entry = inputs[0].clone();
    result.states[target].key = inputs[0].key();
    (result, target, inputs)
}

fn reuse(result: &WorldAnalysis) -> ExecutionReuse {
    let (catalog, _) = CodeCatalog::new(result.world.fork())
        .with_runtime(result.states[0].entry.active().program.clone().unwrap())
        .unwrap();
    ExecutionReuse::new(
        Arc::new(catalog),
        0,
        result.config.max_work,
        result.config.max_memory_bytes,
    )
}

fn node(entry: MachinePayload, resume_status: Option<bool>, marker: u64) -> Node {
    let mut exit = entry.clone();
    exit.active_mut().stack = vec![fixtures::word(marker)];
    Node {
        entry,
        resume_status,
        exit,
        exit_stack: vec![fixtures::word(marker)],
        executed_pcs: vec![usize::MAX],
        instruction_progress: Vec::new(),
        instruction_values: Arc::from([]),
        diagnostics: Vec::new(),
        completed_calls: Vec::new(),
    }
}

fn certificate(nodes: Vec<Node>) -> Certificate {
    Certificate {
        record: 0,
        nodes,
        edges: Vec::new(),
        terminals: Vec::new(),
    }
}

fn has_initial_dependency(engine: &Engine<'_>, target: usize) -> bool {
    engine
        .reuse
        .as_ref()
        .unwrap()
        .dependencies
        .reads()
        .any(|(dependency, states)| {
            *dependency == FactDependency::AllInitialState && states.contains(&target)
        })
}

#[test]
fn successor_promotes_invalidates_reexecutes_and_keeps_canonical_roots_bounded() {
    let (result, target, [first, second, third]) = fixture();
    let mut reuse = reuse(&result);
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters {
        states: result.states.len(),
        ..Default::default()
    };
    let mut engine = engine(result, target, &mut budget, &mut counters);
    engine.reuse = Some(&mut reuse);
    engine.worklist = Worklist::default();
    let kind = MachineEdgeKind::Intraprocedural(EdgeKind::Jump);
    assert_eq!(
        engine.successor(
            0,
            Successor {
                payload: second,
                kind
            },
            FrontierReason::Work
        ),
        Some(target)
    );
    assert!(!engine.alternatives[target].has_loop_bound());
    engine.run();
    assert!(
        engine.result.states[target]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    let selectors = engine.selectors.clone();
    assert_eq!(
        engine.successor(
            0,
            Successor {
                payload: third.clone(),
                kind
            },
            FrontierReason::Work
        ),
        Some(target)
    );
    assert!(engine.alternatives[target].has_loop_bound());
    assert!(has_initial_dependency(&engine, target));
    assert_eq!(engine.selectors, selectors);
    assert!(
        !engine.result.states[target]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    assert!(engine.worklist.contains(&target));
    assert_eq!(
        engine
            .worklist
            .in_order()
            .filter(|id| *id == target)
            .count(),
        1
    );
    let roots = engine.result.states[target].entry.fresh_leaves();
    let work = engine.result.states[target].entry.work_size();
    engine.run();
    assert_eq!(engine.result.status(), Status::Converged);
    assert!(engine.result.frontiers.is_empty());
    assert!(
        engine.result.states[target]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    assert!(
        engine
            .result
            .outcomes
            .iter()
            .any(|outcome| outcome.state == target && outcome.kind == OutcomeKind::Return)
    );
    for payload in [first, third] {
        assert_eq!(
            engine.successor(0, Successor { payload, kind }, FrontierReason::Work),
            Some(target)
        );
        assert_eq!(engine.result.states[target].entry.fresh_leaves(), roots);
        assert_eq!(engine.result.states[target].entry.work_size(), work);
        assert!(!engine.worklist.contains(&target));
    }
    assert_eq!(engine.result.states.len(), 2);
}

#[test]
fn replay_keeps_same_key_resume_partitions_and_reexecutes_incompatible_certificates() {
    let (result, target, [first, second, third]) = fixture();
    let mut reuse = reuse(&result);
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters {
        states: result.states.len(),
        ..Default::default()
    };
    let mut engine = engine(result, target, &mut budget, &mut counters);
    engine.reuse = Some(&mut reuse);
    engine.worklist = Worklist::default();
    let failed = with_word(&first, 9, engine.domain);
    let certificate = certificate(vec![
        node(first.clone(), Some(true), 71),
        node(failed.clone(), Some(false), 72),
        node(first.clone(), None, 73),
        node(second, Some(true), 74),
        node(third, Some(true), 75),
    ]);
    let prepared = Prepared {
        nodes: certificate.nodes.clone(),
        terminals: Vec::new(),
        renaming: BTreeMap::new(),
    };
    engine.install_replay(&certificate, prepared).unwrap();
    let key = first.key();
    let success = engine.ids[&(key.clone(), Some(true))];
    let failure = engine.ids[&(key.clone(), Some(false))];
    assert_eq!(engine.ids[&(key, None)], target);
    assert_ne!(success, failure);
    assert_ne!(success, target);
    assert!(engine.alternatives[success].has_loop_bound());
    assert!(!engine.alternatives[failure].has_loop_bound());
    assert!(!engine.alternatives[target].has_loop_bound());
    assert_eq!(engine.result.states[failure].entry, failed);
    assert_eq!(engine.result.states[target].entry, first);
    assert!(has_initial_dependency(&engine, success));
    // Compatible None installs its exact exit; the promoted success cannot use
    // the final certificate arm's marker 75 as the exit of all its inputs.
    assert_eq!(
        engine.result.states[target].exit_stack,
        vec![fixtures::word(73)]
    );
    assert_eq!(
        engine.result.states[success].exit_stack,
        vec![fixtures::word(71)]
    );
    assert!(
        !engine.result.states[success]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    assert!(engine.worklist.contains(&success));
    assert!(!engine.worklist.contains(&failure));
    assert!(!engine.worklist.contains(&target));
    engine.run();
    assert_eq!(engine.result.status(), Status::Converged);
    assert!(engine.result.frontiers.is_empty());
    assert_eq!(engine.result.states[success].executed_pcs, vec![3, 4]);
    assert!(
        engine
            .result
            .outcomes
            .iter()
            .any(|outcome| outcome.state == success && outcome.kind == OutcomeKind::Return)
    );
}

#[test]
fn successor_work_refusal_keeps_old_entry_proof_and_full_incoming_frontier() {
    for reason in [FrontierReason::Work, FrontierReason::SummaryWork] {
        let (result, target, [first, _, third]) = fixture();
        let mut budget = WorkBudget::new(0);
        let mut counters = Counters::default();
        let mut engine = engine(result, target, &mut budget, &mut counters);
        let worklist = engine.worklist.clone();
        let before = engine.alternatives[target].work_size();
        assert!(
            engine
                .successor(
                    0,
                    Successor {
                        payload: third.clone(),
                        kind: MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
                    },
                    reason.clone()
                )
                .is_none()
        );
        assert_eq!(engine.result.states[target].entry, first);
        assert_eq!(engine.alternatives[target].work_size(), before);
        assert!(!engine.alternatives[target].has_loop_bound());
        assert_eq!(engine.worklist, worklist);
        assert!(engine.edges.is_empty());
        let frontier = engine.result.frontiers.last().unwrap();
        assert_eq!(frontier.reason, reason);
        assert_eq!(frontier.payload.as_deref(), Some(&third));
    }
}

#[test]
fn actual_staged_replay_publishes_promotion_and_dependencies_only_after_success() {
    for reject_after_promotion in [false, true] {
        let (mut result, child) = interrupted_fixture();
        result.config.analysis.relations.enabled = false;
        result.config.analysis.relations.max_constraints = 2;
        let domain = result.config.domain().unwrap();
        let first = with_word(&result.states[child].entry, 1, domain);
        let second = with_word(&first, 2, domain);
        let third = with_word(&first, 3, domain);
        let exact = first.join(&second, domain).unwrap();
        result.states[child].entry = exact.clone();
        result.states[child].key = exact.key();
        let mut relative = third.clone();
        relative.call_stack = crate::analysis::CallStack::new(RootFrame {
            state: third.active().clone(),
        });
        relative.normalize();
        let mut certificate = certificate(vec![node(relative, None, 99)]);
        if reject_after_promotion {
            // A later graph refusal must roll back the temporary promotion too.
            certificate.edges.push(MachineEdge {
                from: 0,
                to: usize::MAX,
                kind: MachineEdgeKind::Intraprocedural(EdgeKind::Jump),
            });
        }
        let mut reuse = reuse(&result);
        let mut budget = WorkBudget::new(100_000_000);
        let mut counters = Counters {
            states: result.states.len(),
            ..Default::default()
        };
        let mut engine = engine(result, child, &mut budget, &mut counters);
        engine.reuse = Some(&mut reuse);
        engine.alternatives[child] = super::super::EntryAlternatives::new(&first);
        engine.alternatives[child].retain(second);
        let before_work = engine.alternatives[child].work_size();
        let worklist = engine.worklist.clone();
        let count = engine.result.states.len();
        if reject_after_promotion {
            assert_eq!(
                engine.replay(child, &certificate),
                Err(FrontierReason::StateJoin(
                    crate::analysis::JoinBoundary::Shape
                ))
            );
            assert_eq!(engine.result.states[child].entry, exact);
            assert_eq!(engine.alternatives[child].work_size(), before_work);
            assert!(!engine.alternatives[child].has_loop_bound());
            assert!(!has_initial_dependency(&engine, child));
            assert_eq!(engine.worklist, worklist);
            assert_eq!(engine.result.summary_stats.imported_states, 0);
            assert_eq!(
                engine.result.frontiers.last().unwrap().payload.as_deref(),
                Some(&exact)
            );
        } else {
            engine.replay(child, &certificate).unwrap();
            assert!(engine.alternatives[child].has_loop_bound());
            assert!(has_initial_dependency(&engine, child));
            assert!(engine.worklist.contains(&child));
            assert_ne!(engine.result.states[child].executed_pcs, vec![usize::MAX]);
            assert!(
                !engine.result.states[child]
                    .execution_evidence()
                    .unwrap()
                    .is_current()
            );
            for word in [1, 2, 3] {
                assert!(
                    engine.result.states[child].entry.active().stack[0].contains(U256::from(word))
                );
            }
        }
        assert_eq!(engine.result.states.len(), count);
        assert!(engine.budget.used() > 0);
    }
}

#[test]
fn replay_satisfaction_then_growth_keeps_fifo_position_cost_and_stop_coverage() {
    let (result, target, [first, second, _]) = fixture();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters::default();
    let mut engine = engine(result, target, &mut budget, &mut counters);
    engine.worklist = [target, 0].into_iter().collect();
    let certificate = certificate(vec![node(first, None, 73)]);
    let prepared = Prepared {
        nodes: certificate.nodes.clone(),
        terminals: Vec::new(),
        renaming: BTreeMap::new(),
    };
    engine.install_replay(&certificate, prepared).unwrap();
    assert!(!engine.worklist.contains(&target));
    assert_eq!(engine.worklist.work_size(), 3);
    assert_eq!(
        engine.successor(
            0,
            Successor {
                payload: second,
                kind: MachineEdgeKind::Intraprocedural(EdgeKind::Jump),
            },
            FrontierReason::Work,
        ),
        Some(target),
    );
    assert!(
        !engine.result.states[target]
            .execution_evidence()
            .unwrap()
            .is_current()
    );
    // Reactivation makes the older record eligible again. Neither compacting
    // it nor moving it behind state 0 preserves the established schedule/fee.
    assert_eq!(engine.worklist.work_size(), 5);
    let used = engine.budget.used();
    assert_eq!(engine.next_pending().unwrap(), Some(target));
    assert_eq!(engine.budget.used() - used, 7);
    let reason = FrontierReason::Budget(crate::analysis::Limit::Transfers);
    engine.stop_pending(target, reason.clone());
    assert_eq!(engine.result.frontiers.len(), 2);
    assert_eq!(
        engine
            .result
            .frontiers
            .iter()
            .map(|frontier| {
                assert_eq!(frontier.reason, reason);
                assert_eq!(frontier.from, None);
                frontier.target.clone().unwrap()
            })
            .collect::<Vec<_>>(),
        [target, 0].map(|id| engine.result.states[id].key.clone()),
    );
    assert_eq!(engine.next_pending().unwrap(), Some(0));
    let used = engine.budget.used();
    assert_eq!(engine.next_pending().unwrap(), None);
    assert_eq!(engine.budget.used(), used);
}
