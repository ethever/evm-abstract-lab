//! Conversion between measured egui content and viewport-independent geometry.
//! The private ELK graph is owned by evm-abstract-layout. This layer preserves
//! original leaf/edge IDs and translates complete routes into painter coordinates.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use egui::{FontId, Painter, Pos2, Rect, Vec2};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::{AnalysisReport, EdgeKind};

use super::{index::ReportIndex, projection::DisplayGraph};
use crate::palette;

pub(super) const FIT_MARGIN: f32 = 7.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EdgeRoute {
    pub path: Vec<Pos2>,
    pub label: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Group {
    pub id: geometry::GroupId,
    pub parent: Option<geometry::GroupId>,
    pub kind: geometry::GroupKind,
    pub members: Vec<usize>,
    pub bounds: Rect,
    pub caption: Rect,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Placement {
    pub nodes: BTreeMap<usize, Rect>,
    pub edges: BTreeMap<usize, EdgeRoute>,
    pub groups: Vec<Group>,
    pub bounds: Rect,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            groups: Vec::new(),
            bounds: Rect::NOTHING,
        }
    }
}

pub(super) fn fit_scale(content: Vec2, viewport: Vec2) -> f32 {
    ((viewport.x - FIT_MARGIN * 2.0).max(1.0) / content.x.max(1.0))
        .min((viewport.y - FIT_MARGIN * 2.0).max(1.0) / content.y.max(1.0))
}

pub(super) fn prepare(
    report: &AnalysisReport,
    index: &ReportIndex,
    display: &DisplayGraph,
    sizes: &BTreeMap<usize, Vec2>,
    labels: &BTreeMap<usize, String>,
    painter: &Painter,
) -> geometry::Input {
    let mut nodes = Vec::new();
    for node in &display.nodes {
        let Some(size) = sizes.get(&node.id) else {
            continue;
        };
        let mut depths = BTreeSet::new();
        let mut boundary = false;
        for member in &node.members {
            if let Some(state) = index.cfg(report, *member) {
                depths.insert(state.frame_depth);
            } else {
                boundary = true;
            }
            boundary |= index.has_frontier(*member);
            for position in index
                .incoming(*member)
                .iter()
                .chain(index.outgoing(*member))
            {
                let edge = &report.edges[*position];
                boundary |= !display.state_to_node.contains_key(&edge.from)
                    || !display.state_to_node.contains_key(&edge.to);
            }
        }
        nodes.push(geometry::Node {
            id: node.id,
            size: geometry::Size::new(f64::from(size.x), f64::from(size.y)),
            program: node.program,
            block: node.basic_block,
            frame_depth: if depths.len() == 1 {
                depths.first().copied()
            } else {
                None
            },
            boundary,
        });
    }
    let visible: BTreeSet<_> = nodes.iter().map(|node| node.id).collect();
    let edges = display
        .edges
        .iter()
        .filter(|edge| visible.contains(&edge.from) && visible.contains(&edge.to))
        .map(|edge| {
            let text = labels
                .get(&edge.id)
                .cloned()
                .unwrap_or_else(|| super::edge_label(edge.id, edge.kind));
            let measured = painter
                .layout_job(crate::notation::job(
                    &text,
                    FontId::monospace(9.0),
                    palette::MUTED,
                ))
                .size();
            geometry::Edge {
                id: edge.id,
                from: edge.from,
                to: edge.to,
                kind: kind(edge.kind),
                label: geometry::Label {
                    text,
                    size: geometry::Size::new(
                        f64::from(measured.x + 6.0),
                        f64::from((measured.y + 4.0).max(16.0)),
                    ),
                },
            }
        })
        .collect();
    geometry::Input { nodes, edges }
}

fn kind(kind: EdgeKind) -> geometry::EdgeKind {
    match kind {
        EdgeKind::Fallthrough => geometry::EdgeKind::Fallthrough,
        EdgeKind::Jump => geometry::EdgeKind::Jump,
        EdgeKind::BranchTrue => geometry::EdgeKind::BranchTrue,
        EdgeKind::BranchFalse => geometry::EdgeKind::BranchFalse,
        EdgeKind::Call => geometry::EdgeKind::Call,
        EdgeKind::Return => geometry::EdgeKind::Return,
        EdgeKind::Failure => geometry::EdgeKind::Failure,
        EdgeKind::Revert => geometry::EdgeKind::Revert,
    }
}

pub(super) fn scene(input: &geometry::Input) -> Result<Placement, geometry::LayoutError> {
    let layout = geometry::layout(input)?;
    let mut placement = Placement::default();
    for (id, bounds) in layout.nodes {
        placement.nodes.insert(id, rectangle(bounds, "node", id)?);
    }
    for (id, edge) in layout.edges {
        placement.edges.insert(
            id,
            EdgeRoute {
                path: edge
                    .path
                    .into_iter()
                    .map(|p| point(p, "edge", id))
                    .collect::<Result<_, _>>()?,
                label: rectangle(edge.label, "label", id)?,
            },
        );
    }
    for group in layout.groups {
        placement.groups.push(Group {
            id: group.group.id,
            parent: group.group.parent,
            kind: group.group.kind,
            members: group.group.members,
            bounds: rectangle(group.bounds, "group", group.group.id.0)?,
            caption: rectangle(group.caption, "group caption", group.group.id.0)?,
            title: group.title,
        });
    }
    if let Some(bounds) = layout.bounds {
        placement.bounds = rectangle(bounds, "content", 0)?;
    }
    Ok(placement)
}

fn point(
    value: geometry::Point,
    element: &'static str,
    id: usize,
) -> Result<Pos2, geometry::LayoutError> {
    if !value.is_finite()
        || value.x.abs() > f64::from(f32::MAX)
        || value.y.abs() > f64::from(f32::MAX)
    {
        return Err(geometry::LayoutError::Geometry {
            element,
            id,
            reason: "outside painter coordinate range",
        });
    }
    Ok(Pos2::new(value.x as f32, value.y as f32))
}

fn rectangle(
    value: geometry::Rect,
    element: &'static str,
    id: usize,
) -> Result<Rect, geometry::LayoutError> {
    if !value.is_valid() {
        return Err(geometry::LayoutError::Geometry {
            element,
            id,
            reason: "nonfinite or empty rectangle",
        });
    }
    Ok(Rect::from_min_max(
        point(value.origin, element, id)?,
        point(
            geometry::Point::new(value.right(), value.bottom()),
            element,
            id,
        )?,
    ))
}

#[cfg(test)]
pub(super) fn for_report(report: &AnalysisReport, sizes: &BTreeMap<usize, Vec2>) -> Placement {
    use egui::{Context, RawInput};
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let index = ReportIndex::new(report);
    let display = DisplayGraph::build(report, &index, super::GraphMode::States, None, None, 2);
    let mut result = Placement::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        result = scene(&prepare(
            report,
            &index,
            &display,
            sizes,
            &BTreeMap::new(),
            ui.painter(),
        ))
        .expect("valid layout fixture");
    });
    output.textures_delta.clear();
    result
}
