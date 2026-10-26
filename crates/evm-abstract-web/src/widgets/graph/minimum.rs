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
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::{AnalysisReport, SummarySourceStatus};

mod control;
use control::{Card, Control, Edge};

pub(super) struct Scene {
    summary: usize,
    placement: super::Placement,
    edges: Vec<Edge>,
    nodes: usize,
    cards: Vec<Card>,
    source_edges: usize,
    hubs: usize,
    calls: usize,
    deliveries: usize,
    selected: Option<usize>,
    error: Option<geometry::LayoutError>,
    nodes_visible: super::Visibility,
    edges_visible: super::Visibility,
}

impl Scene {
    fn new(report: &AnalysisReport, summary: usize, ui: &Ui) -> Self {
        let model = report.minimum.as_ref().unwrap();
        let relation = &model.summaries[summary].relation;
        let control = Control::new(report, summary);
        let edges = control.edges;
        let input = geometry::Input {
            nodes: control
                .cards
                .iter()
                .enumerate()
                .map(|(id, card)| geometry::Node {
                    id,
                    size: geometry::Size::new(330.0, 86.0),
                    program: None,
                    // Private display ordering is not a Native block assertion.
                    block: id,
                    frame_depth: None,
                    boundary: card.boundary,
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
        Self {
            summary,
            placement,
            nodes: relation.regional_cfg.nodes.len(),
            cards: control.cards,
            source_edges: control.source_edges,
            hubs: control.hubs,
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
            nodes_visible,
            edges_visible,
        }
    }
}

impl Graph {
    pub(crate) fn show_minimum(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        identity: ReportViewId,
        selection: &mut Selection,
    ) {
        if ui.is_sizing_pass() {
            return;
        }
        self.activate(identity);
        let model = report.minimum.as_deref().unwrap();
        let Some(summary) = model.summaries.get(selection.summary) else {
            heading(ui, "GLOBAL CONTROL", "No procedure relation attached");
            self.viewport = Some(ui.available_size());
            self.scene.minimum = Some(Scene {
                summary: selection.summary,
                placement: super::Placement::default(),
                edges: Vec::new(),
                nodes: 0,
                cards: Vec::new(),
                source_edges: 0,
                hubs: 0,
                calls: 0,
                deliveries: 0,
                selected: None,
                error: None,
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
            self.scene.minimum = Some(Scene::new(report, selection.summary, ui));
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
            let card = &scene.cards[id];
            let node = &summary.relation.regional_cfg.nodes[card.node];
            let Some(world) = scene.placement.nodes.get(&id) else {
                continue;
            };
            let rect = self.screen_rect(canvas, *world);
            if !rect.intersects(canvas) {
                continue;
            }
            let selected = selection.node == Some(node.id) && selection.control == card.location;
            let current = source == SummarySourceStatus::Current
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current
                        && transfer.input_revision == node.input_revision
                        && transfer.complete
                })
                && !node.excluded;
            let color = if selected {
                palette::ACCENT
            } else if card.boundary {
                palette::WARNING
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
                &card.title,
                font.clone(),
                palette::TEXT,
            );
            if self.zoom >= 0.25 {
                painter.text(
                    at + egui::vec2(0.0, 22.0 * self.zoom),
                    Align2::LEFT_TOP,
                    &card.caption,
                    FontId::proportional(11.0 * self.zoom),
                    color,
                );
                painter.text(
                    at + egui::vec2(0.0, 43.0 * self.zoom),
                    Align2::LEFT_TOP,
                    &card.qualifier,
                    FontId::monospace(10.0 * self.zoom),
                    palette::MUTED,
                );
            }
            let hit = ui.interact(
                rect.intersect(canvas),
                ui.id().with(("minimum_control", selection.summary, id)),
                Sense::click(),
            );
            if hit.clicked() {
                selection.select_node(model, node.id);
                selection.control = card.location;
            }
            hit.on_hover_text(format!(
                "{}\n{}\n{}\n{}\n{}",
                card.title,
                card.caption,
                card.hover,
                crate::report::regional_status(node, source),
                "Instruction-level SSA belongs to Detailed paths."
            ));
        }
        self.scene.minimum.as_mut().unwrap().selected = selection.node;
    }

    pub(super) fn minimum_summary(&self) -> String {
        self.scene.minimum.as_ref().map_or_else(|| "Global control: awaiting display".into(), |scene| format!("Global control: {} regional nodes; {} MAY source points; {} MAY source edges; {} expanded unknown-jump hubs; {} calls; {} output deliveries; Selected regional {}; Global zoom: {:.3}%", scene.nodes, scene.cards.len(), scene.source_edges, scene.hubs, scene.calls, scene.deliveries, scene.selected.map_or("none".into(), |id| format!("r{id}")), self.zoom * 100.0))
    }
}
