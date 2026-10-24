//! Canonical adjacency, dynamic feedback and atomic graph-import regressions.

use super::{Counters, Engine, WorldAnalysis, engine, fixtures, interrupted_fixture};
use crate::{
    analysis::{
        CallStack, EdgeKind, FrontierReason, JoinBoundary, MachineEdge, MachineEdgeKind,
        OutcomeKind, RootFrame,
        engine::{Successor, graph::outgoing},
        summary::{Certificate, Node},
        worklist::kernel_failure,
    },
    domain::{ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

fn edge(from: usize, to: usize, kind: MachineEdgeKind) -> MachineEdge {
    MachineEdge { from, to, kind }
}

fn jump(from: usize, to: usize) -> MachineEdge {
    edge(from, to, MachineEdgeKind::Intraprocedural(EdgeKind::Jump))
}

fn with_engine(test: impl FnOnce(&mut Engine<'_>)) {
    let mut config = fixtures::config();
    config.root_terminal_coverage = false;
    let result = fixtures::run(
        "6003565b6007565b00",
        config,
        crate::analysis::testing::NumericLimits::machine().spec(),
    );
    assert_eq!(result.states.len(), 3);
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
    let policies = Default::default();
    let mut budget = WorkBudget::new(100_000_000);
    let mut counters = Counters {
        states: result.states.len(),
        ..Default::default()
    };
    let mut engine = engine(result, 0, &mut budget, &mut counters, &algebra, &policies);
    engine.reset_pending(&[]);
    test(&mut engine);
}

fn arrive(engine: &mut Engine<'_>, from: usize, to: usize) {
    let payload = engine.result.states[to].entry.clone();
    assert_eq!(
        engine.successor(
            from,
            Successor {
                payload,
                kind: MachineEdgeKind::Intraprocedural(EdgeKind::Jump),
            },
            FrontierReason::Work,
        ),
        Some(to),
    );
}

fn version(engine: &mut Engine<'_>, id: usize) -> u64 {
    engine
        .runtime
        .with_run(engine.active.as_mut().unwrap(), engine.budget, |run| {
            run.slot_version(&engine.inputs[id]).map_err(kernel_failure)
        })
        .unwrap()
        .get()
}

fn scan_reaches(edges: &BTreeSet<MachineEdge>, target: usize, from: usize) -> bool {
    let mut queue = VecDeque::from([target]);
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if id == from {
            return true;
        }
        if seen.insert(id) {
            queue.extend(
                edges
                    .iter()
                    .filter(|edge| edge.from == id)
                    .map(|edge| edge.to),
            );
        }
    }
    false
}

#[test]
fn source_prefix_includes_every_kind_and_extreme_endpoint_in_sorted_order() {
    let kinds = [
        MachineEdgeKind::Intraprocedural(EdgeKind::Fallthrough),
        MachineEdgeKind::Intraprocedural(EdgeKind::Jump),
        MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue),
        MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse),
        MachineEdgeKind::Call,
        MachineEdgeKind::Return,
        MachineEdgeKind::Failure,
        MachineEdgeKind::Revert,
        MachineEdgeKind::Summary(OutcomeKind::Return),
        MachineEdgeKind::Summary(OutcomeKind::Revert),
        MachineEdgeKind::Summary(OutcomeKind::Failure),
    ];
    let mut edges = BTreeSet::new();
    for from in [usize::MAX, 4, 0] {
        for to in [usize::MAX, 7, 0] {
            for kind in kinds.into_iter().rev() {
                let edge = edge(from, to, kind);
                assert!(edges.insert(edge.clone()));
                assert!(!edges.insert(edge));
            }
        }
    }
    for from in [0, 1, 4, 5, usize::MAX - 1, usize::MAX] {
        let expected: Vec<_> = edges.iter().filter(|edge| edge.from == from).collect();
        assert_eq!(outgoing(&edges, from).collect::<Vec<_>>(), expected);
    }
    assert_eq!(outgoing(&edges, 0).count(), 3 * kinds.len());
    assert_eq!(outgoing(&edges, usize::MAX).count(), 3 * kinds.len());
    assert_eq!(outgoing(&BTreeSet::new(), usize::MAX).count(), 0);
}

#[test]
fn reachability_matches_full_scan_after_every_arbitrary_edge_insertion() {
    let edges = [jump(2, 0), jump(0, 1), jump(1, 2)];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        with_engine(|engine| {
            for index in order {
                engine.edges.insert(edges[index].clone());
                for target in 0..3 {
                    for from in 0..3 {
                        let expected = scan_reaches(&engine.edges, target, from);
                        assert_eq!(engine.joins_cycle(target, from), Ok(expected));
                    }
                }
            }
        });
    }
}

#[test]
fn indexed_reachability_retains_the_existing_global_edge_charge_per_visited_node() {
    with_engine(|engine| {
        engine.edges.extend([
            jump(2, 0),
            jump(0, 1),
            edge(
                0,
                1,
                MachineEdgeKind::Intraprocedural(EdgeKind::Fallthrough),
            ),
        ]);
        let before = engine.budget.used();
        assert_eq!(engine.joins_cycle(2, usize::MAX), Ok(false));
        assert_eq!(engine.budget.used() - before, 3 * (engine.edges.len() + 1));
        let before = engine.budget.used();
        assert_eq!(engine.joins_cycle(2, 2), Ok(true));
        assert_eq!(engine.budget.used(), before);
    });
}

#[test]
fn known_feedback_skips_a_search_that_would_still_charge_for_another_source() {
    with_engine(|engine| {
        arrive(engine, 0, 0);
        assert!(engine.feedback[0]);
        let input = engine.result.states[0].entry.clone();
        let history = engine.alternatives[0].work_size();
        let first_version = version(engine, 0);
        assert_eq!(first_version, 1);

        // A self-arrival never traversed edges, even before the optimization.
        // Covered admission has the same input, policy and metadata in both
        // calls; only the second source would require the old graph search.
        let before = engine.budget.used();
        arrive(engine, 0, 0);
        let self_work = engine.budget.used() - before;
        let before = engine.budget.used();
        arrive(engine, 1, 0);
        assert_eq!(engine.budget.used() - before, self_work);
        assert_eq!(version(engine, 0), first_version);
        assert_eq!(engine.updates[0], 0);
        assert_eq!(engine.result.states[0].entry, input);
        assert_eq!(engine.alternatives[0].work_size(), history);
        assert!(engine.result.frontiers.is_empty());
    });
}

#[test]
fn a_late_closing_path_marks_a_previously_covered_duplicate_arrival() {
    with_engine(|engine| {
        let input = engine.result.states[1].entry.clone();
        arrive(engine, 0, 1);
        assert!(!engine.feedback[1]);
        assert_eq!(version(engine, 1), 0);
        assert!(
            engine.result.states[1]
                .execution_evidence()
                .unwrap()
                .is_current()
        );
        arrive(engine, 1, 2);
        arrive(engine, 2, 0);
        assert!(engine.feedback[0]);
        assert!(!engine.feedback[1]);

        let edges_before = engine.edges.clone();
        arrive(engine, 0, 1);
        assert_eq!(engine.edges, edges_before);
        assert!(engine.feedback[1]);
        assert_eq!(version(engine, 1), 1);
        assert_eq!(engine.updates[1], 0);
        assert_eq!(engine.result.states[1].entry, input);
        assert!(
            !engine.result.states[1]
                .execution_evidence()
                .unwrap()
                .is_current()
        );
        assert!(engine.runtime.contains_world(&1));
    });
}

fn replay_certificate(result: &WorldAnalysis, child: usize) -> Certificate {
    let state = &result.states[child];
    let relative = |payload: &crate::analysis::MachinePayload| {
        let mut payload = payload.clone();
        payload.call_stack = CallStack::new(RootFrame {
            state: payload.active().clone(),
        });
        payload.normalize();
        payload
    };
    let receipt = state.execution_evidence().unwrap();
    let node = Node {
        entry: relative(&state.entry),
        resume_status: state.resume_status,
        partition: state.resume_status,
        exit: relative(state.exit.as_ref().unwrap()),
        exit_stack: state.exit_stack.clone(),
        executed_pcs: state.executed_pcs.clone(),
        instruction_progress: receipt.instructions().to_vec(),
        instruction_values: Arc::clone(&receipt.instruction_values),
        diagnostics: Vec::new(),
        completed_calls: Vec::new(),
    };
    let mut partitioned = node.clone();
    partitioned.resume_status = Some(true);
    partitioned.partition = Some(true);
    Certificate {
        record: 0,
        nodes: vec![node, partitioned],
        edges: vec![jump(0, 1), jump(1, 0)],
        terminals: Vec::new(),
    }
}

#[test]
fn replay_clone_commit_and_failed_import_keep_adjacency_with_its_graph() {
    for refuse in [false, true] {
        let (result, child) = interrupted_fixture();
        let count = result.states.len();
        let mut certificate = replay_certificate(&result, child);
        if refuse {
            // Refuse after both a new edge and first feedback were staged.
            certificate.edges.push(jump(0, usize::MAX));
        }
        let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(result.domain_spec()));
        let policies = Default::default();
        let mut budget = WorkBudget::new(100_000_000);
        let mut counters = Counters {
            states: count,
            ..Default::default()
        };
        let mut engine = engine(
            result,
            child,
            &mut budget,
            &mut counters,
            &algebra,
            &policies,
        );
        engine.edges.insert(edge(0, child, MachineEdgeKind::Call));
        let before_edges = engine.edges.clone();
        let before_queue = engine.runtime.queue_snapshot();
        let before_work = engine.budget.used();
        let outcome = engine.replay(child, &certificate);
        if refuse {
            assert_eq!(outcome, Err(FrontierReason::StateJoin(JoinBoundary::Shape)));
            assert_eq!(engine.edges, before_edges);
            assert_eq!(engine.result.states.len(), count);
            assert_eq!(engine.counters.states, count + 1);
            assert!(!engine.feedback[child]);
            assert!(
                engine.result.states[child]
                    .execution_evidence()
                    .unwrap()
                    .is_current()
            );
            assert_eq!(engine.runtime.queue_snapshot(), before_queue);
            assert_eq!(engine.joins_cycle(child, count), Ok(false));

            // Reusing the discarded machine-state index must not recover its
            // abandoned reachability or feedback. Commit a fresh import there.
            certificate.edges.pop();
            engine.replay(child, &certificate).unwrap();
            assert_eq!(engine.counters.states, count + 2);
        } else {
            outcome.unwrap();
        }
        assert!(engine.budget.used() > before_work);
        assert_eq!(engine.result.states.len(), count + 1);
        assert!(engine.edges.is_superset(&before_edges));
        assert_eq!(engine.joins_cycle(child, count), Ok(true));
        assert_eq!(engine.joins_cycle(count, child), Ok(true));
        assert!(engine.feedback[child]);
        assert!(
            !engine.result.states[child]
                .execution_evidence()
                .unwrap()
                .is_current()
        );
        assert!(engine.runtime.contains_world(&child));
    }
}
