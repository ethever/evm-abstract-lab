//! Initial layout over real states and edge-label vertices. Every visible edge
//! is State -> EdgeLabel -> State; label boxes reserve space like other nodes.
//! Virtual identities never enter the analysis or the exposed native node map.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};

use egui::{Pos2, Rect, Vec2};
#[cfg(test)]
use evm_abstract_protocol::AnalysisReport;
use evm_abstract_protocol::{CfgEdge, EdgeKind};

const CROSS_GAP: f32 = 22.0;
const LAYER_GAP: f32 = 12.0;
const ROUTE_CLEARANCE: f32 = 3.0;
const OUTER_GAP: f32 = 18.0;
const LANE_GAP: f32 = 6.0;
pub(super) const FIT_MARGIN: f32 = 7.0;

pub(super) struct EdgeRoute {
    pub to_label: Vec<Pos2>,
    pub to_target: Vec<Pos2>,
    pub label: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flow {
    Down,
    Right,
}

impl Flow {
    fn point(self, main: f32, cross: f32) -> Pos2 {
        match self {
            Self::Down => Pos2::new(cross, main),
            Self::Right => Pos2::new(main, cross),
        }
    }

    fn main(self, point: Pos2) -> f32 {
        match self {
            Self::Down => point.y,
            Self::Right => point.x,
        }
    }

    fn cross(self, point: Pos2) -> f32 {
        match self {
            Self::Down => point.x,
            Self::Right => point.y,
        }
    }

    fn depth(self, size: Vec2) -> f32 {
        match self {
            Self::Down => size.y,
            Self::Right => size.x,
        }
    }

    fn span(self, size: Vec2) -> f32 {
        match self {
            Self::Down => size.x,
            Self::Right => size.y,
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum VertexId {
    State(usize),
    EdgeLabel(usize),
}

struct Expanded {
    layers: Vec<Vec<VertexId>>,
    sizes: BTreeMap<VertexId, Vec2>,
    levels: BTreeMap<usize, usize>,
    edges: Vec<CfgEdge>,
}

impl Expanded {
    #[cfg(test)]
    fn new(report: &AnalysisReport, state_sizes: &BTreeMap<usize, Vec2>) -> Self {
        let nodes: Vec<_> = report.cfg.iter().map(|state| state.id).collect();
        Self::scene(&nodes, &report.edges, state_sizes, &BTreeMap::new())
    }

    fn scene(
        node_ids: &[usize],
        edges: &[CfgEdge],
        state_sizes: &BTreeMap<usize, Vec2>,
        labels: &BTreeMap<usize, String>,
    ) -> Self {
        let mut sizes: BTreeMap<_, _> = node_ids
            .iter()
            .filter_map(|state| {
                state_sizes
                    .get(state)
                    .copied()
                    .filter(|size| size.is_finite() && size.min_elem() > 0.0)
                    .map(|size| (VertexId::State(*state), size))
            })
            .collect();
        // Both endpoints must exist in the rendered state set. A dangling edge
        // cannot create a phantom label, state, or BFS layer.
        let edges: Vec<_> = edges
            .iter()
            .filter(|edge| {
                sizes.contains_key(&VertexId::State(edge.from))
                    && sizes.contains_key(&VertexId::State(edge.to))
            })
            .map(|edge| (edge.id, edge))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .cloned()
            .collect();
        let mut outgoing = BTreeMap::<usize, Vec<usize>>::new();
        for edge in &edges {
            outgoing.entry(edge.from).or_default().push(edge.to);
        }
        let mut levels = BTreeMap::new();
        let mut queue = VecDeque::new();
        if let Some(root) = node_ids
            .iter()
            .find(|state| sizes.contains_key(&VertexId::State(**state)))
        {
            levels.insert(*root, 0usize);
            queue.push_back(*root);
        }
        while let Some(state) = queue.pop_front() {
            let level = levels[&state] + 1;
            for target in outgoing.get(&state).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(entry) = levels.entry(*target) {
                    entry.insert(level);
                    queue.push_back(*target);
                }
            }
        }
        let disconnected = levels.values().copied().max().unwrap_or(0) + 1;
        let mut layers = BTreeMap::<usize, Vec<VertexId>>::new();
        for vertex in sizes.keys() {
            let VertexId::State(state) = *vertex else {
                continue;
            };
            let level = *levels.entry(state).or_insert(disconnected);
            layers.entry(level * 2).or_default().push(*vertex);
        }
        // Source grouping keeps labels near the rank whose outgoing edges they
        // describe. Native edge IDs break ties deterministically.
        let mut ordered_edges: Vec<_> = edges.iter().collect();
        ordered_edges.sort_by_key(|edge| (levels[&edge.from], edge.from, edge.id));
        for edge in ordered_edges {
            let vertex = VertexId::EdgeLabel(edge.id);
            let label = labels
                .get(&edge.id)
                .cloned()
                .unwrap_or_else(|| super::edge_label(edge.id, edge.kind));
            sizes.insert(vertex, text_label_size(&label));
            layers
                .entry(levels[&edge.from] * 2 + 1)
                .or_default()
                .push(vertex);
        }
        Self {
            layers: layers.into_values().collect(),
            sizes,
            levels,
            edges,
        }
    }
}

pub(super) fn fit_scale(content: Vec2, viewport: Vec2) -> f32 {
    ((viewport.x - FIT_MARGIN * 2.0).max(1.0) / content.x.max(1.0))
        .min((viewport.y - FIT_MARGIN * 2.0).max(1.0) / content.y.max(1.0))
}

#[cfg(test)]
pub(super) fn adaptive(
    report: &AnalysisReport,
    sizes: &BTreeMap<usize, Vec2>,
    viewport: Vec2,
) -> Placement {
    let nodes: Vec<_> = report.cfg.iter().map(|state| state.id).collect();
    adaptive_scene(&nodes, &report.edges, sizes, &BTreeMap::new(), viewport)
}

pub(super) fn adaptive_scene(
    node_ids: &[usize],
    edges: &[CfgEdge],
    sizes: &BTreeMap<usize, Vec2>,
    labels: &BTreeMap<usize, String>,
    viewport: Vec2,
) -> Placement {
    let graph = Expanded::scene(node_ids, edges, sizes, labels);
    if graph.sizes.is_empty() {
        return Placement::default();
    }
    let mut best = Placement::default();
    let mut best_scale = -1.0;
    for flow in [Flow::Down, Flow::Right] {
        for capacity in candidate_capacities(&graph, viewport, flow) {
            let candidate = arrange(&graph, capacity, flow);
            let scale = fit_scale(candidate.bounds.size(), viewport).min(1.0);
            let fewer_ranks = flow.depth(candidate.bounds.size()) < flow.depth(best.bounds.size());
            // Preserve vertical orientation on equal capped fit; once native
            // typography fits, avoid adding rows to chase unused magnification.
            if scale > best_scale || (scale == best_scale && flow == best.flow && fewer_ranks) {
                best_scale = scale;
                best = candidate;
            }
        }
    }
    best
}

// Full state graphs can contain thousands of context instances. Keep the
// original exhaustive small-scene choice, but route a large graph only twice
// (one capacity in each orientation), rather than constructing 24 placements.
fn candidate_capacities(graph: &Expanded, viewport: Vec2, flow: Flow) -> Vec<usize> {
    let widest = graph.layers.iter().map(Vec::len).max().unwrap_or(1).min(12);
    if graph.sizes.len() <= 256 {
        return (1..=widest).collect();
    }
    // Estimate how many real nodes fit across this orientation. Tiny virtual
    // labels must not dilute the mean and cause crowded state rows; labels
    // still reserve their full bounds during packing and final fit.
    let (total_span, count) = graph
        .sizes
        .iter()
        .filter(|(vertex, _)| matches!(vertex, VertexId::State(_)))
        .fold((0.0, 0usize), |(sum, count), (_, size)| {
            (sum + flow.span(*size), count + 1)
        });
    let average_span = total_span / count.max(1) as f32;
    let available = (flow.span(viewport) - FIT_MARGIN * 2.0).max(0.0);
    let capacity = ((available + CROSS_GAP) / (average_span + CROSS_GAP)) as usize;
    vec![capacity.clamp(1, widest)]
}

#[cfg(test)]
fn label_size(id: usize, kind: EdgeKind) -> Vec2 {
    text_label_size(&super::edge_label(id, kind))
}

fn text_label_size(label: &str) -> Vec2 {
    // Secondary annotations use 9pt monospace with a conservative 6pt advance
    // and 3pt horizontal padding. Keep a compact box for routing and avoidance.
    Vec2::new(label.chars().count() as f32 * 6.0 + 6.0, 16.0)
}

struct Vertex {
    rect: Rect,
    band: usize,
}

struct Band {
    start: f32,
    end: f32,
    vertices: Vec<VertexId>,
}

struct Geometry {
    vertices: BTreeMap<VertexId, Vertex>,
    bands: Vec<Band>,
    bounds: Rect,
    flow: Flow,
}

impl Geometry {
    fn pack(graph: &Expanded, capacity: usize, flow: Flow) -> Self {
        let mut result = Self {
            vertices: BTreeMap::new(),
            bands: Vec::new(),
            bounds: Rect::NOTHING,
            flow,
        };
        let mut main = 0.0;
        for layer in &graph.layers {
            for ids in layer.chunks(capacity) {
                let span = ids.iter().map(|id| flow.span(graph.sizes[id])).sum::<f32>()
                    + CROSS_GAP * ids.len().saturating_sub(1) as f32;
                let mut cross = -span * 0.5;
                let mut depth = 0.0_f32;
                for id in ids {
                    let size = graph.sizes[id];
                    let rect = Rect::from_min_size(flow.point(main, cross), size);
                    result.vertices.insert(
                        *id,
                        Vertex {
                            rect,
                            band: result.bands.len(),
                        },
                    );
                    result.bounds = result.bounds.union(rect);
                    depth = depth.max(flow.depth(size));
                    cross += flow.span(size) + CROSS_GAP;
                }
                result.bands.push(Band {
                    start: main,
                    end: main + depth,
                    vertices: ids.to_vec(),
                });
                main += depth + LAYER_GAP;
            }
        }
        result
    }

    fn blocked(&self, points: &[Pos2], from: VertexId, to: VertexId) -> bool {
        points.windows(2).any(|segment| {
            let low = self.flow.main(segment[0]).min(self.flow.main(segment[1]));
            let high = self.flow.main(segment[0]).max(self.flow.main(segment[1]));
            // Bands are sorted, non-overlapping rank slices. Query only slices
            // intersected by this segment, rather than scanning every vertex.
            let first = self
                .bands
                .partition_point(|band| band.end + ROUTE_CLEARANCE <= low);
            for band in self.bands[first..]
                .iter()
                .take_while(|band| band.start - ROUTE_CLEARANCE <= high)
            {
                if band.vertices.iter().any(|id| {
                    *id != from
                        && *id != to
                        && crosses_interior(
                            segment[0],
                            segment[1],
                            self.vertices[id].rect.expand(ROUTE_CLEARANCE),
                        )
                }) {
                    return true;
                }
            }
            false
        })
    }

    fn link(
        &self,
        from: VertexId,
        to: VertexId,
        port: f32,
        force_outer: bool,
        lanes: &mut Lanes,
    ) -> Vec<Pos2> {
        let source = &self.vertices[&from];
        let target = &self.vertices[&to];
        let source_cross =
            self.flow.cross(source.rect.min) + self.flow.span(source.rect.size()) * port;
        let target_cross = self.flow.cross(target.rect.center());
        let start = self
            .flow
            .point(self.flow.main(source.rect.max), source_cross);
        let end = self
            .flow
            .point(self.flow.main(target.rect.min), target_cross);
        let departure = self.bands[source.band].end + LAYER_GAP * 0.5;
        let approach = self.bands[target.band].start - LAYER_GAP * 0.5;
        if !force_outer && target.band > source.band {
            for bend in [departure, approach] {
                let path = compact(vec![
                    start,
                    self.flow.point(bend, source_cross),
                    self.flow.point(bend, target_cross),
                    end,
                ]);
                if !self.blocked(&path, from, to) {
                    return path;
                }
            }
        }
        // Leave through the empty corridor after the complete source band,
        // enter through the corridor before the target band, and travel beyond
        // every state/label box between them. This also handles self/back edges.
        let lane = lanes.allocate(departure.min(approach), departure.max(approach));
        let outside = self.flow.cross(self.bounds.max) + OUTER_GAP + lane as f32 * LANE_GAP;
        compact(vec![
            start,
            self.flow.point(departure, source_cross),
            self.flow.point(departure, outside),
            self.flow.point(approach, outside),
            self.flow.point(approach, target_cross),
            end,
        ])
    }
}

#[derive(Default)]
struct Lanes(Vec<Vec<(f32, f32)>>);

impl Lanes {
    fn allocate(&mut self, low: f32, high: f32) -> usize {
        for (index, intervals) in self.0.iter_mut().enumerate() {
            if intervals
                .iter()
                .all(|(start, end)| high < *start || low > *end)
            {
                intervals.push((low, high));
                return index;
            }
        }
        self.0.push(vec![(low, high)]);
        self.0.len() - 1
    }
}

fn arrange(graph: &Expanded, capacity: usize, flow: Flow) -> Placement {
    let geometry = Geometry::pack(graph, capacity, flow);
    let mut result = Placement {
        flow,
        bounds: geometry.bounds,
        ..Placement::default()
    };
    for (id, vertex) in &geometry.vertices {
        if let VertexId::State(state) = id {
            result.nodes.insert(*state, vertex.rect);
        }
    }
    let mut lanes = Lanes::default();
    for edge in &graph.edges {
        let label_id = VertexId::EdgeLabel(edge.id);
        let port = match edge.kind {
            EdgeKind::BranchTrue => 0.28,
            EdgeKind::BranchFalse => 0.72,
            _ => 0.5,
        };
        let to_label = geometry.link(
            VertexId::State(edge.from),
            label_id,
            port,
            false,
            &mut lanes,
        );
        let to_target = geometry.link(
            label_id,
            VertexId::State(edge.to),
            0.5,
            graph.levels[&edge.to] <= graph.levels[&edge.from],
            &mut lanes,
        );
        for point in to_label.iter().chain(&to_target) {
            result.bounds.extend_with(*point);
        }
        result.edges.insert(
            edge.id,
            EdgeRoute {
                to_label,
                to_target,
                label: geometry.vertices[&label_id].rect,
            },
        );
    }
    result
}

fn compact(mut path: Vec<Pos2>) -> Vec<Pos2> {
    path.dedup();
    let mut result: Vec<Pos2> = Vec::with_capacity(path.len());
    for point in path {
        while result.len() >= 2 {
            let previous = result[result.len() - 2];
            let last = result[result.len() - 1];
            if (previous.x == last.x
                && last.x == point.x
                && (last.y - previous.y) * (point.y - last.y) >= 0.0)
                || (previous.y == last.y
                    && last.y == point.y
                    && (last.x - previous.x) * (point.x - last.x) >= 0.0)
            {
                result.pop();
            } else {
                break;
            }
        }
        result.push(point);
    }
    result
}

fn crosses_interior(from: Pos2, to: Pos2, rect: Rect) -> bool {
    if from.x == to.x {
        from.x > rect.left()
            && from.x < rect.right()
            && from.y.max(to.y) > rect.top()
            && from.y.min(to.y) < rect.bottom()
    } else if from.y == to.y {
        from.y > rect.top()
            && from.y < rect.bottom()
            && from.x.max(to.x) > rect.left()
            && from.x.min(to.x) < rect.right()
    } else {
        // All routing primitives are orthogonal; reject a diagonal candidate.
        true
    }
}
