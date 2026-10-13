use evm_abstract_protocol::{
    AnalysisReport, BlockCoverage, CfgEdge, EdgeKind, Frontier, FrontierDetails, FrontierKind,
    Outcome, OutcomeKind,
};

use super::{TerminationCounts, TerminationSummary, Terminations};
use crate::widgets::graph::{
    index::ReportIndex,
    projection::{DisplayGraph, GraphMode},
};

fn report() -> AnalysisReport {
    let mut report = crate::tests::report();
    report.edges.clear();
    report.outcomes.clear();
    report.frontiers.clear();
    report
}

fn outcome(state: usize, kind: OutcomeKind) -> Outcome {
    Outcome {
        state,
        kind,
        data: 0,
        store: 0,
    }
}

fn edge(id: usize, from: usize, to: usize, kind: EdgeKind) -> CfgEdge {
    CfgEdge { id, from, to, kind }
}

fn expected_counts(kind: OutcomeKind, count: usize) -> TerminationCounts {
    match kind {
        OutcomeKind::Return => TerminationCounts {
            returns: count,
            ..TerminationCounts::default()
        },
        OutcomeKind::Revert => TerminationCounts {
            reverts: count,
            ..TerminationCounts::default()
        },
        OutcomeKind::Failure => TerminationCounts {
            failures: count,
            ..TerminationCounts::default()
        },
    }
}

#[test]
fn each_root_kind_is_terminal_and_duplicate_payloads_count_one_state() {
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        let mut report = report();
        report.outcomes = vec![outcome(1, kind), outcome(1, kind)];
        report.outcomes[1].store = 8;
        let summary = Terminations::new(&report).summary(&[1, 1]);
        assert_eq!(summary.members, 1);
        assert_eq!(summary.ending, 1);
        assert_eq!(summary.root, expected_counts(kind, 1));
        assert_eq!(summary.frame, TerminationCounts::default());
        assert!(summary.has_termination());
        assert_eq!(summary.single_kind(), Some(kind));
    }
}

#[test]
fn frame_endings_require_exactly_one_child_frame_pop_and_deduplicate_edges() {
    for (kind, expected) in [
        (EdgeKind::Return, OutcomeKind::Return),
        (EdgeKind::Revert, OutcomeKind::Revert),
        (EdgeKind::Failure, OutcomeKind::Failure),
    ] {
        let mut report = report();
        report.cfg[1].frame_depth = 3;
        report.cfg[2].frame_depth = 2;
        report.edges = vec![edge(91, 1, 2, kind), edge(999, 1, 2, kind)];
        let summary = Terminations::new(&report).summary(&[1]);
        assert_eq!(summary.frame, expected_counts(expected, 1));
        assert_eq!(summary.root, TerminationCounts::default());
        assert_eq!(summary.continuing, 0);
        assert_eq!(summary.single_kind(), Some(expected));
    }
    // Skipping multiple frames and edges from a root do not establish a child
    // completion, even if their transfer category is Return.
    let mut report = report();
    report.cfg[1].frame_depth = 3;
    report.edges = vec![edge(0, 1, 0, EdgeKind::Return)];
    let summary = Terminations::new(&report).summary(&[1]);
    assert!(!summary.has_termination());
    assert_eq!(summary.continuing, 1);
    assert_eq!(summary.single_kind(), None);
}

#[test]
fn rejected_call_failure_at_same_depth_is_observed_continuation() {
    let mut report = report();
    report.cfg[0].frame_depth = 2;
    report.cfg[1].frame_depth = 2;
    report.edges = vec![edge(40, 0, 1, EdgeKind::Failure)];
    let summary = Terminations::new(&report).summary(&[0]);
    assert_eq!(summary.frame, TerminationCounts::default());
    assert_eq!(summary.continuing, 1);
    assert_eq!(summary.unobserved, 0);
    assert!(!summary.has_termination());
    assert_eq!(summary.single_kind(), None);
}

#[test]
fn root_outcome_with_another_successor_is_mixed_evidence() {
    let mut report = report();
    report.outcomes = vec![outcome(0, OutcomeKind::Failure)];
    report.edges = vec![edge(71, 0, 1, EdgeKind::Jump)];
    let summary = Terminations::new(&report).summary(&[0]);
    assert_eq!(summary.root.failures, 1);
    assert_eq!(summary.ending, 1);
    assert_eq!(summary.continuing, 1);
    assert!(summary.has_termination());
    assert_eq!(summary.single_kind(), None);
}

#[test]
fn no_edges_and_frontiers_never_invent_a_terminal_kind() {
    let mut report = report();
    let no_edges = Terminations::new(&report).summary(&[1]);
    assert_eq!(no_edges.unobserved, 1);
    assert!(!no_edges.has_termination());
    assert_eq!(no_edges.single_kind(), None);

    report.frontiers = vec![Frontier {
        range: None,
        payload: None,
        from: Some(1),
        pc: Some(5),
        kind: FrontierKind::Work,
        reason: FrontierDetails::Work,
        detail: "unexpanded transfer".into(),
    }];
    let frontier = Terminations::new(&report).summary(&[1]);
    assert_eq!(frontier.unresolved, 1);
    assert!(!frontier.has_termination());
    report.outcomes = vec![outcome(1, OutcomeKind::Return)];
    let ending_and_frontier = Terminations::new(&report).summary(&[1]);
    assert!(ending_and_frontier.has_termination());
    assert_eq!(ending_and_frontier.unresolved, 1);
    assert_eq!(ending_and_frontier.single_kind(), None);
}

#[test]
fn hidden_program_neighbors_and_zero_hop_local_views_do_not_erase_continuation() {
    let mut report = report();
    report.cfg[1].program = Some(19);
    report.edges = vec![edge(30, 0, 1, EdgeKind::Call)];
    report.outcomes = vec![outcome(0, OutcomeKind::Failure)];
    let index = ReportIndex::new(&report);
    for (mode, program, center) in [
        (GraphMode::States, Some(0), None),
        (GraphMode::Local, None, Some(0)),
    ] {
        let display = DisplayGraph::build(&report, &index, mode, program, center, 0);
        let node = display.nodes.iter().find(|node| node.id == 0).unwrap();
        assert!(display.edges.is_empty());
        let summary = index.termination_summary(&node.members);
        assert_eq!(summary.root.failures, 1);
        assert_eq!(summary.continuing, 1);
        assert_eq!(summary.single_kind(), None);
    }
}

#[test]
fn block_folding_keeps_multiple_kinds_and_ending_with_non_ending_members_neutral() {
    let mut report = report();
    report.cfg[1].basic_block = report.cfg[0].basic_block;
    report.cfg[1].start_pc = report.cfg[0].start_pc;
    report.outcomes = vec![
        outcome(0, OutcomeKind::Return),
        outcome(1, OutcomeKind::Revert),
    ];
    let index = ReportIndex::new(&report);
    let display = DisplayGraph::build(&report, &index, GraphMode::Blocks, None, None, 0);
    let node = display.nodes.iter().find(|node| node.id == 0).unwrap();
    assert_eq!(node.members, [0, 1]);
    let mixed = index.termination_summary(&node.members);
    assert_eq!(mixed.root.returns, 1);
    assert_eq!(mixed.root.reverts, 1);
    assert_eq!(mixed.ending, 2);
    assert_eq!(mixed.single_kind(), None);

    report.outcomes.pop();
    report.edges = vec![edge(1, 1, 2, EdgeKind::Fallthrough)];
    let mixed = Terminations::new(&report).summary(&node.members);
    assert_eq!(mixed.ending, 1);
    assert_eq!(mixed.continuing, 1);
    assert_eq!(mixed.single_kind(), None);
    report.edges.clear();
    let mixed = Terminations::new(&report).summary(&node.members);
    assert_eq!(mixed.unobserved, 1);
    assert_eq!(mixed.single_kind(), None);
}

#[test]
fn multiple_types_on_one_state_are_retained_and_shared_type_across_scopes_can_be_pure() {
    let mut report = report();
    report.outcomes = vec![
        outcome(0, OutcomeKind::Return),
        outcome(0, OutcomeKind::Revert),
        outcome(0, OutcomeKind::Failure),
    ];
    let mixed = Terminations::new(&report).summary(&[0]);
    assert_eq!(
        mixed.root,
        TerminationCounts {
            returns: 1,
            reverts: 1,
            failures: 1
        }
    );
    assert_eq!(mixed.ending, 1);
    assert_eq!(mixed.single_kind(), None);

    report.outcomes.truncate(1);
    report.cfg[1].frame_depth = 2;
    report.edges = vec![edge(88, 1, 2, EdgeKind::Return)];
    let homogeneous = Terminations::new(&report).summary(&[0, 1]);
    assert_eq!(homogeneous.root.returns, 1);
    assert_eq!(homogeneous.frame.returns, 1);
    assert_eq!(homogeneous.ending, 2);
    assert_eq!(homogeneous.single_kind(), Some(OutcomeKind::Return));
}

#[test]
fn non_current_and_missing_receipts_keep_observed_endings_but_block_pure_fill() {
    for coverage in [BlockCoverage::Stale, BlockCoverage::Unexecuted] {
        let mut report = report();
        report.ssa.blocks[1].coverage = coverage;
        report.outcomes = vec![outcome(1, OutcomeKind::Return)];
        let summary = Terminations::new(&report).summary(&[1]);
        assert_eq!(summary.root.returns, 1);
        assert_eq!(summary.non_current, 1);
        assert_eq!(summary.single_kind(), None);
    }
    let mut report = report();
    report.ssa.blocks.retain(|block| block.state != 1);
    report.outcomes = vec![outcome(1, OutcomeKind::Return)];
    let summary = Terminations::new(&report).summary(&[1]);
    assert_eq!(summary.non_current, 1);
    assert_eq!(summary.single_kind(), None);
}

#[test]
fn dangling_edges_and_sparse_native_ids_remain_conservative() {
    let mut report = report();
    for (position, id) in [205, 7, 91].into_iter().enumerate() {
        report.cfg[position].id = id;
        report.ssa.blocks[position].state = id;
    }
    report.ssa.blocks.swap(0, 2);
    report.cfg[1].frame_depth = 2;
    report.outcomes = vec![outcome(91, OutcomeKind::Revert)];
    report.edges = vec![
        edge(42, 7, 205, EdgeKind::Return),
        edge(800, 91, 999, EdgeKind::Jump),
    ];
    let terminations = Terminations::new(&report);
    assert_eq!(
        terminations.summary(&[7]).single_kind(),
        Some(OutcomeKind::Return)
    );
    let dangling = terminations.summary(&[91]);
    assert_eq!(dangling.root.reverts, 1);
    assert_eq!(dangling.unresolved, 1);
    assert_eq!(dangling.single_kind(), None);
    assert_eq!(terminations.summary(&[999]).unobserved, 1);
    assert_eq!(terminations.summary(&[]), TerminationSummary::default());

    report.edges = vec![edge(999, 999, 91, EdgeKind::Return)];
    assert_eq!(Terminations::new(&report).summary(&[91]).unresolved, 1);
}
