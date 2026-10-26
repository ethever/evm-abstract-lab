//! Native scene painting and hit-testing over cached, scoped display data.
use super::{
    Graph, NativeIntent,
    content::{self, node_preview_with_display, node_tooltip},
    edges, groups, paint,
};
use crate::{app::Selection, palette, report::ReportView, widgets::heading};
use edges::paint_edge_text;
use egui::{Align2, FontId, Pos2, Rect, Sense, Ui, Vec2};

impl Graph {
    pub(crate) fn show(&mut self, ui: &mut Ui, report: &ReportView<'_>, selection: &mut Selection) {
        if let Some(NativeIntent::Select(picked) | NativeIntent::Open(picked)) =
            self.show_scoped(ui, report, selection)
        {
            *selection = picked;
        }
    }

    pub(crate) fn show_scoped(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
    ) -> Option<NativeIntent> {
        if ui.is_sizing_pass() {
            return None;
        }
        self.activate(report.identity());
        self.set_ssa_display(self.ssa_display, *selection);
        self.ensure_scene(report, selection.state);
        let fit_requested = self.fit_shortcut(ui);
        heading(
            ui,
            "CONTROL FLOW",
            &format!(
                "{} nodes · {} / {} states · {} edges",
                self.scene.display.nodes.len(),
                self.scene.shown_states,
                report.cfg.len(),
                self.scene.display.edges.len()
            ),
        );
        self.controls(ui, report, selection);
        self.ensure_scene(report, selection.state);
        self.set_ssa_display(self.ssa_display, *selection);
        self.view_legend(ui);
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(canvas, 3.0, palette::BACKGROUND);
        if self.scene.nodes.is_empty()
            && let Some(index) = &self.scene.index
        {
            self.scene.nodes = self
                .scene
                .display
                .nodes
                .iter()
                .map(|node| {
                    (
                        node.id,
                        if let Some(portal) = self.scene.display.portals.get(&node.id) {
                            content::portal_preview(&painter, report, index, portal)
                        } else {
                            node_preview_with_display(
                                &painter,
                                report,
                                index,
                                node,
                                self.content,
                                self.ssa_display,
                                self.scene.ssa_selection,
                            )
                        },
                    )
                })
                .collect();
            self.scene.preview_dirty.clear();
        } else if let Some(index) = &self.scene.index {
            for id in std::mem::take(&mut self.scene.preview_dirty) {
                let Some(node) = self
                    .scene
                    .node_positions
                    .get(&id)
                    .and_then(|position| self.scene.display.nodes.get(*position))
                else {
                    continue;
                };
                self.scene.nodes.insert(
                    id,
                    node_preview_with_display(
                        &painter,
                        report,
                        index,
                        node,
                        self.content,
                        self.ssa_display,
                        self.scene.ssa_selection,
                    ),
                );
            }
        }
        if ui.ctx().will_discard() || !self.update_viewport(report, canvas.size(), &painter) {
            return None;
        }
        let selected_node = selection
            .state
            .and_then(|state| self.scene.display.state_to_node.get(&state).copied());
        if self.entry_pending {
            let anchor = selected_node
                .or_else(|| self.scene.display.nodes.first().map(|node| node.id))
                .and_then(|id| self.scene.placement.nodes.get(&id))
                .copied();
            self.readable_entry(canvas.size(), self.scene.placement.bounds, anchor);
            self.entry_pending = false;
        }
        self.camera_restored = false;
        if self.focus_pending {
            if let Some(rect) = selected_node
                .and_then(|id| self.scene.placement.nodes.get(&id))
                .copied()
            {
                self.readable_entry(canvas.size(), rect, Some(rect));
            }
            self.focus_pending = false;
        }
        if !fit_requested {
            self.navigate(ui, &response);
        }
        self.grid(&painter, canvas);
        if let Some(error) = &self.scene.layout_error {
            crate::notation::paint(
                &painter,
                canvas.center(),
                Align2::CENTER_CENTER,
                &format!("Unable to arrange this graph: {error}"),
                FontId::monospace(12.0),
                palette::WARNING,
            );
            return None;
        }
        self.update_highlight(report, selection.state);
        let world_view = Rect::from_min_max(
            Pos2::ZERO - self.pan / self.zoom,
            Pos2::ZERO + (canvas.size() - self.pan) / self.zoom,
        );
        let visible = world_view.expand(16.0 / self.zoom);
        groups::paint(
            ui,
            &painter,
            &self.scene.placement.groups,
            canvas,
            self.pan,
            self.zoom,
            visible,
        );
        let mut animating_edges = false;
        let mut deferred_edges = 0;
        for id in self.scene.edge_visibility.query(visible) {
            let Some(route) = self.scene.placement.edges.get(&id) else {
                continue;
            };
            let Some(edge) = self
                .scene
                .edge_positions
                .get(&id)
                .and_then(|position| self.scene.display.edges.get(*position))
            else {
                continue;
            };
            let label = self.scene.edge_labels.get(&id).map_or("", String::as_str);
            if !edges::has_visible_geometry(route, visible, self.zoom >= 0.9 && !label.is_empty()) {
                deferred_edges += 1;
                continue;
            }
            animating_edges |= paint_edge_text(
                &painter,
                route,
                canvas.min + self.pan,
                edge.kind,
                label,
                self.zoom,
                self.scene.highlighted_edges.contains(&id),
            );
            let label_rect = self.screen_rect(canvas, route.label);
            if canvas.intersects(label_rect) {
                let hover = ui.interact(
                    label_rect.intersect(canvas),
                    ui.id().with(("cfg_edge", id)),
                    Sense::hover(),
                );
                hover.on_hover_ui(|ui| self.edge_detail(ui, report, id));
            }
        }
        self.scene.deferred_edges = deferred_edges;
        if animating_edges {
            ui.ctx().request_repaint_after(edges::FLOW_FRAME_INTERVAL);
        }
        let detail_level = paint::detail_level(ui, self.zoom);
        self.scene.detail_level = detail_level;
        let mut open_state = None;
        let mut intent = None;
        for id in self.scene.node_visibility.query(visible) {
            let (Some(world), Some(content)) = (
                self.scene.placement.nodes.get(&id),
                self.scene.nodes.get(&id),
            ) else {
                continue;
            };
            let Some(node) = self
                .scene
                .node_positions
                .get(&id)
                .and_then(|position| self.scene.display.nodes.get(*position))
            else {
                continue;
            };
            let rect = self.screen_rect(canvas, *world);
            if !canvas.intersects(rect) {
                continue;
            }
            let hit = ui.interact(
                rect.intersect(canvas),
                ui.id().with(("cfg_node", id)),
                Sense::click(),
            );
            hit.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    selected_node == Some(id),
                    &content.title,
                )
            });
            paint::node(
                &painter,
                rect,
                content,
                selected_node == Some(id),
                hit.hovered() || hit.has_focus(),
                self.zoom,
                detail_level,
            );
            if hit.clicked() {
                if let Some(portal) = self.scene.display.portals.get(&id) {
                    if portal.targets.len() == 1 {
                        open_state = portal.targets.first().copied();
                    } else {
                        self.scene.instance_group = Some(id);
                        self.scene.instance_filter.clear();
                    }
                } else {
                    let state = selection
                        .state
                        .filter(|state| self.scene.display.state_to_node.get(state) == Some(&id))
                        .or_else(|| node.members.first().copied());
                    *selection = Selection {
                        value: None,
                        state,
                        pc: None,
                    };
                    intent = Some(NativeIntent::Select(*selection));
                    if node.members.len() > 1 {
                        self.scene.instance_group = Some(id);
                        self.scene.instance_filter.clear();
                    }
                }
                ui.ctx().request_repaint();
            }
            hit.on_hover_ui(|ui| {
                if self.scene.display.portals.contains_key(&id) {
                    ui.label("Open recorded adjacent states. Each destination retains its own source, owner, caller and frame context.");
                    return;
                }
                if let Some(index) = &self.scene.index {
                    ui.label(crate::notation::widget(
                        ui,
                        egui::RichText::new(node_tooltip(report, index, node, self.content))
                            .monospace(),
                    ));
                }
            });
        }
        if let Some(state) = open_state {
            self.open_native_state(report, selection, state);
            intent = Some(NativeIntent::Open(*selection));
        }
        if self.scene.display.nodes.is_empty() {
            painter.text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "No reachable states in this view",
                FontId::proportional(13.0),
                palette::MUTED,
            );
        }
        if ui.is_enabled() {
            intent = self.instances(ui.ctx(), report, selection).or(intent);
        }
        intent
    }
}
