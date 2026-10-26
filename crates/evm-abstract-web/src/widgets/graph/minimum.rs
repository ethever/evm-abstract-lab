//! Canonical Global control uses regional identities, never Native machine/SSA
//! coordinates. It shares the existing camera, gestures and layout engine.

use super::{Graph, layout};
use crate::{
    app::{
        minimum::{self, Selection},
        model::ReportViewId,
    },
    controls::{self, Density},
    palette,
    widgets::heading,
};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::{EdgeKind, MinimumModel, RegionalNode, SummarySourceStatus};

struct Edge {
    from: usize,
    to: usize,
    kind: EdgeKind,
    label: String,
    color: Color32,
}

pub(super) struct Scene {
    summary: usize,
    placement: super::Placement,
    edges: Vec<Edge>,
    nodes: usize,
    calls: usize,
    deliveries: usize,
    selected: Option<usize>,
    error: Option<geometry::LayoutError>,
    positions: std::collections::BTreeMap<usize, usize>,
    nodes_visible: super::Visibility,
    edges_visible: super::Visibility,
}

fn collect_edges(model: &MinimumModel, summary: usize) -> Vec<Edge> {
    let cfg = &model.summaries[summary].relation.regional_cfg;
    let mut edges = Vec::new();
    for node in &cfg.nodes {
        for &to in &node.successors {
            let kind = node
                .transfer
                .as_ref()
                .and_then(|transfer| transfer.successors.iter().find(|edge| edge.to == to))
                .map_or(EdgeKind::Fallthrough, |edge| edge.kind);
            edges.push(Edge {
                from: node.id,
                to,
                kind,
                label: format!("MAY {kind:?}"),
                color: palette::MUTED,
            });
        }
    }
    for call in &cfg.calls {
        for &to in &call.entries {
            edges.push(Edge {
                from: call.from,
                to,
                kind: EdgeKind::Call,
                label: format!("call c{}", call.id),
                color: palette::PURPLE,
            });
        }
        for delivery in &call.deliveries {
            for target in &delivery.successors {
                edges.push(Edge {
                    from: call.from,
                    to: target.to,
                    kind: target.kind,
                    label: format!(
                        "c{} {:?} · generation {}",
                        call.id, delivery.kind, delivery.output_generation
                    ),
                    color: if delivery.current && delivery.input_revision == call.input_revision {
                        palette::BLUE
                    } else {
                        palette::MUTED
                    },
                });
            }
        }
    }
    edges
}

impl Scene {
    fn new(model: &MinimumModel, summary: usize, ui: &Ui) -> Self {
        let relation = &model.summaries[summary].relation;
        let edges = collect_edges(model, summary);
        let input = geometry::Input {
            nodes: relation
                .regional_cfg
                .nodes
                .iter()
                .map(|node| geometry::Node {
                    id: node.id,
                    size: geometry::Size::new(260.0, 77.0),
                    program: None,
                    // The geometry-only grouping coordinate is the actual regional
                    // ID. No program/basic-block assertion is exported from it.
                    block: node.id,
                    frame_depth: None,
                    boundary: node.excluded || node.transfer.is_none(),
                })
                .collect(),
            edges: edges
                .iter()
                .enumerate()
                .map(|(id, edge)| {
                    let measured = ui
                        .painter()
                        .layout_no_wrap(edge.label.clone(), FontId::monospace(10.0), edge.color)
                        .size();
                    geometry::Edge {
                        id,
                        from: edge.from,
                        to: edge.to,
                        kind: layout::kind(edge.kind),
                        label: geometry::Label {
                            text: edge.label.clone(),
                            size: geometry::Size::new(
                                f64::from(measured.x + 8.0),
                                f64::from(measured.y + 4.0),
                            ),
                        },
                    }
                })
                .collect(),
        };
        let (placement, error) = match layout::scene(&input) {
            Ok(layout) => (layout, None),
            Err(error) => (super::Placement::default(), Some(error)),
        };
        let nodes_visible = super::Visibility::new(&placement.nodes);
        let edge_bounds = placement
            .edges
            .iter()
            .map(|(id, route)| {
                let mut bounds = route.label;
                for point in &route.path {
                    bounds.extend_with(*point);
                }
                (*id, bounds)
            })
            .collect();
        let edges_visible = super::Visibility::new(&edge_bounds);
        let positions = relation
            .regional_cfg
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id, index))
            .collect();
        Self {
            summary,
            placement,
            nodes: relation.regional_cfg.nodes.len(),
            calls: relation.regional_cfg.calls.len(),
            deliveries: relation
                .regional_cfg
                .calls
                .iter()
                .map(|call| call.deliveries.len())
                .sum(),
            edges,
            selected: None,
            error,
            positions,
            nodes_visible,
            edges_visible,
        }
    }
}

fn title(node: &RegionalNode) -> String {
    format!("r{} · procedure {}", node.id, node.procedure)
}

impl Graph {
    pub(crate) fn show_minimum(
        &mut self,
        ui: &mut Ui,
        model: &MinimumModel,
        identity: ReportViewId,
        selection: &mut Selection,
    ) {
        if ui.is_sizing_pass() {
            return;
        }
        self.activate(identity);
        let Some(summary) = model.summaries.get(selection.summary) else {
            heading(ui, "GLOBAL CONTROL", "No procedure relation attached");
            self.viewport = Some(ui.available_size());
            self.scene.minimum = Some(Scene {
                summary: selection.summary,
                placement: super::Placement::default(),
                edges: Vec::new(),
                nodes: 0,
                calls: 0,
                deliveries: 0,
                selected: None,
                error: None,
                positions: Default::default(),
                nodes_visible: Default::default(),
                edges_visible: Default::default(),
            });
            crate::widgets::empty(ui, "No procedure relation is attached to this observation");
            return;
        };
        let fit_requested = self.fit_shortcut(ui);
        heading(
            ui,
            "GLOBAL CONTROL",
            &format!(
                "{} regional nodes · {} calls · source-qualified MAY control",
                summary.relation.regional_cfg.nodes.len(),
                summary.relation.regional_cfg.calls.len()
            ),
        );
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if controls::action(ui, "Fit graph", Density::Compact).clicked() {
                    self.request_fit(ui, true);
                }
                ui.label(format!(
                    "{:.0}% · {}",
                    self.zoom * 100.0,
                    if self.fitted { "Fit" } else { "Manual" }
                ));
                ui.label("MAY edges · purple calls · blue output deliveries");
            })
        });
        if self
            .scene
            .minimum
            .as_ref()
            .is_none_or(|scene| scene.summary != selection.summary)
        {
            self.scene.minimum = Some(Scene::new(model, selection.summary, ui));
        }
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        self.viewport = Some(canvas.size());
        if self.fit_pending {
            let bounds = self.scene.minimum.as_ref().unwrap().placement.bounds;
            self.fit_bounds(canvas.size(), bounds);
            self.fit_pending = false;
            self.fitted = true;
        }
        if !fit_requested {
            self.navigate(ui, &response);
        }
        painter.rect_filled(canvas, 3.0, palette::BACKGROUND);
        self.grid(&painter, canvas);
        let scene = self.scene.minimum.as_ref().unwrap();
        if let Some(error) = &scene.error {
            painter.text(
                canvas.center(),
                Align2::CENTER_CENTER,
                format!("Global layout unavailable: {error}"),
                FontId::proportional(13.0),
                palette::WARNING,
            );
            return;
        }
        let visible = Rect::from_min_max(
            Pos2::ZERO - self.pan / self.zoom,
            Pos2::ZERO + (canvas.size() - self.pan) / self.zoom,
        )
        .expand(16.0 / self.zoom);
        for id in scene.edges_visible.query(visible) {
            let edge = &scene.edges[id];
            let Some(route) = scene.placement.edges.get(&id) else {
                continue;
            };
            let path: Vec<_> = route
                .path
                .iter()
                .map(|point| canvas.min + self.pan + point.to_vec2() * self.zoom)
                .collect();
            if let [.., from, to] = path.as_slice() {
                painter.add(egui::Shape::line(
                    path.clone(),
                    Stroke::new(1.2, edge.color),
                ));
                painter.arrow(*from, *to - *from, Stroke::new(1.2, edge.color));
            }
            if self.zoom >= 0.25 {
                let label = self.screen_rect(canvas, route.label);
                painter.rect_filled(label, 2.0, palette::BACKGROUND);
                painter.text(
                    label.center(),
                    Align2::CENTER_CENTER,
                    &edge.label,
                    FontId::monospace(10.0 * self.zoom),
                    edge.color,
                );
            }
        }
        let source = minimum::source_status(model, selection.summary);
        for id in scene.nodes_visible.query(visible) {
            let node = &summary.relation.regional_cfg.nodes[scene.positions[&id]];
            let Some(world) = scene.placement.nodes.get(&node.id) else {
                continue;
            };
            let rect = self.screen_rect(canvas, *world);
            if !rect.intersects(canvas) {
                continue;
            }
            let selected = selection.node == Some(node.id);
            let current = source == SummarySourceStatus::Current
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current
                        && transfer.input_revision == node.input_revision
                        && transfer.complete
                })
                && !node.excluded;
            let color = if selected {
                palette::ACCENT
            } else if current {
                palette::BLUE
            } else {
                palette::MUTED
            };
            painter.rect_filled(
                rect,
                4.0,
                if selected {
                    palette::SELECTED
                } else {
                    palette::RAISED
                },
            );
            painter.rect_stroke(
                rect,
                4.0,
                Stroke::new(if selected { 2.0 } else { 1.0 }, color),
                StrokeKind::Inside,
            );
            let at = rect.min + Vec2::splat(7.0 * self.zoom);
            let font = FontId::monospace((12.0 * self.zoom).max(7.0));
            painter.text(
                at,
                Align2::LEFT_TOP,
                title(node),
                font.clone(),
                palette::TEXT,
            );
            if self.zoom >= 0.25 {
                painter.text(
                    at + egui::vec2(0.0, 22.0 * self.zoom),
                    Align2::LEFT_TOP,
                    "Source-qualified possible control",
                    FontId::proportional(11.0 * self.zoom),
                    color,
                );
                painter.text(
                    at + egui::vec2(0.0, 43.0 * self.zoom),
                    Align2::LEFT_TOP,
                    format!(
                        "input revision {} · {:?}",
                        node.input_revision, node.frame.kind
                    ),
                    FontId::monospace(10.0 * self.zoom),
                    palette::MUTED,
                );
            }
            let hit = ui.interact(
                rect.intersect(canvas),
                ui.id()
                    .with(("minimum_control", selection.summary, node.id)),
                Sense::click(),
            );
            if hit.clicked() {
                selection.select_node(model, node.id);
            }
            hit.on_hover_text(format!("{}\n{}\nRegional identity in this relation; instruction-level SSA belongs to Detailed paths.", title(node), crate::report::regional_status(node, source)));
        }
        self.scene.minimum.as_mut().unwrap().selected = selection.node;
    }

    pub(super) fn minimum_summary(&self) -> String {
        self.scene.minimum.as_ref().map_or_else(|| "Global control: awaiting display".into(), |scene| format!("Global control: {} regional nodes; {} calls; {} output deliveries; Selected regional {}; Global zoom: {:.3}%", scene.nodes, scene.calls, scene.deliveries, scene.selected.map_or("none".into(), |id| format!("r{id}")), self.zoom * 100.0))
    }
}
