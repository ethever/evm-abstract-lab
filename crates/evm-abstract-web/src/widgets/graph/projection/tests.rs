use std::collections::BTreeSet;

use evm_abstract_protocol::{AnalysisReport, AnalysisStatus, CfgEdge, EdgeKind};

use super::{DisplayGraph, GraphMode, LOCAL_STATE_LIMIT, ReportIndex};

fn repeated_states(count: usize) -> AnalysisReport {
    let mut report = crate::tests::report();
    let template = report.cfg[0].clone();
    report.cfg = (0..count)
        .map(|id| {
            let mut state = template.clone();
            state.id = id * 7 + 10;
            state.context = vec![id];
            state
        })
        .collect();
    report.edges.clear();
    report
}

fn edge(id: usize, from: usize, to: usize, kind: EdgeKind) -> CfgEdge {
    CfgEdge { id, from, to, kind }
}

#[test]
fn blocks_fold_129_contexts_without_changing_native_analysis_or_member_ids() {
    let mut report = repeated_states(129);
    report.status = AnalysisStatus::Incomplete;
    report.ssa.complete = false;
    report.edges = (0..129)
        .map(|position| {
            edge(
                position * 11 + 3,
                report.cfg[position].id,
                report.cfg[(position + 1) % 129].id,
                EdgeKind::Jump,
            )
        })
        .collect();
    // Storage and call-frame roles remain members of the same source block.
    report.cfg[128].frame_depth = 4;
    report.cfg[128].storage_address = "different storage owner".into();
    let before = report.clone();
    let index = ReportIndex::new(&report);
    let graph = DisplayGraph::build(&report, &index, GraphMode::default(), None, None, 2);
    let ids: Vec<_> = report.cfg.iter().map(|state| state.id).collect();
    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(graph.nodes[0].id, 10);
    assert_eq!(graph.nodes[0].members, ids);
    assert_eq!(graph.nodes[0].program, Some(0));
    assert_eq!(graph.nodes[0].basic_block, 0);
    assert_eq!(graph.nodes[0].start_pc, Some(0));
    assert_eq!(graph.state_to_node.len(), 129);
    assert!(ids.iter().all(|id| graph.state_to_node[id] == 10));
    assert_eq!(graph.edges, [edge(3, 10, 10, EdgeKind::Jump)]);
    assert_eq!(
        graph.edge_members[&3],
        report.edges.iter().map(|edge| edge.id).collect::<Vec<_>>()
    );
    assert_eq!((graph.hidden_states, graph.boundary_edges), (0, 0));
    assert_eq!(
        report, before,
        "projection must preserve SSA and incomplete coverage verbatim"
    );
}

#[test]
fn source_identity_includes_program_and_block_but_synthetic_states_stay_separate() {
    let mut report = repeated_states(8);
    report.cfg[1].program = Some(9);
    report.cfg[2].program = None;
    report.cfg[3].program = None;
    report.cfg[4].start_pc = None;
    report.cfg[5].start_pc = None;
    report.cfg[6].basic_block = 1;
    report.cfg[7].start_pc = Some(9);
    let index = ReportIndex::new(&report);
    let all = DisplayGraph::build(&report, &index, GraphMode::Blocks, None, None, 2);
    assert_eq!(all.nodes.len(), 8);
    assert!(all.nodes.iter().all(|node| node.members == [node.id]));
    let filtered = DisplayGraph::build(&report, &index, GraphMode::Blocks, Some(9), None, 2);
    assert_eq!(filtered.nodes.len(), 1);
    assert_eq!(filtered.nodes[0].members, [17]);
    assert_eq!(filtered.hidden_states, 7);
}

#[test]
fn folding_keeps_edge_kinds_and_all_native_endpoints_available() {
    let mut report = repeated_states(4);
    report.cfg[2].basic_block = 1;
    report.cfg[3].basic_block = 1;
    report.edges = vec![
        edge(100, 10, 24, EdgeKind::Jump),
        edge(200, 17, 31, EdgeKind::Jump),
        edge(300, 17, 31, EdgeKind::BranchTrue),
        edge(400, 31, 17, EdgeKind::Return),
        edge(500, 9999, 10, EdgeKind::Call),
    ];
    let index = ReportIndex::new(&report);
    let graph = DisplayGraph::build(&report, &index, GraphMode::Blocks, None, None, 2);
    assert_eq!(
        graph.edges,
        [
            edge(100, 10, 24, EdgeKind::Jump),
            edge(300, 10, 24, EdgeKind::BranchTrue),
            edge(400, 24, 10, EdgeKind::Return),
        ]
    );
    assert_eq!(graph.edge_members[&100], [100, 200]);
    assert_eq!(graph.edge_members[&300], [300]);
    for displayed in &graph.edges {
        for native_id in &graph.edge_members[&displayed.id] {
            let native = index.edge(&report, *native_id).unwrap();
            assert_eq!(graph.state_to_node[&native.from], displayed.from);
            assert_eq!(graph.state_to_node[&native.to], displayed.to);
            assert_eq!(native.kind, displayed.kind);
        }
    }
    assert_eq!(graph.boundary_edges, 0);
    let states = DisplayGraph::build(&report, &index, GraphMode::States, None, None, 2);
    assert_eq!(states.edges, report.edges[..4]);
    assert!(states.nodes.iter().all(|node| node.members == [node.id]));
    assert!(
        states
            .edge_members
            .iter()
            .all(|(id, members)| members == &[*id])
    );
}

#[test]
fn local_follows_incoming_and_outgoing_calls_across_program_scope_and_counts_boundary() {
    let mut report = repeated_states(5);
    report.cfg[1].program = Some(9);
    report.cfg[2].program = Some(9);
    report.edges = vec![
        edge(80, 10, 17, EdgeKind::Call),
        edge(90, 17, 24, EdgeKind::Jump),
        edge(100, 24, 31, EdgeKind::Return),
        edge(110, 31, 38, EdgeKind::Jump),
    ];
    let index = ReportIndex::new(&report);
    let local = DisplayGraph::build(&report, &index, GraphMode::Local, Some(0), Some(24), 1);
    assert_eq!(
        local.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        [17, 24, 31]
    );
    assert_eq!(local.edges, report.edges[1..3]);
    assert_eq!((local.hidden_states, local.boundary_edges), (2, 2));
    let isolated = DisplayGraph::build(&report, &index, GraphMode::Local, None, Some(24), 0);
    assert_eq!(isolated.nodes[0].id, 24);
    assert_eq!(isolated.nodes.len(), 1);
    assert_eq!((isolated.hidden_states, isolated.boundary_edges), (4, 2));
    let scoped = DisplayGraph::build(&report, &index, GraphMode::States, Some(9), None, 2);
    assert_eq!(scoped.edges, report.edges[1..2]);
    assert_eq!((scoped.hidden_states, scoped.boundary_edges), (3, 2));
}

#[test]
fn local_caps_dense_neighborhood_deterministically_and_accounts_for_all_omitted_states() {
    let mut report = repeated_states(251);
    report.edges = report.cfg[1..]
        .iter()
        .enumerate()
        .map(|(position, state)| edge(position + 1000, 10, state.id, EdgeKind::Jump))
        .collect();
    let index = ReportIndex::new(&report);
    let graph = DisplayGraph::build(
        &report,
        &index,
        GraphMode::Local,
        None,
        Some(10),
        usize::MAX,
    );
    assert_eq!(graph.nodes.len(), LOCAL_STATE_LIMIT);
    assert_eq!(graph.edges.len(), LOCAL_STATE_LIMIT - 1);
    assert_eq!((graph.hidden_states, graph.boundary_edges), (51, 51));
    let expected: BTreeSet<_> = report.cfg[..LOCAL_STATE_LIMIT]
        .iter()
        .map(|state| state.id)
        .collect();
    assert_eq!(
        graph.state_to_node.keys().copied().collect::<BTreeSet<_>>(),
        expected
    );
    let fallback = DisplayGraph::build(
        &report,
        &index,
        GraphMode::Local,
        None,
        Some(99999),
        usize::MAX,
    );
    assert_eq!(graph.state_to_node, fallback.state_to_node);
    let empty = crate::tests::empty_report();
    let empty_graph = DisplayGraph::build(
        &empty,
        &ReportIndex::new(&empty),
        GraphMode::Local,
        None,
        None,
        2,
    );
    assert!(empty_graph.nodes.is_empty() && empty_graph.edges.is_empty());
}
