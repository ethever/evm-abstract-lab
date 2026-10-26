//! Scoped conservative control. Its cards retain regional provenance; camera,
//! gestures, LOD and edge routing follow the same rules as the Native view.
use super::{Graph, edges, paint as lod};
use crate::{
    app::{minimum::Selection, model::ReportViewId},
    controls::{self, Density},
    palette,
    widgets::heading,
};
use egui::{Align2, FontId, Pos2, Rect, Sense, Ui, Vec2};
use evm_abstract_protocol::AnalysisReport;

pub(super) mod control;
use control::{Card, GraphLevel};
mod paint;
mod picker;
mod scene;
mod text;
use super::{ControlOrigin, GraphIntent, GraphScope, ProjectionTarget};
mod interaction;
use interaction::CardAction;
use scene::Picker;
pub(super) use scene::{LayoutMemory, Scene};

impl Graph {
    /// Legacy selection adapter. The canonical shell consumes the same render's
    /// typed intent through show_minimum_scoped, including directory-only picks.
    pub(crate) fn show_minimum(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        identity: ReportViewId,
        selection: &mut Selection,
    ) {
        let _ = self.show_minimum_scoped(ui, report, identity, selection);
    }

    pub(crate) fn show_minimum_scoped(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        identity: ReportViewId,
        selection: &mut Selection,
    ) -> Option<GraphIntent> {
        if ui.is_sizing_pass() {
            return None;
        }
        self.activate(identity);
        let fit_requested = self.fit_shortcut(ui);
        let mut scope = if self.whole_scope {
            GraphScope::Overview {
                summary: selection.summary,
            }
        } else {
            self.scene
                .minimum
                .as_ref()
                .filter(|scene| scene.contains(selection))
                .map_or_else(
                    || {
                        selection.node.map_or(
                            GraphScope::Entry {
                                summary: selection.summary,
                            },
                            |node| {
                                ProjectionTarget {
                                    summary: selection.summary,
                                    procedure: selection.procedure,
                                    origin: ControlOrigin {
                                        node,
                                        location: selection.control,
                                    },
                                }
                                .scope()
                            },
                        )
                    },
                    |scene| scene.scope,
                )
        };
        self.ensure_minimum_scope(report, scope, ui);
        let context_caption = &self.scene.minimum.as_ref().unwrap().context_caption;
        heading(
            ui,
            match self.scene.minimum.as_ref().unwrap().control.level {
                GraphLevel::Source => "SOURCE CFG",
                GraphLevel::AbstractBoundary => "ABSTRACT CONTROL",
                GraphLevel::Model => "MODEL OVERVIEW",
            },
            context_caption,
        );
        let mut entry_requested = false;
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if controls::action(ui, "Entry", Density::Compact).clicked() {
                    entry_requested = true;
                }
                if controls::action(ui, "Fit graph", Density::Compact).clicked() { self.request_fit(ui, true); }
                if controls::action_enabled(ui, selection.node.is_some(), "Focus selected", Density::Compact).clicked() { self.focus_selection(); }
                if ui.checkbox(&mut self.whole_scope, "Model overview").changed() {
                    scope = if self.whole_scope { GraphScope::Overview { summary: selection.summary } }
                        else { GraphScope::Context { summary: selection.summary, procedure: selection.procedure,
                            anchor: selection.node.map(|node| ControlOrigin { node, location: selection.control }) } };
                }
                if let Some(scene) = self.scene.minimum.as_mut() && let Some(set) = scene.control.hidden_set
                    && controls::action(ui, "Hidden control", Density::Compact).clicked() { scene.picker = Some(Picker::Targets(set)); }
                ui.label(format!("{:.0}% · {}", self.camera().0 * 100.0, if self.fitted { "Fit" } else { "Pan / zoom" }))
                    .on_hover_text("Drag or scroll to pan; pinch or Ctrl/Cmd+scroll to zoom. F fits this scope; Entry restores readable scale.");
            });
        });
        self.ensure_minimum_scope(report, scope, ui);
        let scene = self.scene.minimum.as_ref().unwrap();
        let counts = scene.control.counts;
        let mut model_requested = false;
        let model_set = scene.control.model_set;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} / {} source points in scope",
                    counts.displayed_points, counts.total_points
                ))
                .small()
                .color(palette::MUTED),
            );
            if counts.unexpanded_points > 0 {
                ui.label(
                    egui::RichText::new(format!("{} unexpanded", counts.unexpanded_points))
                        .small()
                        .color(palette::WARNING),
                );
            }
            ui.menu_button("Advanced", |ui| {
                ui.label(format!(
                    "{} / {} context receipts · {} other-context receipts",
                    counts.displayed_receipts,
                    counts.context_receipts,
                    counts.other_context_receipts
                ));
                ui.label(format!(
                    "{} folded flows · {} calls · {} deliveries",
                    counts.hidden_flows, counts.calls, counts.deliveries
                ));
                ui.label(format!(
                    "{} source edges · {} unknown-jump hubs",
                    scene.control.source_edges, scene.control.hubs
                ));
                ui.label(format!(
                    "{} offscreen routes deferred; pan or Fit to reveal",
                    self.scene.deferred_edges
                ));
                ui.label(format!(
                    "{} model relations summarized · {} untyped topology memberships",
                    counts.summarized_model_relations, counts.retained_topology
                ));
                if controls::action_enabled(
                    ui,
                    model_set.is_some(),
                    "Inspect model relations",
                    Density::Compact,
                )
                .clicked()
                {
                    model_requested = true;
                }
                ui.monospace(format!("{:?}", scene.scope));
            });
        });
        if model_requested && let Some(set) = model_set {
            self.scene.minimum.as_mut().unwrap().picker = Some(Picker::Targets(set));
        }
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        if canvas.size().min_elem() <= 1.0 || ui.ctx().will_discard() {
            return None;
        }
        self.viewport = Some(canvas.size());
        let scene = self.scene.minimum.as_ref().unwrap();
        let selected = scene
            .control
            .cards
            .iter()
            .position(|card| card_selected(card, selection));
        let anchor = if self.focus_pending {
            selected
        } else {
            scene.control.anchor.or(selected)
        };
        let anchor = anchor
            .and_then(|id| scene.placement.nodes.get(&id))
            .copied();
        if self.fit_pending {
            self.fit_bounds(canvas.size(), scene.placement.bounds);
            self.fit_pending = false;
            self.entry_pending = false;
            self.fitted = true;
        } else if self.entry_pending || self.focus_pending {
            let bounds = if self.focus_pending {
                anchor.unwrap_or(scene.placement.bounds)
            } else {
                scene.placement.bounds
            };
            self.readable_entry(canvas.size(), bounds, anchor);
            self.entry_pending = false;
        }
        self.focus_pending = false;
        self.camera_restored = false;
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
                format!("Control scope unavailable: {error}"),
                FontId::proportional(13.0),
                palette::WARNING,
            );
            return None;
        }
        let world_view = Rect::from_min_max(
            Pos2::ZERO - self.pan / self.zoom,
            Pos2::ZERO + (canvas.size() - self.pan) / self.zoom,
        );
        let visible = world_view.expand(16.0 / self.zoom);
        let mut animated = false;
        let mut deferred_edges = 0;
        for id in scene.edges_visible.query(visible) {
            let edge = &scene.control.edges[id];
            let Some(route) = scene.placement.edges.get(&id) else {
                continue;
            };
            if !edges::has_visible_geometry(
                route,
                visible,
                self.zoom >= 0.9 && !edge.label.is_empty(),
            ) {
                deferred_edges += 1;
                continue;
            }
            let highlighted = Some(edge.from) == selected || Some(edge.to) == selected;
            let label = self.screen_rect(canvas, route.label).intersect(canvas);
            if self.zoom >= 0.9 && label.is_positive() {
                ui.interact(label, ui.id().with(("control_edge", id)), Sense::hover())
                    .on_hover_ui(|ui| {
                        ui.label(format!(
                            "{:?} · {} independent flow origins",
                            edge.level,
                            edge.origins.len()
                        ));
                        for origin in edge.origins.iter().take(12) {
                            ui.label(origin.detail(report, scope.summary()));
                        }
                    });
            }
            animated |= edges::paint_edge_style(
                &painter,
                route,
                canvas.min + self.pan,
                edges::EdgeStyle {
                    kind: edge.kind,
                    label: &edge.label,
                    zoom: self.zoom,
                    selected: highlighted,
                    color: Some(edge.color),
                },
            );
        }
        if animated {
            ui.ctx().request_repaint_after(edges::FLOW_FRAME_INTERVAL);
        }
        let level = lod::detail_level(ui, self.zoom);
        let clicked = self.minimum_cards(ui, &painter, canvas, visible, level, selection);
        self.scene.deferred_edges = deferred_edges;
        let mut intent = None;
        if let Some(action) = clicked {
            let scene = self.scene.minimum.as_mut().unwrap();
            let (id, targets) = match action {
                CardAction::Receipts(id) => (id, false),
                CardAction::Targets(id) => (id, true),
            };
            let card = &scene.control.cards[id];
            if targets || card.origins.is_empty() {
                if let Some(set) = card.target_set {
                    scene.picker = Some(Picker::Targets(set));
                }
            } else if card.origins.len() == 1 {
                intent = Some(GraphIntent::Select(card.origins[0].target));
            } else {
                scene.picker = Some(Picker::Origins(id));
            }
        }
        if ui.is_enabled() {
            intent = self.minimum_picker(ui, report, canvas).or(intent);
        }
        if entry_requested {
            intent = report
                .minimum
                .as_ref()
                .and_then(|model| model.summaries.get(selection.summary))
                .and_then(|summary| {
                    summary
                        .relation
                        .root_procedure
                        .map(|procedure| (summary, procedure))
                })
                .and_then(|(summary, procedure)| {
                    summary
                        .relation
                        .regional_cfg
                        .nodes
                        .iter()
                        .find(|node| node.procedure == procedure)
                        .map(|node| {
                            GraphIntent::Open(ProjectionTarget {
                                summary: selection.summary,
                                procedure,
                                origin: ControlOrigin {
                                    node: node.id,
                                    location: None,
                                },
                            })
                        })
                });
            self.whole_scope = false;
        }
        if let Some(GraphIntent::Select(target) | GraphIntent::Open(target)) = intent {
            apply_target(report, selection, target);
        }
        self.scene.minimum.as_mut().unwrap().selected = selection.node;
        if entry_requested {
            self.scene.minimum = None;
        }
        intent
    }

    pub(super) fn minimum_summary(&self) -> String {
        self.scene.minimum.as_ref().map_or_else(|| "Scoped control: awaiting display".into(), |scene| {
            let counts = scene.control.counts;
            format!("Scoped control: {:?}; {} / {} MAY source points; {} / {} context receipts; {} other-context receipts; {} portals; {} folded flows; {} / {} unexpanded points; {} calls; {} output deliveries; {} source edges; {} unknown-jump hubs; Selected regional {}; {:?} level; {} summarized model relations; {} retained topology memberships; {} retained model rows; Global zoom: {:.3}%",
                scene.scope, counts.displayed_points, counts.total_points, counts.displayed_receipts, counts.context_receipts, counts.other_context_receipts,
                scene.control.target_sets.len(), counts.hidden_flows, counts.displayed_unexpanded_points, counts.unexpanded_points, counts.calls, counts.deliveries, scene.control.source_edges, scene.control.hubs,
                scene.selected.map_or("none".into(), |id| format!("r{id}")), scene.control.level, counts.summarized_model_relations, counts.retained_topology, scene.control.model_relations.len(), self.zoom * 100.0)
        })
    }
}

fn card_selected(card: &Card, selection: &Selection) -> bool {
    card.origins.iter().any(|origin| {
        origin.target.summary == selection.summary
            && origin.target.procedure == selection.procedure
            && Some(origin.target.origin.node) == selection.node
            && origin.target.origin.location == selection.control
    })
}

fn apply_target(report: &AnalysisReport, selection: &mut Selection, target: ProjectionTarget) {
    selection.summary = target.summary;
    if let Some(model) = report.minimum.as_deref() {
        selection.select_node(model, target.origin.node);
    }
    selection.procedure = target.procedure;
    selection.node = Some(target.origin.node);
    selection.control = target.origin.location;
    selection.output = None;
    selection.account = None;
}

#[cfg(test)]
mod tests;
