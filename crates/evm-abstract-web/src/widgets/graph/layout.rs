//! Conversion between measured egui content and viewport-independent geometry.
//! The private ELK graph is owned by evm-abstract-layout. This layer preserves
//! original leaf/edge IDs and translates complete routes into painter coordinates.
use crate::report::ReportView;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use egui::{FontId, Painter, Vec2};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::EdgeKind;

use super::{index::ReportIndex, projection::DisplayGraph};
use crate::palette;

pub(super) use crate::graph_view::{EdgeRoute, Group, Placement, fit_scale, scene};

pub(super) fn prepare(
    report: &ReportView<'_>,
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
        let mut boundary = display.portals.contains_key(&node.id);
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
            // Geometry needs an ordering key; a process display ID is not a code block.
            block: node.basic_block.unwrap_or(node.id),
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
                    FontId::monospace(10.0),
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

pub(super) fn kind(kind: EdgeKind) -> geometry::EdgeKind {
    match kind {
        // This selects geometric routing only; the wire kind and process label stay intact.
        EdgeKind::Process(_) | EdgeKind::ProcessControl | EdgeKind::Fallthrough => {
            geometry::EdgeKind::Fallthrough
        }
        EdgeKind::Jump => geometry::EdgeKind::Jump,
        EdgeKind::BranchTrue => geometry::EdgeKind::BranchTrue,
        EdgeKind::BranchFalse => geometry::EdgeKind::BranchFalse,
        EdgeKind::Call => geometry::EdgeKind::Call,
        EdgeKind::Return | EdgeKind::SummaryReturn => geometry::EdgeKind::Return,
        EdgeKind::Failure | EdgeKind::SummaryFailure => geometry::EdgeKind::Failure,
        EdgeKind::Revert | EdgeKind::SummaryRevert => geometry::EdgeKind::Revert,
    }
}

#[cfg(test)]
pub(super) fn for_report(report: &ReportView<'_>, sizes: &BTreeMap<usize, Vec2>) -> Placement {
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
