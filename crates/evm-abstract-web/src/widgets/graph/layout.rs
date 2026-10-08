//! Deterministic rank packing. Candidate row widths are compared against the
//! actual viewport, including routed edge and label bounds, before fitting.

use egui::{Pos2, Rect, Vec2};
use evm_abstract_protocol::{AnalysisReport, EdgeKind};
use std::collections::{BTreeMap, VecDeque};

const COLUMN_GAP: f32 = 22.0;
const RANK_GAP: f32 = 30.0;
pub(super) const FIT_MARGIN: f32 = 7.0;

pub(super) struct EdgeRoute {
    pub points: Vec<Pos2>,
    pub label: Pos2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flow {
    Down,
    Right,
}
pub(super) struct Placement {
    pub nodes: BTreeMap<usize, Rect>,
    pub edges: BTreeMap<usize, EdgeRoute>,
    pub bounds: Rect,
    pub flow: Flow,
}
impl Default for Placement {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            bounds: Rect::NOTHING,
            flow: Flow::Down,
        }
    }
}
pub(super) fn fit_scale(content: Vec2, viewport: Vec2) -> f32 {
    ((viewport.x - FIT_MARGIN * 2.0).max(1.0) / content.x.max(1.0))
        .min((viewport.y - FIT_MARGIN * 2.0).max(1.0) / content.y.max(1.0))
}
pub(super) fn adaptive(
    report: &AnalysisReport,
    sizes: &BTreeMap<usize, Vec2>,
    viewport: Vec2,
) -> Placement {
    let ranks = ranks(report);
    let widest = ranks.iter().map(Vec::len).max().unwrap_or(1).min(12);
    let mut best = Placement::default();
    let mut best_scale = -1.0;
    // Real text sizes differ across nodes. Compare complete placements instead
    // of guessing a column count from one fixed node width.
    for flow in [Flow::Down, Flow::Right] {
        for columns in 1..=widest {
            let candidate = arrange(report, sizes, &ranks, columns, flow);
            // Vertical stays preferred whenever both orientations fit at native
            // size. Short-wide canvases switch only to gain readable scale.
            let scale = fit_scale(candidate.bounds.size(), viewport).min(1.0);
            let fewer_ranks = match flow {
                Flow::Down => candidate.bounds.height() < best.bounds.height(),
                Flow::Right => candidate.bounds.width() < best.bounds.width(),
            };
            if scale > best_scale || (scale == best_scale && flow == best.flow && fewer_ranks) {
                best_scale = scale;
                best = candidate;
            }
        }
    }
    best
}
fn ranks(report: &AnalysisReport) -> Vec<Vec<usize>> {
    let mut levels = BTreeMap::new();
    let mut queue = VecDeque::new();
    if let Some(root) = report.cfg.first() {
        levels.insert(root.id, 0usize);
        queue.push_back(root.id);
    }
    let mut outgoing = BTreeMap::<usize, Vec<usize>>::new();
    for edge in &report.edges {
        outgoing.entry(edge.from).or_default().push(edge.to);
    }
    while let Some(id) = queue.pop_front() {
        let level = levels[&id] + 1;
        for target in outgoing.get(&id).into_iter().flatten() {
            if !levels.contains_key(target) {
                levels.insert(*target, level);
                queue.push_back(*target);
            }
        }
    }
    let mut rows = BTreeMap::<usize, Vec<usize>>::new();
    let last = levels.values().copied().max().unwrap_or(0) + 1;
    for block in &report.cfg {
        rows.entry(levels.get(&block.id).copied().unwrap_or(last))
            .or_default()
            .push(block.id);
    }
    rows.into_values().collect()
}
fn arrange(
    report: &AnalysisReport,
    sizes: &BTreeMap<usize, Vec2>,
    ranks: &[Vec<usize>],
    columns: usize,
    flow: Flow,
) -> Placement {
    let mut result = Placement {
        flow,
        ..Placement::default()
    };
    let rank_gap = match flow {
        Flow::Down => RANK_GAP,
        // Labels on horizontal edges live entirely between adjacent columns.
        Flow::Right => report
            .edges
            .iter()
            .map(|edge| label_size(edge.id, edge.kind).x + 10.0)
            .fold(RANK_GAP, f32::max),
    };
    let mut main = 0.0;
    for rank in ranks {
        for ids in rank.chunks(columns) {
            let span = ids
                .iter()
                .map(|id| match flow {
                    Flow::Down => sizes[id].x,
                    Flow::Right => sizes[id].y,
                })
                .sum::<f32>()
                + COLUMN_GAP * ids.len().saturating_sub(1) as f32;
            let mut cross = -span * 0.5;
            let mut depth = 0.0_f32;
            for id in ids {
                let size = sizes[id];
                let origin = match flow {
                    Flow::Down => Pos2::new(cross, main),
                    Flow::Right => Pos2::new(main, cross),
                };
                let rect = Rect::from_min_size(origin, size);
                result.nodes.insert(*id, rect);
                result.bounds = result.bounds.union(rect);
                let (node_depth, node_span) = match flow {
                    Flow::Down => (size.y, size.x),
                    Flow::Right => (size.x, size.y),
                };
                depth = depth.max(node_depth);
                cross += node_span + COLUMN_GAP;
            }
            main += depth + rank_gap;
        }
    }
    let node_bounds = result.bounds;
    for edge in &report.edges {
        let (Some(from), Some(to)) = (result.nodes.get(&edge.from), result.nodes.get(&edge.to))
        else {
            continue;
        };
        let route = match flow {
            Flow::Down => route_down(*from, *to, node_bounds.right(), edge.kind, edge.id),
            Flow::Right => route_right(
                *from,
                *to,
                node_bounds.bottom(),
                rank_gap,
                edge.kind,
                edge.id,
            ),
        };
        for point in &route.points {
            result.bounds.extend_with(*point);
        }
        // Reserve labels even at low zoom so zoom gestures never change layout.
        result.bounds = result.bounds.union(Rect::from_min_size(
            route.label,
            label_size(edge.id, edge.kind),
        ));
        result.edges.insert(edge.id, route);
    }
    result
}
fn label_size(id: usize, kind: EdgeKind) -> Vec2 {
    Vec2::new(
        super::edge_label(id, kind).chars().count() as f32 * 7.0 + 2.0,
        14.0,
    )
}

fn branch_port(kind: EdgeKind) -> f32 {
    match kind {
        EdgeKind::BranchTrue => 0.28,
        EdgeKind::BranchFalse => 0.72,
        _ => 0.5,
    }
}

fn route_down(from: Rect, to: Rect, outer_right: f32, kind: EdgeKind, id: usize) -> EdgeRoute {
    if to.top() >= from.bottom() + 1.0 {
        let port = branch_port(kind);
        let start = Pos2::new(from.left() + from.width() * port, from.bottom());
        let end = to.center_top();
        let middle = to.top() - RANK_GAP * 0.5;
        // Source-only labels collapse onto each other for an unknown jump with
        // many outgoing edges. Center each label on its own horizontal segment.
        let label_width = super::edge_label(id, kind).chars().count() as f32 * 7.0;
        EdgeRoute {
            points: vec![
                start,
                Pos2::new(start.x, middle),
                Pos2::new(end.x, middle),
                end,
            ],
            label: Pos2::new((start.x + end.x - label_width) * 0.5, middle - 12.0),
        }
    } else {
        let side = outer_right + 12.0 + (id % 5) as f32 * 6.0;
        let start = from.right_center();
        let end = Pos2::new(to.right(), to.center().y - 10.0);
        EdgeRoute {
            points: vec![start, Pos2::new(side, start.y), Pos2::new(side, end.y), end],
            label: Pos2::new(side + 3.0, start.y - 12.0),
        }
    }
}

fn route_right(
    from: Rect,
    to: Rect,
    outer_bottom: f32,
    rank_gap: f32,
    kind: EdgeKind,
    id: usize,
) -> EdgeRoute {
    if to.left() >= from.right() + 1.0 {
        let start = Pos2::new(from.right(), from.top() + from.height() * branch_port(kind));
        let end = to.left_center();
        let middle = to.left() - rank_gap * 0.5;
        EdgeRoute {
            points: vec![
                start,
                Pos2::new(middle, start.y),
                Pos2::new(middle, end.y),
                end,
            ],
            label: Pos2::new(
                middle - label_size(id, kind).x * 0.5,
                (start.y + end.y) * 0.5 - 12.0,
            ),
        }
    } else {
        let side = outer_bottom + 12.0 + (id % 5) as f32 * 6.0;
        let start = from.center_bottom();
        let end = Pos2::new(to.center().x - 10.0, to.bottom());
        EdgeRoute {
            points: vec![start, Pos2::new(start.x, side), Pos2::new(end.x, side), end],
            label: Pos2::new(start.x + 3.0, side + 3.0),
        }
    }
}
