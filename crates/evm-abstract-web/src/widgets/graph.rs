//! CFG views over an immutable native report. Source blocks fold only for
//! display; selecting an instance always uses its original state and SSA IDs.

mod content;
mod controls;
mod edges;
mod index;
mod layout;
mod navigation;
mod projection;
mod scene;
#[cfg(test)]
mod tests;
mod visibility;

use super::heading;
use crate::{app::Selection, palette};
#[cfg(test)]
use content::node_text;
use content::{NodeText, NodeView, node_preview, node_tooltip};
use edges::{edge_label, paint_edge_text};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage};
use index::ReportIndex;
use layout::Placement;
use projection::DisplayGraph;
pub(crate) use projection::GraphMode;
use scene::ReportStamp;
use std::collections::{BTreeMap, BTreeSet};
use visibility::Visibility;

const PAD: f32 = 6.0;
const HEADER_HEIGHT: f32 = 33.0;
const LINE_HEIGHT: f32 = 14.0;
pub(crate) struct Graph {
    pub(crate) zoom: f32,
    content: NodeView,
    mode: GraphMode,
    program_scope: Option<usize>,
    neighborhood_hops: usize,
    local_center: Option<usize>,
    index: Option<ReportIndex>,
    report_stamp: Option<ReportStamp>,
    display: DisplayGraph,
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
    pan: Vec2,
    nodes: BTreeMap<usize, NodeText>,
    placement: Placement,
    fitted: bool,
    layout_pending: bool,
    fit_pending: bool,
    focus_pending: bool,
    cancel_wheel: bool,
    viewport: Option<Vec2>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            content: NodeView::default(),
            mode: GraphMode::default(),
            program_scope: None,
            neighborhood_hops: 2,
            local_center: None,
            index: None,
            report_stamp: None,
            display: DisplayGraph::default(),
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
            pan: Vec2::ZERO,
            nodes: BTreeMap::new(),
            placement: Placement::default(),
            fitted: true,
            layout_pending: true,
            fit_pending: true,
            focus_pending: false,
            cancel_wheel: false,
            viewport: None,
        }
    }
}

impl Graph {
    pub(crate) fn show(&mut self, ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
        if ui.is_sizing_pass() {
            return;
        }
        self.ensure_scene(report, selection.state);
        let fit_requested = self.fit_shortcut(ui);
        heading(
            ui,
            "CONTROL FLOW",
            &format!(
                "{} nodes · {} / {} states · {} edges",
                self.display.nodes.len(),
                self.shown_states,
                report.cfg.len(),
                self.display.edges.len()
            ),
        );
        self.controls(ui, report, selection);
        self.ensure_scene(report, selection.state);
        self.view_legend(ui);
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(canvas, 3.0, palette::BACKGROUND);
        if self.nodes.is_empty()
            && let Some(index) = &self.index
        {
            self.nodes = self
                .display
                .nodes
                .iter()
                .map(|node| {
                    (
                        node.id,
                        node_preview(&painter, report, index, node, self.content),
                    )
                })
                .collect();
        }
        if ui.ctx().will_discard() || !self.update_viewport(report, canvas.size()) {
            return;
        }
        let selected_node = selection
            .state
            .and_then(|state| self.display.state_to_node.get(&state).copied());
        if self.focus_pending {
            if let Some(rect) = selected_node
                .and_then(|id| self.placement.nodes.get(&id))
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
        self.update_highlight(report, selection.state);
        let visible = Rect::from_min_max(
            Pos2::ZERO - self.pan / self.zoom,
            Pos2::ZERO + (canvas.size() - self.pan) / self.zoom,
        )
        .expand(16.0 / self.zoom);
        for id in self.edge_visibility.query(visible) {
            let Some(route) = self.placement.edges.get(&id) else {
                continue;
            };
            let Some(edge) = self
                .edge_positions
                .get(&id)
                .and_then(|position| self.display.edges.get(*position))
            else {
                continue;
            };
            let label = self.edge_labels.get(&id).map_or("", String::as_str);
            paint_edge_text(
                &painter,
                route,
                canvas.min + self.pan,
                edge.kind,
                label,
                self.zoom,
                self.highlighted_edges.contains(&id),
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
        for id in self.node_visibility.query(visible) {
            let (Some(world), Some(content)) = (self.placement.nodes.get(&id), self.nodes.get(&id))
            else {
                continue;
            };
            let Some(node) = self
                .node_positions
                .get(&id)
                .and_then(|position| self.display.nodes.get(*position))
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
            paint_node(
                &painter,
                rect,
                content,
                selected_node == Some(id),
                hit.hovered(),
                self.zoom,
            );
            if hit.clicked() {
                let state = selection
                    .state
                    .filter(|state| self.display.state_to_node.get(state) == Some(&id))
                    .or_else(|| node.members.first().copied());
                *selection = Selection { state, pc: None };
                if node.members.len() > 1 {
                    self.instance_group = Some(id);
                    self.instance_filter.clear();
                }
                ui.ctx().request_repaint();
            }
            hit.on_hover_ui(|ui| {
                if let Some(index) = &self.index {
                    ui.label(crate::notation::widget(
                        ui,
                        egui::RichText::new(node_tooltip(report, index, node, self.content))
                            .monospace(),
                    ));
                }
            });
        }
        if self.display.nodes.is_empty() {
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

    /// New reports invalidate all report-bound mappings; view/content choices
    /// remain preferences, while a program filter belongs to the old report.
    pub(crate) fn reset_report(&mut self) {
        *self = Self {
            content: self.content,
            mode: self.mode,
            neighborhood_hops: self.neighborhood_hops,
            cancel_wheel: self.cancel_wheel,
            ..Self::default()
        };
    }

    pub(crate) fn content_label(&self) -> &'static str {
        match self.content {
            NodeView::Disassembly => "Disasm",
            NodeView::Ssa => "SSA",
        }
    }

    pub(crate) fn set_mode(&mut self, mode: GraphMode) {
        if self.mode != mode {
            self.mode = mode;
            self.invalidate_scene();
        }
    }

    fn invalidate_scene(&mut self) {
        self.display_dirty = true;
        self.nodes.clear();
        self.content_anchor = None;
        self.instance_group = None;
        self.layout_pending = true;
        self.fit_pending = true;
        self.fitted = true;
    }

    fn set_content(&mut self, content: NodeView, selected: Option<usize>) {
        if content == self.content {
            return;
        }
        self.content_anchor = self.viewport.and_then(|viewport| {
            let screen = Rect::from_min_size(Pos2::ZERO, viewport);
            let selected = selected
                .and_then(|state| self.display.state_to_node.get(&state))
                .and_then(|id| self.placement.nodes.get(id).map(|rect| (*id, rect)))
                .filter(|(_, rect)| {
                    screen.contains(Pos2::ZERO + self.pan + rect.center().to_vec2() * self.zoom)
                });
            let center = (viewport * 0.5 - self.pan) / self.zoom;
            selected
                .or_else(|| {
                    self.placement
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
        self.nodes.clear();
        self.layout_pending = true;
        self.fitted = false;
    }

    fn queue_fit(&mut self) {
        self.fitted = true;
        self.fit_pending = true;
        self.focus_pending = false;
    }

    fn update_viewport(&mut self, _report: &AnalysisReport, size: Vec2) -> bool {
        if !size.is_finite() || size.min_elem() <= 1.0 {
            return false;
        }
        self.viewport = Some(size);
        if self.layout_pending {
            let sizes = self
                .nodes
                .iter()
                .map(|(id, node)| (*id, node.size))
                .collect();
            let ids: Vec<_> = self.display.nodes.iter().map(|node| node.id).collect();
            self.placement =
                layout::adaptive_scene(&ids, &self.display.edges, &sizes, &self.edge_labels, size);
            self.rebuild_visibility();
            self.layout_pending = false;
            if let Some((id, position)) = self.content_anchor.take()
                && let Some(rect) = self.placement.nodes.get(&id)
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
        self.fitted = false;
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.08, 2.5);
        self.pan = pointer - (pointer - self.pan) * (self.zoom / old);
    }

    fn screen_rect(&self, canvas: Rect, world: Rect) -> Rect {
        Rect::from_min_size(
            canvas.min + self.pan + world.min.to_vec2() * self.zoom,
            world.size() * self.zoom,
        )
    }

    fn fit(&mut self, size: Vec2) {
        if self.placement.bounds.is_finite() {
            let bounds = self.placement.bounds;
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

fn paint_node(
    painter: &Painter,
    rect: Rect,
    content: &NodeText,
    selected: bool,
    hovered: bool,
    zoom: f32,
) {
    let border = if selected {
        palette::ACCENT
    } else if content.frontier || content.coverage != BlockCoverage::Current {
        palette::WARNING
    } else if hovered {
        palette::BLUE
    } else {
        palette::BORDER
    };
    painter.rect(
        rect,
        4.0,
        if selected {
            palette::SELECTED
        } else {
            palette::PANEL
        },
        Stroke::new(if selected { 1.5 } else { 1.0 }, border),
        StrokeKind::Inside,
    );
    if zoom < 0.22 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let label = |y: f32, text: &str, color: Color32, size: f32| {
        crate::notation::paint(
            &painter,
            rect.min + Vec2::new(PAD, y) * zoom,
            Align2::LEFT_TOP,
            text,
            FontId::monospace(size * zoom),
            color,
        );
    };
    label(
        4.0,
        &content.title,
        if content.coverage == BlockCoverage::Current {
            palette::ACCENT
        } else {
            palette::WARNING
        },
        12.0,
    );
    label(18.0, &content.detail, palette::MUTED, 10.0);
    painter.hline(
        rect.x_range(),
        rect.top() + (HEADER_HEIGHT - 3.0) * zoom,
        Stroke::new(1.0, palette::BORDER),
    );
    for (index, (line, color)) in content.lines.iter().enumerate() {
        label(
            HEADER_HEIGHT + index as f32 * LINE_HEIGHT,
            line,
            *color,
            11.0,
        );
    }
}

/// Lightweight deterministic layout entry used by the native identity regression.
#[cfg(test)]
pub(crate) fn layout(report: &AnalysisReport) -> BTreeMap<usize, Pos2> {
    let sizes = report
        .cfg
        .iter()
        .map(|block| (block.id, Vec2::new(140.0, 70.0)))
        .collect();
    layout::adaptive(report, &sizes, Vec2::new(800.0, 600.0))
        .nodes
        .into_iter()
        .map(|(id, rect)| (id, rect.min))
        .collect()
}
