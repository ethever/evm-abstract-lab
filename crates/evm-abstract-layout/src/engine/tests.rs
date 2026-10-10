use std::collections::BTreeMap;

use elkrs::graph::graph::{ElkGraph, ShapeId};

use super::{
    contains, edge_path, layout, on_border, overlap, place_captions, same_size, segment_hits,
    valid_rect,
};
use crate::{
    Edge, EdgeGeometry, EdgeKind, Group, GroupGeometry, GroupId, GroupKind, Input, Label, Layout,
    Node, Point, Rect, Size,
};

fn node(id: usize, program: usize) -> Node {
    Node {
        id,
        size: Size::new(120.0, 70.0),
        program: Some(program),
        block: id,
        frame_depth: Some(0),
        boundary: false,
    }
}
fn edge(id: usize, from: usize, to: usize, kind: EdgeKind) -> Edge {
    Edge {
        id,
        from,
        to,
        kind,
        label: Label {
            text: format!("e{id}"),
            size: Size::new(48.0, 18.0),
        },
    }
}
fn assert_geometry(input: &Input, output: &Layout) {
    assert_eq!(output.nodes.len(), input.nodes.len());
    assert_eq!(output.edges.len(), input.edges.len());
    let bounds = output.bounds.unwrap();
    assert!(valid_rect(bounds));
    for node in &input.nodes {
        let rect = output.nodes[&node.id];
        assert!(valid_rect(rect) && same_size(node.size, rect.size));
        assert!(contains(bounds, rect));
    }
    for edge in &input.edges {
        let geometry = &output.edges[&edge.id];
        assert_eq!(
            (geometry.from, geometry.to, geometry.kind),
            (edge.from, edge.to, edge.kind)
        );
        assert!(on_border(output.nodes[&edge.from], geometry.path[0]));
        assert!(on_border(
            output.nodes[&edge.to],
            *geometry.path.last().unwrap()
        ));
        assert!(valid_rect(geometry.label) && same_size(edge.label.size, geometry.label.size));
        for node in output.nodes.values() {
            assert!(
                !overlap(*node, geometry.label),
                "edge {} label covers a node",
                edge.id
            );
            let interior = Rect::new(
                Point::new(node.origin.x + 1e-4, node.origin.y + 1e-4),
                Size::new(node.size.width - 2e-4, node.size.height - 2e-4),
            );
            for segment in geometry.path.windows(2) {
                assert!(
                    !segment_hits(interior, segment[0], segment[1]),
                    "edge {} crosses a node: {segment:?} {node:?}",
                    edge.id
                );
            }
        }
    }
    for group in &output.groups {
        assert!(contains(bounds, group.bounds));
        assert!(contains(bounds, group.caption));
        for member in &group.group.members {
            assert!(contains(group.bounds, output.nodes[member]));
        }
        for edge in output.edges.values() {
            assert!(!overlap(group.caption, edge.label));
            for segment in edge.path.windows(2) {
                assert!(
                    !segment_hits(group.caption, segment[0], segment[1]),
                    "caption {} crosses an edge",
                    group.title
                );
            }
        }
        assert!(
            output
                .nodes
                .values()
                .all(|node| !overlap(group.caption, *node))
        );
    }
}

#[test]
fn shortcut_labels_and_routes_avoid_the_middle_card() {
    let input = Input {
        nodes: vec![node(usize::MAX, 0), node(19, 0), node(301, 0)],
        edges: vec![
            edge(usize::MAX, usize::MAX, 19, EdgeKind::BranchTrue),
            edge(79, usize::MAX, 301, EdgeKind::BranchFalse),
            edge(111, 19, 301, EdgeKind::Jump),
        ],
    };
    let result = layout(&input, &[]).unwrap();
    assert_geometry(&input, &result);
    assert!(
        result.edges[&79].path.len() >= 4,
        "a long shortcut must route around its intermediate card"
    );
    assert_eq!(
        result,
        layout(&input, &[]).unwrap(),
        "fixed seed and policy must produce deterministic geometry"
    );
}

#[test]
fn nested_program_groups_keep_cross_group_leaf_endpoints_and_safe_captions() {
    let input = Input {
        nodes: vec![node(91, 0), node(412, 0), node(733, 1), node(usize::MAX, 1)],
        edges: vec![
            edge(31, 91, 412, EdgeKind::Jump),
            edge(39, 733, usize::MAX, EdgeKind::Jump),
            edge(121, 412, 733, EdgeKind::Call),
            edge(191, usize::MAX, 91, EdgeKind::Return),
        ],
    };
    let groups = vec![
        Group {
            id: GroupId(0),
            parent: None,
            kind: GroupKind::Program(0),
            members: vec![91, 412],
        },
        Group {
            id: GroupId(1),
            parent: Some(GroupId(0)),
            kind: GroupKind::Chain,
            members: vec![91, 412],
        },
        Group {
            id: GroupId(2),
            parent: None,
            kind: GroupKind::Program(1),
            members: vec![733, usize::MAX],
        },
        Group {
            id: GroupId(3),
            parent: Some(GroupId(2)),
            kind: GroupKind::Chain,
            members: vec![733, usize::MAX],
        },
    ];
    let result = layout(&input, &groups).unwrap();
    assert_geometry(&input, &result);
    assert_eq!(result.groups[0].title, "P₀");
    assert_eq!(result.groups[1].title, "Chain");
    assert!(contains(result.groups[0].bounds, result.groups[1].bounds));
    assert!(contains(result.groups[2].bounds, result.groups[3].bounds));
    assert!(!overlap(result.groups[0].bounds, result.groups[2].bounds));
}

#[test]
fn cycles_parallel_edges_and_self_loops_keep_separate_original_routes() {
    let input = Input {
        nodes: vec![node(17, 7), node(99, 7), node(401, 7)],
        edges: vec![
            edge(31, 17, 99, EdgeKind::BranchTrue),
            edge(81, 17, 99, EdgeKind::BranchFalse),
            edge(301, 99, 401, EdgeKind::Jump),
            edge(331, 401, 17, EdgeKind::Jump),
            edge(591, 99, 99, EdgeKind::Jump),
        ],
    };
    let groups = vec![Group {
        id: GroupId(0),
        parent: None,
        kind: GroupKind::Cycle,
        members: vec![17, 99, 401],
    }];
    let result = layout(&input, &groups).unwrap();
    assert_geometry(&input, &result);
    assert_ne!(result.edges[&31].path, result.edges[&81].path);
    assert!(result.edges[&591].path.len() >= 4);
}

#[test]
fn full_measured_cards_and_labels_survive_a_long_chain() {
    let input = Input {
        nodes: (0..128).map(|id| node(id * 17, 0)).collect(),
        edges: (0..127)
            .map(|id| edge(id * 31, id * 17, (id + 1) * 17, EdgeKind::Jump))
            .collect(),
    };
    let groups = vec![Group {
        id: GroupId(0),
        parent: None,
        kind: GroupKind::Chain,
        members: input.nodes.iter().map(|node| node.id).collect(),
    }];
    assert_geometry(&input, &layout(&input, &groups).unwrap());
}

#[test]
fn unordered_cross_hierarchy_sections_are_stitched_without_losing_a_segment() {
    let mut graph = ElkGraph::new();
    let root = graph.root;
    let a = graph.create_node(Some(root));
    let b = graph.create_node(Some(root));
    let edge = graph.create_simple_edge(ShapeId::Node(a), ShapeId::Node(b));
    let first = graph.create_section(edge);
    graph.section_mut(first).set_start_location(40.0, 60.0);
    graph.section_mut(first).set_end_location(40.0, 100.0);
    let second = graph.create_section(edge);
    graph.section_mut(second).set_start_location(40.0, 100.0);
    graph.section_mut(second).set_end_location(40.0, 200.0);
    graph.edge_mut(edge).sections.reverse();
    let path = edge_path(
        &graph,
        edge,
        Point::new(10.0, 20.0),
        Rect::new(Point::new(10.0, 20.0), Size::new(80.0, 60.0)),
        Rect::new(Point::new(10.0, 220.0), Size::new(80.0, 60.0)),
    )
    .unwrap();
    assert_eq!(
        path,
        vec![
            Point::new(50.0, 80.0),
            Point::new(50.0, 120.0),
            Point::new(50.0, 220.0)
        ]
    );
    graph.section_mut(second).set_start_location(41.0, 100.0);
    assert_eq!(
        edge_path(
            &graph,
            edge,
            Point::new(10.0, 20.0),
            Rect::new(Point::new(10.0, 20.0), Size::new(80.0, 60.0)),
            Rect::new(Point::new(10.0, 220.0), Size::new(80.0, 60.0))
        ),
        Err("disconnected or branching route sections")
    );
}

#[test]
fn crowded_headers_move_annotations_without_rewriting_control_flow() {
    let mut output = Layout {
        nodes: BTreeMap::new(),
        edges: BTreeMap::from([(
            9,
            EdgeGeometry {
                from: 1,
                to: 2,
                kind: EdgeKind::Call,
                path: vec![Point::new(0.0, 12.0), Point::new(200.0, 12.0)],
                label: Rect::new(Point::new(0.0, 100.0), Size::new(20.0, 18.0)),
            },
        )]),
        groups: vec![GroupGeometry {
            group: Group {
                id: GroupId(7),
                parent: None,
                kind: GroupKind::Cycle,
                members: vec![],
            },
            bounds: Rect::new(Point::new(0.0, 0.0), Size::new(200.0, 140.0)),
            caption: Rect::new(Point::new(6.0, 6.0), Size::new(30.0, 14.0)),
            title: "Cycle".into(),
        }],
        bounds: Some(Rect::new(Point::new(0.0, 0.0), Size::new(200.0, 140.0))),
    };
    let edge = output.edges[&9].clone();
    place_captions(&mut output).unwrap();
    assert_eq!(output.edges[&9], edge);
    let caption = output.groups[0].caption;
    assert!(caption.right() < 0.0);
    assert!(contains(output.bounds.unwrap(), caption));
    assert!(!segment_hits(caption, edge.path[0], edge.path[1]));
}

#[test]
fn empty_scene_has_no_synthetic_geometry() {
    assert_eq!(layout(&Input::default(), &[]).unwrap(), Layout::default());
}
