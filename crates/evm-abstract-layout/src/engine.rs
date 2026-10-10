//! Private typed ELK adapter. Engine objects never cross the owned layout boundary.

use std::collections::{BTreeMap, BTreeSet};

use elkrs::core::{engine::RecursiveGraphLayoutEngine, options};
use elkrs::graph::{
    graph::{EdgeId, ElementId, ElkGraph, LabelId, NodeId, ShapeId},
    math::{KVector, Spacing},
    properties::{EnumSet, PropertyHolder},
};
use evm_abstract_notation::Symbol;

use crate::{
    EdgeGeometry, Group, GroupGeometry, GroupId, GroupKind, Input, Layout, LayoutError, Point,
    Rect, Size,
};

#[cfg(test)]
mod tests;

const EPSILON: f64 = 1e-6;
const INSET: f64 = 6.0;
const TITLE_HEIGHT: f64 = 14.0;
const HEADER_HEIGHT: f64 = 22.0;

struct Scene {
    graph: ElkGraph,
    nodes: BTreeMap<usize, NodeId>,
    groups: BTreeMap<GroupId, NodeId>,
    edges: BTreeMap<usize, (EdgeId, LabelId)>,
    captions: BTreeMap<GroupId, (String, LabelId)>,
}

pub(super) fn layout(input: &Input, groups: &[Group]) -> Result<Layout, LayoutError> {
    if input.nodes.is_empty() {
        return Ok(Layout::default());
    }
    let mut scene = Scene::new(input, groups)?;
    let elk = elkrs::create_elk();
    RecursiveGraphLayoutEngine::new(&elk.algorithms)
        .layout(&mut scene.graph)
        .map_err(|message| LayoutError::Engine { message })?;
    scene.geometry(input, groups)
}

fn policy(graph: &mut ElkGraph, node: NodeId, padding: Spacing) {
    let node = graph.node_mut(node);
    node.set_property(&options::ALGORITHM, "org.eclipse.elk.layered".to_owned());
    node.set_property(&options::DIRECTION, options::Direction::DOWN);
    node.set_property(&options::EDGE_ROUTING, options::EdgeRouting::ORTHOGONAL);
    node.set_property(
        &options::HIERARCHY_HANDLING,
        options::HierarchyHandling::INCLUDE_CHILDREN,
    );
    node.set_property(&options::RANDOM_SEED, 1);
    node.set_property(&options::PADDING, padding);
    node.set_property(&options::SPACING_NODE_NODE, 12.0);
    node.set_property(&options::SPACING_EDGE_LABEL, 4.0);
}

fn failure(element: &'static str, id: usize, reason: &'static str) -> LayoutError {
    LayoutError::Geometry {
        element,
        id,
        reason,
    }
}

impl Scene {
    fn new(input: &Input, groups: &[Group]) -> Result<Self, LayoutError> {
        for count in [
            input
                .nodes
                .len()
                .checked_add(groups.len())
                .and_then(|count| count.checked_add(1)),
            input.edges.len().checked_add(groups.len()),
            Some(input.edges.len()),
        ] {
            if count.is_none_or(|count| u32::try_from(count).is_err()) {
                return Err(LayoutError::TooLarge);
            }
        }
        let mut scene = Self {
            graph: ElkGraph::new(),
            nodes: BTreeMap::new(),
            groups: BTreeMap::new(),
            edges: BTreeMap::new(),
            captions: BTreeMap::new(),
        };
        let root = scene.graph.root;
        policy(&mut scene.graph, root, Spacing::uniform(INSET));
        let mut parents = BTreeMap::new();
        for group in groups {
            let parent = match group.parent {
                Some(parent) => *scene
                    .groups
                    .get(&parent)
                    .ok_or_else(|| failure("group", group.id.0, "parent is not registered"))?,
                None => root,
            };
            let node = scene.graph.create_node(Some(parent));
            scene.graph.node_mut(node).identifier = Some(format!("group:{}", group.id.0));
            policy(
                &mut scene.graph,
                node,
                Spacing::new(HEADER_HEIGHT, INSET, INSET, INSET),
            );
            let title = match group.kind {
                GroupKind::Program(program) => Symbol::Program(program).to_string(),
                GroupKind::Chain => "Chain".to_owned(),
                GroupKind::Cycle => "Cycle".to_owned(),
            };
            // These short captions use a conservative 10pt glyph advance. The
            // actual cards and edge labels always retain caller-measured sizes.
            let width = title.chars().count() as f64 * 6.0;
            let label = scene.graph.create_label(&title, ElementId::Node(node));
            scene
                .graph
                .label_mut(label)
                .shape
                .set_dimensions(width, TITLE_HEIGHT);
            let node_data = scene.graph.node_mut(node);
            node_data.set_property(
                &options::NODE_LABELS_PLACEMENT,
                EnumSet::of(&[
                    options::NodeLabelPlacement::INSIDE,
                    options::NodeLabelPlacement::V_TOP,
                    options::NodeLabelPlacement::H_LEFT,
                ]),
            );
            node_data.set_property(
                &options::NODE_LABELS_PADDING,
                Spacing::new(INSET, INSET, 2.0, INSET),
            );
            node_data.set_property(
                &options::NODE_SIZE_CONSTRAINTS,
                EnumSet::of(&[
                    options::SizeConstraint::MINIMUM_SIZE,
                    options::SizeConstraint::NODE_LABELS,
                ]),
            );
            node_data.set_property(
                &options::NODE_SIZE_MINIMUM,
                KVector::new(width + INSET * 2.0, HEADER_HEIGHT + INSET),
            );
            scene.groups.insert(group.id, node);
            scene.captions.insert(group.id, (title, label));
            // Parent-before-child groups let the most specific container own
            // each real leaf, without replacing that leaf's edge endpoints.
            for member in &group.members {
                parents.insert(*member, node);
            }
        }
        for source in &input.nodes {
            let parent = parents.get(&source.id).copied().unwrap_or(root);
            let node = scene.graph.create_node(Some(parent));
            let data = scene.graph.node_mut(node);
            data.identifier = Some(format!("node:{}", source.id));
            data.shape
                .set_dimensions(source.size.width, source.size.height);
            scene.nodes.insert(source.id, node);
        }
        for source in &input.edges {
            let edge = scene.graph.create_simple_edge(
                ShapeId::Node(scene.nodes[&source.from]),
                ShapeId::Node(scene.nodes[&source.to]),
            );
            scene.graph.edge_mut(edge).identifier = Some(format!("edge:{}", source.id));
            let label = scene
                .graph
                .create_label(&source.label.text, ElementId::Edge(edge));
            let data = scene.graph.label_mut(label);
            data.shape
                .set_dimensions(source.label.size.width, source.label.size.height);
            data.set_property(
                &options::EDGE_LABELS_PLACEMENT,
                options::EdgeLabelPlacement::CENTER,
            );
            scene.edges.insert(source.id, (edge, label));
        }
        Ok(scene)
    }

    fn geometry(&self, input: &Input, groups: &[Group]) -> Result<Layout, LayoutError> {
        let offsets = self.offsets()?;
        let mut result = Layout::default();
        for source in &input.nodes {
            let node = self.nodes[&source.id];
            let shape = &self.graph.node(node).shape;
            let bounds = Rect::new(offsets[node.index()], Size::new(shape.width, shape.height));
            if !valid_rect(bounds) || !same_size(bounds.size, source.size) {
                return Err(failure(
                    "node",
                    source.id,
                    "missing or resized measured card",
                ));
            }
            result.nodes.insert(source.id, bounds);
            extend(&mut result.bounds, bounds);
        }
        for group in groups {
            let node = self.groups[&group.id];
            let shape = &self.graph.node(node).shape;
            let bounds = Rect::new(offsets[node.index()], Size::new(shape.width, shape.height));
            if !valid_rect(bounds)
                || group.members.iter().any(|member| {
                    !result
                        .nodes
                        .get(member)
                        .is_some_and(|leaf| contains(bounds, *leaf))
                })
            {
                return Err(failure(
                    "group",
                    group.id.0,
                    "container does not contain its members",
                ));
            }
            let (title, label) = &self.captions[&group.id];
            let caption = label_rect(&self.graph, *label, offsets[node.index()]);
            if !valid_rect(caption) || !contains(bounds, caption) {
                return Err(failure("group", group.id.0, "invalid reserved caption"));
            }
            result.groups.push(GroupGeometry {
                group: group.clone(),
                bounds,
                caption,
                title: title.clone(),
            });
            extend(&mut result.bounds, bounds);
        }
        for source in &input.edges {
            let (edge_id, label_id) = self.edges[&source.id];
            let edge = self.graph.edge(edge_id);
            if edge.sources.as_slice() != [ShapeId::Node(self.nodes[&source.from])]
                || edge.targets.as_slice() != [ShapeId::Node(self.nodes[&source.to])]
                || edge.labels.as_slice() != [label_id]
            {
                return Err(failure(
                    "edge",
                    source.id,
                    "original endpoints or label changed",
                ));
            }
            let offset = edge
                .containing_node
                .map_or(Point::default(), |parent| offsets[parent.index()]);
            let path = edge_path(
                &self.graph,
                edge_id,
                offset,
                result.nodes[&source.from],
                result.nodes[&source.to],
            )
            .map_err(|reason| failure("edge", source.id, reason))?;
            let label = label_rect(&self.graph, label_id, offset);
            if !valid_rect(label) || !same_size(label.size, source.label.size) {
                return Err(failure(
                    "label",
                    source.id,
                    "missing or resized measured label",
                ));
            }
            for point in &path {
                extend(&mut result.bounds, Rect::new(*point, Size::default()));
            }
            extend(&mut result.bounds, label);
            result.edges.insert(
                source.id,
                EdgeGeometry {
                    from: source.from,
                    to: source.to,
                    kind: source.kind,
                    path,
                    label,
                },
            );
        }
        place_captions(&mut result)?;
        if result.nodes.len() != input.nodes.len()
            || result.edges.len() != input.edges.len()
            || result.bounds.is_none_or(|bounds| !valid_rect(bounds))
        {
            return Err(failure("layout", 0, "incomplete or nonfinite bounds"));
        }
        Ok(result)
    }

    fn offsets(&self) -> Result<Vec<Point>, LayoutError> {
        let mut offsets = Vec::with_capacity(self.graph.nodes.len());
        // Our arena registers every parent before its children. Layout changes
        // geometry, never the containment tree or original node identities.
        for (index, node) in self.graph.nodes.iter().enumerate() {
            let parent = match node.parent {
                Some(parent) => *offsets
                    .get(parent.index())
                    .ok_or_else(|| failure("layout", index, "invalid containment order"))?,
                None => Point::default(),
            };
            let offset = Point::new(node.shape.x, node.shape.y).translated(parent);
            if !offset.is_finite() {
                return Err(failure("layout", index, "nonfinite parent offset"));
            }
            offsets.push(offset);
        }
        Ok(offsets)
    }
}

fn label_rect(graph: &ElkGraph, label: LabelId, offset: Point) -> Rect {
    let shape = &graph.label(label).shape;
    Rect::new(
        Point::new(shape.x, shape.y).translated(offset),
        Size::new(shape.width, shape.height),
    )
}

fn edge_path(
    graph: &ElkGraph,
    edge: EdgeId,
    offset: Point,
    from: Rect,
    to: Rect,
) -> Result<Vec<Point>, &'static str> {
    let mut sections = Vec::new();
    let mut ids = BTreeSet::new();
    for id in &graph.edge(edge).sections {
        if !ids.insert(*id) {
            return Err("duplicate route section");
        }
        let section = graph.section(*id);
        let mut points = vec![Point::new(section.start_x, section.start_y).translated(offset)];
        points.extend(
            section
                .bend_points
                .iter()
                .map(|&(x, y)| Point::new(x, y).translated(offset)),
        );
        points.push(Point::new(section.end_x, section.end_y).translated(offset));
        if points.iter().any(|point| !point.is_finite()) {
            return Err("nonfinite route point");
        }
        sections.push(points);
    }
    let starts: Vec<_> = sections
        .iter()
        .enumerate()
        .filter(|(_, points)| on_border(from, points[0]))
        .map(|(index, _)| index)
        .collect();
    if starts.len() != 1 {
        return Err("missing or ambiguous source section");
    }
    let mut remaining: BTreeSet<_> = (0..sections.len()).collect();
    let mut next = starts[0];
    let mut path = Vec::new();
    loop {
        remaining.remove(&next);
        for point in &sections[next] {
            if path.last().is_none_or(|last| !near(*last, *point)) {
                path.push(*point);
            }
        }
        if remaining.is_empty() {
            break;
        }
        let end = *path.last().ok_or("empty route section")?;
        let matches: Vec<_> = remaining
            .iter()
            .copied()
            .filter(|index| near(end, sections[*index][0]))
            .collect();
        if matches.len() != 1 {
            return Err("disconnected or branching route sections");
        }
        next = matches[0];
    }
    if path.len() < 2 || !on_border(to, *path.last().ok_or("empty route")?) {
        return Err("route does not reach the original target");
    }
    if path.windows(2).any(|pair| {
        (pair[0].x - pair[1].x).abs() > EPSILON && (pair[0].y - pair[1].y).abs() > EPSILON
    }) {
        return Err("nonorthogonal route segment");
    }
    Ok(path)
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= EPSILON && (a.y - b.y).abs() <= EPSILON
}
fn same_size(a: Size, b: Size) -> bool {
    (a.width - b.width).abs() <= EPSILON && (a.height - b.height).abs() <= EPSILON
}
fn valid_rect(rect: Rect) -> bool {
    rect.is_valid() && rect.right().is_finite() && rect.bottom().is_finite()
}
fn contains(outer: Rect, inner: Rect) -> bool {
    outer.origin.x <= inner.origin.x + EPSILON
        && outer.origin.y <= inner.origin.y + EPSILON
        && outer.right() + EPSILON >= inner.right()
        && outer.bottom() + EPSILON >= inner.bottom()
}
fn on_border(rect: Rect, point: Point) -> bool {
    point.x + EPSILON >= rect.origin.x
        && point.x <= rect.right() + EPSILON
        && point.y + EPSILON >= rect.origin.y
        && point.y <= rect.bottom() + EPSILON
        && ((point.x - rect.origin.x).abs() <= EPSILON
            || (point.x - rect.right()).abs() <= EPSILON
            || (point.y - rect.origin.y).abs() <= EPSILON
            || (point.y - rect.bottom()).abs() <= EPSILON)
}
fn extend(bounds: &mut Option<Rect>, rect: Rect) {
    *bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.origin.x < b.right() - EPSILON
        && b.origin.x < a.right() - EPSILON
        && a.origin.y < b.bottom() - EPSILON
        && b.origin.y < a.bottom() - EPSILON
}
fn segment_hits(rect: Rect, a: Point, b: Point) -> bool {
    if (a.x - b.x).abs() <= EPSILON {
        a.x > rect.origin.x - EPSILON
            && a.x < rect.right() + EPSILON
            && a.y.min(b.y) < rect.bottom() + EPSILON
            && a.y.max(b.y) > rect.origin.y - EPSILON
    } else {
        a.y > rect.origin.y - EPSILON
            && a.y < rect.bottom() + EPSILON
            && a.x.min(b.x) < rect.right() + EPSILON
            && a.x.max(b.x) > rect.origin.x - EPSILON
    }
}

/// Node labels reserve hierarchy padding in ELK. Incoming cross-group routes can
/// still cross a header; move only that annotation into free header space or the
/// outer margin, retaining every original route and its source/target geometry.
fn place_captions(layout: &mut Layout) -> Result<(), LayoutError> {
    let content = layout
        .bounds
        .ok_or_else(|| failure("layout", 0, "missing content bounds"))?;
    let mut placed = Vec::new();
    for group in &mut layout.groups {
        let size = group.caption.size;
        let y = group.bounds.origin.y + INSET;
        let left = group.bounds.origin.x + INSET;
        let right = group.bounds.right() - size.width - INSET;
        let free = |caption: Rect| {
            let clearance = Rect::new(
                Point::new(caption.origin.x - 2.0, caption.origin.y - 2.0),
                Size::new(caption.size.width + 4.0, caption.size.height + 4.0),
            );
            !layout.nodes.values().any(|node| overlap(clearance, *node))
                && !layout.edges.values().any(|edge| {
                    overlap(clearance, edge.label)
                        || edge
                            .path
                            .windows(2)
                            .any(|pair| segment_hits(clearance, pair[0], pair[1]))
                })
                && !placed.iter().any(|caption| overlap(clearance, *caption))
        };
        let preferred = Rect::new(Point::new(left, y), size);
        if free(preferred) {
            group.caption = preferred;
            placed.push(preferred);
            extend(&mut layout.bounds, preferred);
            continue;
        }
        // Most headers need no adjustment. When one does, only obstacles in
        // this header can supply useful candidate boundaries; unrelated routes
        // must not produce a whole-graph sort for every spatial group.
        let header = Rect::new(
            Point::new(left - 2.0, y - 2.0),
            Size::new(right - left + size.width + 4.0, size.height + 4.0),
        );
        let mut candidates = vec![left, right];
        for edge in layout.edges.values() {
            if overlap(header, edge.label) {
                candidates.extend([
                    edge.label.right() + 3.0,
                    edge.label.origin.x - size.width - 3.0,
                ]);
            }
            for segment in edge.path.windows(2) {
                if segment_hits(header, segment[0], segment[1]) {
                    for point in segment {
                        candidates.extend([point.x + 3.0, point.x - size.width - 3.0]);
                    }
                }
            }
        }
        for obstacle in layout.nodes.values().chain(placed.iter()) {
            if overlap(header, *obstacle) {
                candidates.extend([obstacle.right() + 3.0, obstacle.origin.x - size.width - 3.0]);
            }
        }
        candidates.sort_by(f64::total_cmp);
        candidates.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
        let mut caption = candidates
            .into_iter()
            .filter(|x| *x >= left - EPSILON && *x <= right + EPSILON)
            .map(|x| Rect::new(Point::new(x, y), size))
            .find(|caption| free(*caption));
        if caption.is_none() {
            let x = content.origin.x - size.width - INSET;
            let mut candidate = Rect::new(Point::new(x, y), size);
            // Only prior captions can occupy the margin beyond all engine
            // geometry, so each bounded step clears at least one annotation.
            for _ in 0..=placed.len() {
                if free(candidate) {
                    caption = Some(candidate);
                    break;
                }
                candidate.origin.y += size.height + 4.0;
            }
        }
        let caption = caption
            .ok_or_else(|| failure("group", group.group.id.0, "no finite caption placement"))?;
        if !valid_rect(caption) {
            return Err(failure("group", group.group.id.0, "invalid caption bounds"));
        }
        group.caption = caption;
        placed.push(caption);
        extend(&mut layout.bounds, caption);
    }
    Ok(())
}
