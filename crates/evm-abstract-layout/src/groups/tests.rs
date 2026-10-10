use std::collections::BTreeSet;

use super::{capacity, identify};
use crate::{Edge, EdgeKind, Group, GroupId, GroupKind, Input, Label, LayoutError, Node, Size};

fn node(id: usize) -> Node {
    Node {
        id,
        size: Size::new(160.0, 80.0),
        program: Some(0),
        block: id,
        frame_depth: Some(1),
        boundary: false,
    }
}

fn input(ids: &[usize], edges: &[(usize, usize, EdgeKind)]) -> Input {
    Input {
        nodes: ids.iter().copied().map(node).collect(),
        edges: edges
            .iter()
            .enumerate()
            .map(|(id, &(from, to, kind))| Edge {
                id,
                from,
                to,
                kind,
                label: Label {
                    text: format!("e{id}"),
                    size: Size::new(48.0, 16.0),
                },
            })
            .collect(),
    }
}

fn members(groups: &[Group], kind: GroupKind) -> Vec<Vec<usize>> {
    groups
        .iter()
        .filter(|group| group.kind == kind)
        .map(|group| group.members.clone())
        .collect()
}

#[test]
fn empty_graph_and_isolated_leaves_need_no_container() {
    assert!(identify(&Input::default()).unwrap().is_empty());
    assert!(identify(&input(&[7, 91], &[])).unwrap().is_empty());
}

#[test]
fn shortcut_dag_does_not_claim_a_chain_through_a_merge() {
    let graph = input(
        &[0, 7, 91],
        &[
            (0, 7, EdgeKind::BranchTrue),
            (0, 91, EdgeKind::BranchFalse),
            (7, 91, EdgeKind::Jump),
        ],
    );
    assert!(identify(&graph).unwrap().is_empty());
}

#[test]
fn branch_is_a_chain_endpoint_and_its_successors_stay_separate() {
    let graph = input(
        &[0, 1, 2, 3, 4],
        &[
            (0, 1, EdgeKind::Fallthrough),
            (1, 2, EdgeKind::BranchTrue),
            (1, 3, EdgeKind::BranchFalse),
            (2, 4, EdgeKind::Jump),
            (3, 4, EdgeKind::Jump),
        ],
    );
    let before = graph.clone();
    let groups = identify(&graph).unwrap();
    assert_eq!(members(&groups, GroupKind::Chain), [vec![0, 1]]);
    assert_eq!(
        graph, before,
        "grouping must not retarget or merge either branch"
    );
}

#[test]
fn maximal_chain_follows_edges_instead_of_input_adjacency_or_numeric_ids() {
    let graph = input(
        &[usize::MAX, 91, 12, 1],
        &[
            (12, usize::MAX, EdgeKind::Jump),
            (usize::MAX, 1, EdgeKind::Fallthrough),
        ],
    );
    let groups = identify(&graph).unwrap();
    assert_eq!(
        members(&groups, GroupKind::Chain),
        [vec![12, usize::MAX, 1]]
    );
    assert_eq!(groups[0].id, GroupId(0));
    assert_eq!(groups[0].parent, None);
    assert!(!groups[0].members.contains(&91));
}

#[test]
fn program_and_known_frame_depth_boundaries_stop_chains() {
    for change in [0, 1, 2, 3] {
        let mut graph = input(&[7, 91], &[(7, 91, EdgeKind::Fallthrough)]);
        match change {
            0 => graph.nodes[1].program = Some(1),
            1 => graph.nodes[1].frame_depth = Some(2),
            2 => graph.nodes[1].frame_depth = None,
            3 => {
                graph.nodes[0].frame_depth = None;
                graph.nodes[1].frame_depth = None;
            }
            _ => unreachable!(),
        }
        assert!(members(&identify(&graph).unwrap(), GroupKind::Chain).is_empty());
    }
}

#[test]
fn call_return_failure_and_revert_are_not_internal_chain_links() {
    for kind in [
        EdgeKind::Call,
        EdgeKind::Return,
        EdgeKind::Failure,
        EdgeKind::Revert,
    ] {
        let graph = input(&[7, 91], &[(7, 91, kind)]);
        assert!(identify(&graph).unwrap().is_empty(), "{kind:?}");
    }
}

#[test]
fn nonlocal_edges_still_count_toward_full_incoming_and_outgoing_degrees() {
    let graph = input(
        &[0, 1, 2, 3],
        &[
            (0, 1, EdgeKind::Fallthrough),
            (0, 2, EdgeKind::Call),
            (3, 1, EdgeKind::Return),
        ],
    );
    assert!(identify(&graph).unwrap().is_empty());
}

#[test]
fn frontier_or_hidden_incident_edge_marks_a_chain_boundary() {
    for boundary in [0, 1, 2] {
        let mut graph = input(
            &[0, 1, 2],
            &[(0, 1, EdgeKind::Fallthrough), (1, 2, EdgeKind::Fallthrough)],
        );
        graph.nodes[boundary].boundary = true;
        let chains = members(&identify(&graph).unwrap(), GroupKind::Chain);
        match boundary {
            0 => assert_eq!(chains, [vec![1, 2]]),
            1 => assert!(chains.is_empty()),
            2 => assert_eq!(chains, [vec![0, 1]]),
            _ => unreachable!(),
        }
        assert!(chains.iter().all(|chain| !chain.contains(&boundary)));
    }
}

#[test]
fn ordinary_cycle_members_are_not_also_chained_to_each_other_or_exits() {
    let graph = input(
        &[0, 1, 2, 3],
        &[
            (0, 1, EdgeKind::Jump),
            (1, 0, EdgeKind::BranchTrue),
            (1, 2, EdgeKind::BranchFalse),
            (2, 3, EdgeKind::Fallthrough),
        ],
    );
    let groups = identify(&graph).unwrap();
    assert_eq!(members(&groups, GroupKind::Cycle), [vec![0, 1]]);
    assert_eq!(members(&groups, GroupKind::Chain), [vec![2, 3]]);
}

#[test]
fn ordinary_self_loop_gets_a_cycle_container_without_losing_its_edge() {
    let graph = input(
        &[usize::MAX, 91],
        &[
            (usize::MAX, usize::MAX, EdgeKind::Jump),
            (usize::MAX, 91, EdgeKind::Fallthrough),
        ],
    );
    let before = graph.clone();
    let groups = identify(&graph).unwrap();
    assert_eq!(members(&groups, GroupKind::Cycle), [vec![usize::MAX]]);
    assert!(members(&groups, GroupKind::Chain).is_empty());
    assert_eq!(graph, before);
}

#[test]
fn a_call_and_return_cycle_is_not_a_program_local_cycle() {
    let graph = input(&[0, 1], &[(0, 1, EdgeKind::Call), (1, 0, EdgeKind::Return)]);
    assert!(identify(&graph).unwrap().is_empty());
    let mut graph = input(&[0, 1], &[(0, 1, EdgeKind::Jump), (1, 0, EdgeKind::Jump)]);
    graph.nodes[1].program = Some(1);
    assert!(identify(&graph).unwrap().is_empty());
}

#[test]
fn parallel_original_edges_are_not_deduplicated_into_a_false_chain() {
    let graph = input(
        &[0, 1],
        &[(0, 1, EdgeKind::BranchTrue), (0, 1, EdgeKind::BranchFalse)],
    );
    assert!(identify(&graph).unwrap().is_empty());
}

#[test]
fn program_wrappers_precede_children_and_groups_form_a_laminar_hierarchy() {
    let mut graph = input(
        &[0, 1, 2, 3, 4],
        &[
            (0, 1, EdgeKind::Jump),
            (1, 0, EdgeKind::Jump),
            (2, 3, EdgeKind::Fallthrough),
            (3, 4, EdgeKind::Jump),
        ],
    );
    for node in &mut graph.nodes[2..] {
        node.program = Some(usize::MAX);
    }
    let groups = identify(&graph).unwrap();
    assert_eq!(groups.len(), 4);
    assert_eq!(groups[0].kind, GroupKind::Program(0));
    assert_eq!(groups[0].members, [0, 1]);
    assert_eq!(groups[1].kind, GroupKind::Program(usize::MAX));
    assert_eq!(groups[1].members, [2, 3, 4]);
    assert_eq!(groups[2].parent, Some(groups[0].id));
    assert_eq!(groups[3].parent, Some(groups[1].id));
    for (position, group) in groups.iter().enumerate() {
        assert_eq!(group.id, GroupId(position));
        if let Some(parent) = group.parent {
            assert!(parent.0 < position);
            assert!(
                group
                    .members
                    .iter()
                    .all(|member| groups[parent.0].members.contains(member))
            );
        }
        let members: BTreeSet<_> = group.members.iter().copied().collect();
        assert_eq!(members.len(), group.members.len());
        for other in &groups[..position] {
            let other: BTreeSet<_> = other.members.iter().copied().collect();
            assert!(
                members.is_disjoint(&other)
                    || members.is_subset(&other)
                    || other.is_subset(&members)
            );
        }
    }
    assert_eq!(
        groups,
        identify(&graph).unwrap(),
        "same input produces stable spatial identities"
    );
}

#[test]
fn a_single_program_or_single_leaf_program_does_not_get_an_outer_frame() {
    let single = input(&[0, 1], &[(0, 1, EdgeKind::Jump)]);
    assert_eq!(identify(&single).unwrap().len(), 1);
    let mut mixed = input(&[0, 1, 2], &[(0, 1, EdgeKind::Jump)]);
    mixed.nodes[2].program = Some(1);
    let groups = identify(&mixed).unwrap();
    assert_eq!(members(&groups, GroupKind::Program(0)), [vec![0, 1]]);
    assert!(members(&groups, GroupKind::Program(1)).is_empty());
    mixed.nodes[2].program = None;
    assert!(members(&identify(&mixed).unwrap(), GroupKind::Program(0)).is_empty());
}

#[test]
fn identities_and_endpoints_are_validated_before_grouping() {
    let mut graph = input(&[usize::MAX, 91], &[(usize::MAX, 91, EdgeKind::Jump)]);
    graph.nodes.push(graph.nodes[0].clone());
    assert!(matches!(
        identify(&graph),
        Err(LayoutError::DuplicateNode(usize::MAX))
    ));
    graph.nodes.pop();
    graph.edges.push(graph.edges[0].clone());
    assert!(matches!(
        identify(&graph),
        Err(LayoutError::DuplicateEdge(0))
    ));
    graph.edges.pop();
    graph.edges[0].from = 92;
    assert!(matches!(
        identify(&graph),
        Err(LayoutError::MissingNode { edge: 0, node: 92 })
    ));
    graph.edges[0].from = usize::MAX;
    graph.edges[0].to = 93;
    assert!(matches!(
        identify(&graph),
        Err(LayoutError::MissingNode { edge: 0, node: 93 })
    ));
}

#[test]
fn card_and_label_dimensions_must_be_finite_and_positive() {
    for size in [
        Size::new(0.0, 80.0),
        Size::new(160.0, -1.0),
        Size::new(f64::NAN, 80.0),
        Size::new(160.0, f64::INFINITY),
        Size::new(f64::NEG_INFINITY, 80.0),
    ] {
        let mut graph = input(&[7, 91], &[(7, 91, EdgeKind::Jump)]);
        graph.nodes[0].size = size;
        assert!(matches!(
            identify(&graph),
            Err(LayoutError::InvalidSize {
                element: "node",
                id: 7
            })
        ));
        graph.nodes[0].size = Size::new(160.0, 80.0);
        graph.edges[0].label.size = size;
        assert!(matches!(
            identify(&graph),
            Err(LayoutError::InvalidSize {
                element: "edge label",
                id: 0
            })
        ));
    }
}

#[test]
fn arena_capacity_counts_root_containers_and_group_labels_without_wrapping() {
    assert!(capacity(100, 200, 10).is_ok());
    assert!(matches!(
        capacity(u32::MAX as usize, 0, 0),
        Err(LayoutError::TooLarge)
    ));
    assert!(matches!(
        capacity(0, u32::MAX as usize, 1),
        Err(LayoutError::TooLarge)
    ));
    assert!(matches!(
        capacity(usize::MAX, 0, 0),
        Err(LayoutError::TooLarge)
    ));
}

#[test]
fn very_long_chains_and_cycles_do_not_use_recursive_dfs() {
    let count = 20_000;
    let ids: Vec<_> = (0..count).collect();
    let edges: Vec<_> = (1..count)
        .map(|id| (id - 1, id, EdgeKind::Fallthrough))
        .collect();
    let mut graph = input(&ids, &edges);
    let groups = identify(&graph).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, GroupKind::Chain);
    assert_eq!(groups[0].members, ids);
    graph.edges.push(Edge {
        id: count,
        from: count - 1,
        to: 0,
        kind: EdgeKind::Jump,
        label: Label {
            text: "back".into(),
            size: Size::new(48.0, 16.0),
        },
    });
    let groups = identify(&graph).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, GroupKind::Cycle);
    assert_eq!(groups[0].members, ids);
}
