//! CFG views over an immutable native report. Source blocks fold only for
//! display; selecting an instance always uses its original state and SSA IDs.

mod content;
mod controls;
mod edges;
mod groups;
mod index;
mod layout;
mod navigation;
mod paint;
mod projection;
mod scene;
mod termination;
#[cfg(test)]
mod tests;
mod visibility;

use super::heading;
use crate::app::model::ReportViewId;
use crate::report::ReportView;
use crate::{app::Selection, palette, ssa_display::SsaDisplay};
#[cfg(test)]
use content::node_text;
use content::{NodeText, NodeView, node_preview_with_display, node_tooltip};
use edges::{edge_label, paint_edge_text};
use egui::{Align2, FontId, Painter, Pos2, Rect, Sense, Ui, Vec2};
use index::ReportIndex;
use layout::Placement;
use projection::DisplayGraph;
pub(crate) use projection::GraphMode;
use std::collections::{BTreeMap, BTreeSet};
use visibility::Visibility;

const PAD: f32 = 6.0;
const HEADER_HEIGHT: f32 = 33.0;
const LINE_HEIGHT: f32 = 14.0;
/// Preferences and camera survive replacement of the report-derived scene.
pub(crate) struct Graph {
    pub(crate) zoom: f32,
    content: NodeView,
    ssa_display: SsaDisplay,
    mode: GraphMode,
    neighborhood_hops: usize,
    pan: Vec2,
    fitted: bool,
    fit_pending: bool,
    focus_pending: bool,
    cancel_wheel: bool,
    viewport: Option<Vec2>,
    scene: Scene,
}

struct Scene {
    identity: ReportViewId,
    ssa_selection: Selection,
    preview_dirty: BTreeSet<usize>,
    program_scope: Option<usize>,
    local_center: Option<usize>,
    index: Option<ReportIndex>,
    display: DisplayGraph,
    ending_summary: termination::TerminationSummary,
    display_dirty: bool,
    shown_states: usize,
    native_edges: usize,
    node_positions: BTreeMap<usize, usize>,
    edge_positions: BTreeMap<usize, usize>,
    edge_labels: BTreeMap<usize, String>,
    native_edge_to_display: BTreeMap<usize, usize>,
    highlighted_edges: BTreeSet<usize>,
    highlighted_state: Option<usize>,
    highlight_dirty: bool,
    node_visibility: Visibility,
    edge_visibility: Visibility,
    instance_group: Option<usize>,
    instance_filter: String,
    content_anchor: Option<(usize, Vec2)>,
    nodes: BTreeMap<usize, NodeText>,
    placement: Placement,
    layout_error: Option<evm_abstract_layout::LayoutError>,
    detail_level: paint::DetailLevel,
    layout_pending: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            identity: ReportViewId::default(),
            ssa_selection: Selection::default(),
            preview_dirty: BTreeSet::new(),
            program_scope: None,
            local_center: None,
            index: None,
            display: DisplayGraph::default(),
            ending_summary: termination::TerminationSummary::default(),
            display_dirty: true,
            shown_states: 0,
            native_edges: 0,
            node_positions: BTreeMap::new(),
            edge_positions: BTreeMap::new(),
            edge_labels: BTreeMap::new(),
            native_edge_to_display: BTreeMap::new(),
            highlighted_edges: BTreeSet::new(),
            highlighted_state: None,
            highlight_dirty: true,
            node_visibility: Visibility::default(),
            edge_visibility: Visibility::default(),
            instance_group: None,
            instance_filter: String::new(),
            content_anchor: None,
            nodes: BTreeMap::new(),
            placement: Placement::default(),
            layout_error: None,
            detail_level: paint::DetailLevel::Preview,
            layout_pending: true,
        }
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            content: NodeView::default(),
            ssa_display: SsaDisplay::default(),
            mode: GraphMode::default(),
            neighborhood_hops: 2,
            pan: Vec2::ZERO,
            fitted: true,
            fit_pending: true,
            focus_pending: false,
            cancel_wheel: false,
            viewport: None,
            scene: Scene::default(),
        }
    }
}

impl Graph {
    #[cfg(test)]
    pub(crate) fn camera(&self) -> (f32, Vec2) {
        (self.zoom, self.pan)
    }

    /// The visible scene has actually been measured and laid out. Hidden graph
    /// tabs are never built merely to finish the result's timing measurement.
    pub(crate) fn prepared(&self) -> bool {
        self.scene.index.is_some() && !self.scene.layout_pending && self.viewport.is_some()
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, report: &ReportView<'_>, selection: &mut Selection) {
        if ui.is_sizing_pass() {
            return;
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
                        node_preview_with_display(
                            &painter,
                            report,
                            index,
                            node,
                            self.content,
                            self.ssa_display,
                            self.scene.ssa_selection,
                        ),
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
            return;
        }
        let selected_node = selection
            .state
            .and_then(|state| self.scene.display.state_to_node.get(&state).copied());
        if self.focus_pending {
            if let Some(rect) = selected_node
                .and_then(|id| self.scene.placement.nodes.get(&id))
                .copied()
            {
                self.fitted = false;
                self.zoom = ((canvas.width() - 12.0).max(1.0) / rect.width())
                    .min((canvas.height() - 12.0).max(1.0) / rect.height())
                    .min(1.0);
                self.pan = canvas.size() * 0.5 - rect.center().to_vec2() * self.zoom;
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
            return;
        }
        self.update_highlight(report, selection.state);
        let visible = Rect::from_min_max(
            Pos2::ZERO - self.pan / self.zoom,
            Pos2::ZERO + (canvas.size() - self.pan) / self.zoom,
        )
        .expand(16.0 / self.zoom);
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
        if animating_edges {
            ui.ctx().request_repaint_after(edges::FLOW_FRAME_INTERVAL);
        }
        let detail_level = paint::detail_level(ui, self.zoom);
        self.scene.detail_level = detail_level;
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
            paint::node(
                &painter,
                rect,
                content,
                selected_node == Some(id),
                hit.hovered(),
                self.zoom,
                detail_level,
            );
            if hit.clicked() {
                let state = selection
                    .state
                    .filter(|state| self.scene.display.state_to_node.get(state) == Some(&id))
                    .or_else(|| node.members.first().copied());
                *selection = Selection {
                    value: None,
                    state,
                    pc: None,
                };
                if node.members.len() > 1 {
                    self.scene.instance_group = Some(id);
                    self.scene.instance_filter.clear();
                }
                ui.ctx().request_repaint();
            }
            hit.on_hover_ui(|ui| {
                if let Some(index) = &self.scene.index {
                    ui.label(crate::notation::widget(
                        ui,
                        egui::RichText::new(node_tooltip(report, index, node, self.content))
                            .monospace(),
                    ));
                }
            });
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
            self.instances(ui.ctx(), report, selection);
        }
    }

    /// Bind an immutable graph namespace. Refreshes replace only derived data;
    /// unrelated reports additionally request their initial fit.
    pub(crate) fn activate(&mut self, identity: ReportViewId) {
        if self.scene.identity == identity {
            return;
        }
        let replacement = !self.scene.identity.same_report(identity);
        self.scene = Scene {
            identity,
            ..Scene::default()
        };
        if replacement {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            self.viewport = None;
            self.queue_fit();
        }
    }

    pub(crate) fn content_label(&self) -> &'static str {
        match self.content {
            NodeView::Disassembly => "Disasm",
            NodeView::Ssa => "SSA",
        }
    }

    /// SSA detail choices change cached text only. Both bounded presentations
    /// reserve the same natural geometry, so native routes and camera stay put.
    pub(crate) fn set_ssa_display(&mut self, display: SsaDisplay, selection: Selection) {
        if self.ssa_display != display {
            self.ssa_display = display;
            if self.content == NodeView::Ssa {
                self.scene.nodes.clear();
                self.scene.preview_dirty.clear();
            }
        }
        if !self.scene.ssa_selection.same_native(selection)
            && self.ssa_display == SsaDisplay::Compact
            && self.content == NodeView::Ssa
        {
            for selection in [self.scene.ssa_selection, selection] {
                if let (Some(state), Some(_)) = (selection.state, selection.pc)
                    && let Some(id) = self.scene.display.state_to_node.get(&state)
                    && self.scene.node_positions.get(id).is_some_and(|position| {
                        self.scene.display.nodes[*position].members.as_slice() == [state]
                    })
                {
                    self.scene.preview_dirty.insert(*id);
                }
            }
        }
        self.scene.ssa_selection = selection;
    }

    pub(crate) fn set_mode(&mut self, mode: GraphMode) {
        if self.mode != mode {
            self.mode = mode;
            self.invalidate_scene();
        }
    }

    fn invalidate_scene(&mut self) {
        self.scene.display_dirty = true;
        self.scene.nodes.clear();
        self.scene.preview_dirty.clear();
        self.scene.content_anchor = None;
        self.scene.instance_group = None;
        self.scene.layout_error = None;
        self.scene.layout_pending = true;
        self.fit_pending = true;
        self.fitted = true;
    }

    fn set_content(&mut self, content: NodeView, selected: Option<usize>) {
        if content == self.content {
            return;
        }
        self.scene.content_anchor = self.viewport.and_then(|viewport| {
            let screen = Rect::from_min_size(Pos2::ZERO, viewport);
            let selected = selected
                .and_then(|state| self.scene.display.state_to_node.get(&state))
                .and_then(|id| self.scene.placement.nodes.get(id).map(|rect| (*id, rect)))
                .filter(|(_, rect)| {
                    screen.contains(Pos2::ZERO + self.pan + rect.center().to_vec2() * self.zoom)
                });
            let center = (viewport * 0.5 - self.pan) / self.zoom;
            selected
                .or_else(|| {
                    self.scene
                        .placement
                        .nodes
                        .iter()
                        .min_by(|(_, a), (_, b)| {
                            (a.center().to_vec2() - center)
                                .length_sq()
                                .total_cmp(&(b.center().to_vec2() - center).length_sq())
                        })
                        .map(|(id, rect)| (*id, rect))
                })
                .map(|(id, rect)| (id, self.pan + rect.center().to_vec2() * self.zoom))
        });
        self.content = content;
        self.scene.nodes.clear();
        self.scene.preview_dirty.clear();
        self.scene.layout_pending = true;
        self.fitted = false;
    }

    fn queue_fit(&mut self) {
        self.fitted = true;
        self.fit_pending = true;
        self.focus_pending = false;
    }

    fn update_viewport(&mut self, report: &ReportView<'_>, size: Vec2, painter: &Painter) -> bool {
        if !size.is_finite() || size.min_elem() <= 1.0 {
            return false;
        }
        self.viewport = Some(size);
        if self.scene.layout_pending {
            let sizes = self
                .scene
                .nodes
                .iter()
                .map(|(id, node)| (*id, node.size))
                .collect();
            let input = self.scene.index.as_ref().map_or_else(
                evm_abstract_layout::Input::default,
                |index| {
                    layout::prepare(
                        report,
                        index,
                        &self.scene.display,
                        &sizes,
                        &self.scene.edge_labels,
                        painter,
                    )
                },
            );
            match layout::scene(&input) {
                Ok(placement) => {
                    self.scene.placement = placement;
                    self.scene.layout_error = None;
                }
                Err(error) => {
                    self.scene.placement = Placement::default();
                    self.scene.layout_error = Some(error);
                    self.fit_pending = false;
                }
            }
            self.rebuild_visibility();
            self.scene.layout_pending = false;
            if let Some((id, position)) = self.scene.content_anchor.take()
                && let Some(rect) = self.scene.placement.nodes.get(&id)
            {
                self.pan = position - rect.center().to_vec2() * self.zoom;
            }
        }
        if self.fit_pending {
            self.fit(size);
            self.fit_pending = false;
            self.fitted = true;
        }
        true
    }

    pub(crate) fn zoom_at(&mut self, pointer: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 || (factor - 1.0).abs() <= f32::EPSILON {
            return;
        }
        let old = self.zoom;
        // Fit can legitimately make a large graph smaller than 8%. An
        // arbitrary lower clamp would turn its next zoom-out into zoom-in.
        let zoom = (old * factor).min(2.5);
        let ratio = zoom / old;
        if zoom <= 0.0 || !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        let pan = pointer - (pointer - self.pan) * ratio;
        if !pan.is_finite() {
            return;
        }
        self.fitted = false;
        self.zoom = zoom;
        self.pan = pan;
    }

    fn screen_rect(&self, canvas: Rect, world: Rect) -> Rect {
        Rect::from_min_size(
            canvas.min + self.pan + world.min.to_vec2() * self.zoom,
            world.size() * self.zoom,
        )
    }

    fn fit(&mut self, size: Vec2) {
        if self.scene.placement.bounds.is_finite() {
            let bounds = self.scene.placement.bounds;
            self.zoom = layout::fit_scale(bounds.size(), size).min(1.0);
            self.pan = size * 0.5 - bounds.center().to_vec2() * self.zoom;
        }
    }

    fn grid(&self, painter: &Painter, canvas: Rect) {
        let spacing = 24.0 * self.zoom.max(0.5);
        let offset = Vec2::new(
            self.pan.x.rem_euclid(spacing),
            self.pan.y.rem_euclid(spacing),
        );
        let mut x = canvas.left() + offset.x;
        while x < canvas.right() {
            let mut y = canvas.top() + offset.y;
            while y < canvas.bottom() {
                painter.circle_filled(Pos2::new(x, y), 0.6, palette::BORDER.gamma_multiply(0.4));
                y += spacing;
            }
            x += spacing;
        }
    }
}

/// Lightweight deterministic layout entry used by the native identity regression.
#[cfg(test)]
pub(crate) fn layout(report: &ReportView<'_>) -> BTreeMap<usize, Pos2> {
    let sizes = report
        .cfg
        .iter()
        .map(|block| (block.id, Vec2::new(140.0, 70.0)))
        .collect();
    layout::for_report(report, &sizes)
        .nodes
        .into_iter()
        .map(|(id, rect)| (id, rect.min))
        .collect()
}
