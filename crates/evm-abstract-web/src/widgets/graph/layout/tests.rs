use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2};
use evm_abstract_protocol::{AnalysisReport, CfgEdge, EdgeKind};

use super::{Placement, for_report};

fn fixture(
    nodes: &[(usize, Vec2)],
    edges: &[(usize, usize, usize, EdgeKind)],
) -> (AnalysisReport, BTreeMap<usize, Vec2>) {
    let mut report = crate::tests::report();
    let template = report.cfg[0].clone();
    report.cfg = nodes
        .iter()
        .map(|(id, _)| {
            let mut node = template.clone();
            node.id = *id;
            node.basic_block = *id;
            node
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
    (report, nodes.iter().copied().collect())
}

fn boundary(p: Pos2, r: Rect) -> bool {
    r.expand(0.01).contains(p)
        && [
            (p.x - r.left()).abs(),
            (p.x - r.right()).abs(),
            (p.y - r.top()).abs(),
            (p.y - r.bottom()).abs(),
        ]
        .into_iter()
        .any(|distance| distance <= 0.01)
}

fn enters(r: Rect, a: Pos2, b: Pos2) -> bool {
    if (a.x - b.x).abs() < 0.01 {
        r.left() + 0.01 < a.x
            && a.x < r.right() - 0.01
            && a.y.min(b.y) < r.bottom() - 0.01
            && a.y.max(b.y) > r.top() + 0.01
    } else {
        r.top() + 0.01 < a.y
            && a.y < r.bottom() - 0.01
            && a.x.min(b.x) < r.right() - 0.01
            && a.x.max(b.x) > r.left() + 0.01
    }
}

fn validate(report: &AnalysisReport, placement: &Placement) {
    for (id, rect) in &placement.nodes {
        assert!(rect.is_finite() && placement.bounds.contains_rect(*rect));
        for (other_id, other) in &placement.nodes {
            if id != other_id {
                assert!(!rect.intersects(*other), "cards overlap {id}/{other_id}");
            }
        }
    }
    for edge in &report.edges {
        if !placement.nodes.contains_key(&edge.from) || !placement.nodes.contains_key(&edge.to) {
            continue;
        }
        let route = &placement.edges[&edge.id];
        assert!(route.path.len() >= 2);
        assert!(boundary(route.path[0], placement.nodes[&edge.from]));
        assert!(boundary(
            *route.path.last().unwrap(),
            placement.nodes[&edge.to]
        ));
        assert!(placement.bounds.contains_rect(route.label));
        for node in placement.nodes.values() {
            assert!(
                !node.intersects(route.label),
                "edge {} label overlaps a card",
                edge.id
            );
        }
        for segment in route.path.windows(2) {
            assert_ne!(segment[0], segment[1]);
            assert!(segment[0].x == segment[1].x || segment[0].y == segment[1].y);
            for p in segment {
                assert!(p.is_finite() && placement.bounds.expand(0.01).contains(*p));
            }
            for (id, rect) in &placement.nodes {
                if *id != edge.from && *id != edge.to {
                    assert!(
                        !enters(*rect, segment[0], segment[1]),
                        "edge {} enters card {id}",
                        edge.id
                    );
                }
            }
        }
    }
    for group in &placement.groups {
        assert!(placement.bounds.contains_rect(group.bounds));
        assert!(placement.bounds.contains_rect(group.caption));
        for id in &group.members {
            assert!(group.bounds.expand(0.01).contains_rect(placement.nodes[id]));
        }
        for node in placement.nodes.values() {
            assert!(!node.intersects(group.caption));
        }
    }
}

#[test]
fn unequal_diamond_cards_and_joint_labels_keep_collision_free_geometry() {
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
    let placement = for_report(&report, &sizes);
    assert_eq!(placement.nodes.len(), 4);
    assert_eq!(placement.edges.len(), 4);
    validate(&report, &placement);
}

#[test]
fn shortcut_label_and_route_stay_between_a_and_b_instead_of_false_back_routing() {
    let (report, sizes) = fixture(
        &[
            (7, Vec2::new(140.0, 60.0)),
            (42, Vec2::new(140.0, 80.0)),
            (91, Vec2::new(100.0, 70.0)),
        ],
        &[
            (3, 7, 42, EdgeKind::Jump),
            (4, 7, 91, EdgeKind::BranchFalse),
            (5, 42, 91, EdgeKind::Jump),
        ],
    );
    let placement = for_report(&report, &sizes);
    validate(&report, &placement);
    let a = placement.nodes[&42];
    let b = placement.nodes[&91];
    let q = &placement.edges[&5];
    assert!(a.bottom() < b.top());
    assert!(q.label.top() >= a.bottom() && q.label.bottom() <= b.top());
    assert!(q.path.iter().all(|p| p.y >= a.bottom() && p.y <= b.top()));
}

#[test]
fn cycles_self_edges_and_parallel_edges_keep_original_targets_and_visible_labels() {
    let (report, sizes) = fixture(
        &[
            (7, Vec2::new(140.0, 80.0)),
            (42, Vec2::new(160.0, 60.0)),
            (91, Vec2::new(120.0, 90.0)),
        ],
        &[
            (3, 7, 42, EdgeKind::Jump),
            (4, 42, 91, EdgeKind::Fallthrough),
            (5, 91, 7, EdgeKind::BranchTrue),
            (6, 42, 42, EdgeKind::Jump),
            (7, 7, 42, EdgeKind::BranchFalse),
        ],
    );
    let placement = for_report(&report, &sizes);
    validate(&report, &placement);
    assert_eq!(placement.edges.len(), 5);
    assert_ne!(placement.edges[&3].path, placement.edges[&7].path);
}

#[test]
fn long_sparse_ids_and_dangling_edges_do_not_create_phantom_leaves_or_labels() {
    let (report, sizes) = fixture(
        &[
            (usize::MAX - 7, Vec2::new(140.0, 80.0)),
            (usize::MAX, Vec2::new(140.0, 80.0)),
        ],
        &[
            (usize::MAX, usize::MAX - 7, usize::MAX, EdgeKind::Jump),
            (12, usize::MAX, 99, EdgeKind::Jump),
        ],
    );
    let placement = for_report(&report, &sizes);
    assert_eq!(placement.nodes.len(), 2);
    assert_eq!(placement.edges.len(), 1);
    assert!(placement.edges.contains_key(&usize::MAX));
    validate(&report, &placement);
}

#[test]
fn disconnected_components_and_same_input_have_deterministic_geometry() {
    let (report, sizes) = fixture(
        &[
            (1, Vec2::new(140.0, 80.0)),
            (8, Vec2::new(140.0, 80.0)),
            (19, Vec2::new(140.0, 80.0)),
            (101, Vec2::new(140.0, 80.0)),
        ],
        &[(2, 1, 8, EdgeKind::Jump), (3, 19, 101, EdgeKind::Jump)],
    );
    let first = for_report(&report, &sizes);
    validate(&report, &first);
    assert_eq!(first, for_report(&report, &sizes));
}

#[test]
fn empty_report_has_no_geometry_or_placeholder_labels() {
    let (report, sizes) = fixture(&[], &[]);
    let placement = for_report(&report, &sizes);
    assert!(
        placement.nodes.is_empty() && placement.edges.is_empty() && placement.groups.is_empty()
    );
    assert_eq!(placement.bounds, Rect::NOTHING);
}
