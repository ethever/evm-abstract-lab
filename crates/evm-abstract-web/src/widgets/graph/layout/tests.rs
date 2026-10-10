use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2};
use evm_abstract_protocol::{AnalysisReport, CfgEdge, EdgeKind};

use super::{
    Expanded, Flow, Placement, VertexId, adaptive, adaptive_scene, arrange, candidate_capacities,
    label_size,
};

fn fixture(
    states: &[(usize, Vec2)],
    edges: &[(usize, usize, usize, EdgeKind)],
) -> (AnalysisReport, BTreeMap<usize, Vec2>) {
    let mut report = crate::tests::report();
    let template = report.cfg[0].clone();
    report.cfg = states
        .iter()
        .map(|(id, _)| {
            let mut state = template.clone();
            state.id = *id;
            state.basic_block = *id;
            state
        })
        .collect();
    report.edges = edges
        .iter()
        .map(|(id, from, to, kind)| CfgEdge {
            id: *id,
            from: *from,
            to: *to,
            kind: *kind,
        })
        .collect();
    (report, states.iter().copied().collect())
}

fn boundary(point: Pos2, rect: Rect) -> bool {
    rect.expand(0.001).contains(point)
        && [
            (point.x - rect.left()).abs(),
            (point.x - rect.right()).abs(),
            (point.y - rect.top()).abs(),
            (point.y - rect.bottom()).abs(),
        ]
        .into_iter()
        .any(|distance| distance < 0.001)
}

fn segment_enters(rect: Rect, from: Pos2, to: Pos2) -> bool {
    let overlap = rect.intersect(Rect::from_two_pos(from, to));
    if from.x == to.x {
        overlap.height() > 0.0 && rect.left() < from.x && from.x < rect.right()
    } else {
        overlap.width() > 0.0 && rect.top() < from.y && from.y < rect.bottom()
    }
}

fn validate(report: &AnalysisReport, placed: &Placement) {
    let boxes: Vec<_> = placed
        .nodes
        .iter()
        .map(|(id, rect)| (VertexId::State(*id), *rect))
        .chain(
            placed
                .edges
                .iter()
                .map(|(id, route)| (VertexId::EdgeLabel(*id), route.label)),
        )
        .collect();
    for (index, (id, rect)) in boxes.iter().enumerate() {
        assert!(
            placed.bounds.contains_rect(*rect),
            "{id:?} excluded from bounds"
        );
        for (other_id, other) in &boxes[index + 1..] {
            let overlap = rect.intersect(*other);
            assert!(
                overlap.width() <= 0.0 || overlap.height() <= 0.0,
                "{id:?} {rect:?} overlaps {other_id:?} {other:?}"
            );
        }
    }
    for edge in &report.edges {
        let Some(route) = placed.edges.get(&edge.id) else {
            assert!(!placed.nodes.contains_key(&edge.from) || !placed.nodes.contains_key(&edge.to));
            continue;
        };
        for (path, source, target) in [
            (
                &route.to_label,
                VertexId::State(edge.from),
                VertexId::EdgeLabel(edge.id),
            ),
            (
                &route.to_target,
                VertexId::EdgeLabel(edge.id),
                VertexId::State(edge.to),
            ),
        ] {
            assert!(path.len() >= 2);
            let from_rect = boxes.iter().find(|(id, _)| *id == source).unwrap().1;
            let to_rect = boxes.iter().find(|(id, _)| *id == target).unwrap().1;
            assert!(
                boundary(path[0], from_rect),
                "path does not leave its own source boundary"
            );
            assert!(
                boundary(*path.last().unwrap(), to_rect),
                "path does not reach its own target boundary"
            );
            for pair in path.windows(2) {
                assert_ne!(pair[0], pair[1]);
                assert!(
                    pair[0].x == pair[1].x || pair[0].y == pair[1].y,
                    "non-orthogonal path"
                );
                assert!(
                    pair.iter()
                        .all(|point| point.is_finite() && placed.bounds.contains(*point))
                );
                for (id, rect) in &boxes {
                    if *id != source && *id != target {
                        assert!(
                            !segment_enters(*rect, pair[0], pair[1]),
                            "edge {} {source:?}->{target:?} crosses {id:?} {rect:?}: {pair:?}",
                            edge.id
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unequal_diamond_nodes_and_label_vertices_share_collision_free_layers() {
    let (report, sizes) = fixture(
        &[
            (7, Vec2::new(170.0, 180.0)),
            (42, Vec2::new(300.0, 55.0)),
            (91, Vec2::new(125.0, 205.0)),
            (205, Vec2::new(270.0, 100.0)),
        ],
        &[
            (7, 7, 42, EdgeKind::BranchTrue),
            (8, 7, 91, EdgeKind::BranchFalse),
            (9, 42, 205, EdgeKind::Jump),
            (10, 91, 205, EdgeKind::Fallthrough),
        ],
    );
    let expanded = Expanded::new(&report, &sizes);
    assert!(expanded.sizes.contains_key(&VertexId::State(7)));
    assert!(
        expanded.sizes.contains_key(&VertexId::EdgeLabel(7)),
        "edge IDs must not collide with native state IDs"
    );
    for flow in [Flow::Down, Flow::Right] {
        for capacity in [1, 2, 4] {
            let placed = arrange(&expanded, capacity, flow);
            assert_eq!(placed.nodes.len(), 4);
            assert_eq!(placed.edges.len(), 4);
            validate(&report, &placed);
        }
    }
}

#[test]
fn long_ids_dense_fanout_and_dangling_edges_keep_one_label_per_real_edge() {
    let states: Vec<_> = (0..100)
        .map(|id| {
            (
                id * 17,
                Vec2::new(
                    110.0 + (id % 4) as f32 * 30.0,
                    55.0 + (id % 3) as f32 * 25.0,
                ),
            )
        })
        .collect();
    let mut edges: Vec<_> = (1..100)
        .map(|id| (usize::MAX - id, 0, id * 17, EdgeKind::Jump))
        .collect();
    for id in 1..99 {
        edges.push((id, id * 17, (id + 1) * 17, EdgeKind::Fallthrough));
    }
    edges.push((1000, 99999, 0, EdgeKind::Call));
    edges.push((1001, 0, 99999, EdgeKind::Return));
    let (report, sizes) = fixture(&states, &edges);
    let first = adaptive(&report, &sizes, Vec2::new(1440.0, 900.0));
    let second = adaptive(&report, &sizes, Vec2::new(1440.0, 900.0));
    assert_eq!(first.nodes, second.nodes);
    assert_eq!(first.edges.len(), 197);
    for (id, route) in &first.edges {
        let other = &second.edges[id];
        assert_eq!(route.label, other.label);
        assert_eq!(route.to_label, other.to_label);
        assert_eq!(route.to_target, other.to_target);
        let kind = report
            .edges
            .iter()
            .find(|edge| edge.id == *id)
            .unwrap()
            .kind;
        assert_eq!(route.label.size(), label_size(*id, kind));
    }
    assert!(!first.edges.contains_key(&1000) && !first.edges.contains_key(&1001));
    validate(&report, &first);
}

#[test]
fn self_back_same_rank_and_skipping_edges_route_around_all_unrelated_vertices() {
    let (report, sizes) = fixture(
        &[
            (0, Vec2::new(140.0, 95.0)),
            (7, Vec2::new(180.0, 170.0)),
            (14, Vec2::new(110.0, 60.0)),
            (21, Vec2::new(220.0, 110.0)),
            (28, Vec2::new(170.0, 75.0)),
        ],
        &[
            (0, 0, 7, EdgeKind::BranchTrue),
            (1, 0, 14, EdgeKind::BranchFalse),
            (2, 7, 21, EdgeKind::Fallthrough),
            (3, 21, 28, EdgeKind::Jump),
            (4, 7, 7, EdgeKind::Jump),
            (5, 21, 0, EdgeKind::Jump),
            (6, 14, 7, EdgeKind::Jump),
            (7, 0, 28, EdgeKind::Call),
        ],
    );
    let expanded = Expanded::new(&report, &sizes);
    for flow in [Flow::Down, Flow::Right] {
        for capacity in [1, 3, 12] {
            let placed = arrange(&expanded, capacity, flow);
            validate(&report, &placed);
            let all_boxes = placed
                .nodes
                .values()
                .copied()
                .chain(placed.edges.values().map(|edge| edge.label))
                .fold(Rect::NOTHING, |bounds, rect| bounds.union(rect));
            for id in [4, 5, 6] {
                assert!(
                    placed.edges[&id].to_target.iter().any(|point| match flow {
                        Flow::Down => point.x > all_boxes.right(),
                        Flow::Right => point.y > all_boxes.bottom(),
                    }),
                    "self/back/same-rank edge {id} must use the perimeter"
                );
            }
        }
    }
}

#[test]
fn projected_edge_labels_reserve_the_supplied_text_and_connect_visible_representatives() {
    let (report, sizes) = fixture(
        &[(7, Vec2::new(150.0, 80.0)), (91, Vec2::new(210.0, 125.0))],
        &[(300, 7, 91, EdgeKind::Jump), (400, 91, 7, EdgeKind::Return)],
    );
    let ids = [7, 91];
    let labels = BTreeMap::from([
        (300, "jump (129 contexts)".into()),
        (400, "return (2048)".into()),
    ]);
    let expanded = Expanded::scene(&ids, &report.edges, &sizes, &labels);
    for flow in [Flow::Down, Flow::Right] {
        let placed = arrange(&expanded, 2, flow);
        for edge in &report.edges {
            assert_eq!(
                placed.edges[&edge.id].label.width(),
                labels[&edge.id].len() as f32 * 6.0 + 6.0
            );
        }
        validate(&report, &placed);
    }
    let placed = adaptive_scene(
        &ids,
        &report.edges,
        &sizes,
        &labels,
        Vec2::new(844.0, 390.0),
    );
    assert_eq!(placed.nodes.keys().copied().collect::<Vec<_>>(), ids);
    validate(&report, &placed);
}

#[test]
fn candidate_search_is_exhaustive_for_small_scenes_and_bounded_for_large_state_views() {
    let states: Vec<_> = (0..130).map(|id| (id, Vec2::new(170.0, 80.0))).collect();
    let edges: Vec<_> = (1..130).map(|id| (id, 0, id, EdgeKind::Jump)).collect();
    let (report, sizes) = fixture(&states, &edges);
    let large = Expanded::new(&report, &sizes);
    assert_eq!(large.sizes.len(), 259);
    for viewport in [Vec2::new(320.0, 800.0), Vec2::new(1400.0, 800.0)] {
        for flow in [Flow::Down, Flow::Right] {
            let capacities = candidate_capacities(&large, viewport, flow);
            assert_eq!(
                capacities.len(),
                1,
                "large scenes should route one candidate per orientation"
            );
            assert!((1..=12).contains(&capacities[0]));
        }
    }
    let (small_report, small_sizes) = fixture(&states[..10], &edges[..9]);
    let small = Expanded::new(&small_report, &small_sizes);
    for flow in [Flow::Down, Flow::Right] {
        assert_eq!(
            candidate_capacities(&small, Vec2::new(1400.0, 800.0), flow),
            (1..=9).collect::<Vec<_>>()
        );
    }
}

#[test]
fn large_wide_scene_uses_multiple_columns_without_exhaustive_search() {
    let states: Vec<_> = (0..300).map(|id| (id, Vec2::new(170.0, 80.0))).collect();
    // Disconnected source blocks isolate packing density from edge routing.
    let (report, sizes) = fixture(&states, &[]);
    let graph = Expanded::new(&report, &sizes);
    let wide = Vec2::new(1400.0, 800.0);
    let narrow = Vec2::new(320.0, 800.0);
    let down = candidate_capacities(&graph, wide, Flow::Down);
    let right = candidate_capacities(&graph, wide, Flow::Right);
    assert_eq!(down.len(), 1);
    assert_eq!(right.len(), 1);
    assert!(down[0] > 1 && right[0] > 1);
    assert!(down[0] > candidate_capacities(&graph, narrow, Flow::Down)[0]);
    let packed = arrange(&graph, down[0], Flow::Down);
    let single_column = arrange(&graph, 1, Flow::Down);
    assert_eq!(packed.nodes.len(), states.len());
    assert!(packed.bounds.height() < single_column.bounds.height() / 2.0);
    assert!(packed.bounds.width() <= wide.x);
    let fitted = adaptive(&report, &sizes, wide);
    assert_eq!(fitted.nodes.len(), states.len());
    assert!(fitted.bounds.is_finite());
    assert!(
        super::fit_scale(fitted.bounds.size(), wide)
            > super::fit_scale(single_column.bounds.size(), wide)
    );
}
