//! Compact CFG canvas. A report is arranged and fitted once in its first usable
//! viewport. Later resizes leave the scene and camera fixed; Fit only moves the
//! camera. Node bounds come from the chosen source or SSA representation.

mod content;
mod edges;
mod layout;
mod navigation;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage};

use super::heading;
use crate::{app::Selection, palette};
use content::{NodeText, NodeView, node_text};
use edges::{edge_label, paint_edge};
use layout::Placement;

const PAD: f32 = 6.0;
const HEADER_HEIGHT: f32 = 33.0;
const LINE_HEIGHT: f32 = 14.0;
pub(crate) struct Graph {
    pub(crate) zoom: f32,
    content: NodeView,
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
        // The graph fills its assigned pane; an invisible measurement pass
        // must not establish its one-time layout or consume camera input.
        if ui.is_sizing_pass() {
            return;
        }
        let fit_requested = self.fit_shortcut(ui);
        heading(
            ui,
            "CONTROL FLOW",
            &format!("{} states · {} edges", report.cfg.len(), report.edges.len()),
        );
        let mut content = self.content;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut content, NodeView::Disassembly, "Disasm")
                .on_hover_text("Show decoded bytecode in CFG nodes");
            ui.selectable_value(&mut content, NodeView::Ssa, "SSA")
                .on_hover_text("Show stack SSA definitions, arguments and effects in CFG nodes");
            if ui.small_button("Fit graph").on_hover_text("F / Shift+F: fit and center the existing graph without rearranging nodes").clicked() {
                self.request_fit(ui, true);
            }
            if ui.add_enabled(selection.state.is_some(), egui::Button::new("Focus selected").small())
                .on_hover_text("Center the selected node and keep a manual camera").clicked()
            { self.focus_pending = true; }
            ui.label(egui::RichText::new(format!("{:.0}% · {}", self.zoom * 100.0,
                if self.fitted { "Fit" } else { "Manual" })).small().color(palette::MUTED))
                .on_hover_text("Drag or two-finger scroll to pan; pinch or Ctrl/Cmd+scroll to zoom. Double-click to fit the existing graph. Resizing keeps the current layout and camera.");
        });
        self.set_content(content, selection.state);
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(canvas, 3.0, palette::BACKGROUND);
        // A Graph belongs to one immutable report; Workspace resets it when a
        // request starts. The ID comparison also handles a replaced sparse graph.
        if self.nodes.len() != report.cfg.len()
            || report
                .cfg
                .iter()
                .any(|block| !self.nodes.contains_key(&block.id))
        {
            self.nodes = report
                .cfg
                .iter()
                .map(|block| (block.id, node_text(&painter, report, block, self.content)))
                .collect();
            self.layout_pending = true;
        }
        if ui.ctx().will_discard() || !self.update_viewport(report, canvas.size()) {
            return;
        }
        if self.focus_pending {
            if let Some(rect) = selection
                .state
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
        for edge in &report.edges {
            if let Some(route) = self.placement.edges.get(&edge.id) {
                paint_edge(
                    &painter,
                    route,
                    canvas.min + self.pan,
                    edge.kind,
                    edge.id,
                    self.zoom,
                    selection.state == Some(edge.from) || selection.state == Some(edge.to),
                );
            }
        }
        for block in &report.cfg {
            let (Some(world), Some(content)) = (
                self.placement.nodes.get(&block.id),
                self.nodes.get(&block.id),
            ) else {
                continue;
            };
            let rect = self.screen_rect(canvas, *world);
            if !canvas.intersects(rect) {
                continue;
            }
            let response = ui.interact(
                rect.intersect(canvas),
                ui.id().with(("cfg_node", block.id)),
                Sense::click(),
            );
            paint_node(
                &painter,
                rect,
                content,
                selection.state == Some(block.id),
                response.hovered(),
                self.zoom,
            );
            if response.clicked() {
                *selection = Selection {
                    state: Some(block.id),
                    pc: None,
                };
            }
            let exit = match content.coverage {
                BlockCoverage::Current => {
                    format!("Observed exit stack: {}", block.exit_stack.join(", "))
                }
                BlockCoverage::Stale => format!(
                    "Historical exit stack (entry changed): {}",
                    block.exit_stack.join(", ")
                ),
                BlockCoverage::Unexecuted => {
                    "No execution receipt; only entry state is available".into()
                }
            };
            response.on_hover_text(format!("State S{} · basic block B{}\nFrame depth: {} · code: {}\nContext: {:?} · coverage: {:?}\nCurrent entry stack (bottom → top): {}\n{}\n\n{}",
                block.id, block.basic_block, block.frame_depth, block.code_address, block.context, content.coverage, block.entry_stack.join(", "), exit, content.tooltip));
        }
        if report.cfg.is_empty() {
            painter.text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "No reachable states",
                FontId::proportional(13.0),
                palette::MUTED,
            );
        }
    }

    /// New reports invalidate scene/camera state, but the chosen representation
    /// is a workspace preference and survives analysis requests.
    pub(crate) fn reset_report(&mut self) {
        *self = Self {
            content: self.content,
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

    fn set_content(&mut self, content: NodeView, selected: Option<usize>) {
        if content == self.content {
            return;
        }
        self.content_anchor = self.viewport.and_then(|viewport| {
            let screen = Rect::from_min_size(Pos2::ZERO, viewport);
            let selected = selected
                .and_then(|id| self.placement.nodes.get(&id).map(|rect| (id, rect)))
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

    fn update_viewport(&mut self, report: &AnalysisReport, size: Vec2) -> bool {
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
            self.placement = layout::adaptive(report, &sizes, size);
            self.layout_pending = false;
            if let Some((id, position)) = self.content_anchor.take()
                && let Some(rect) = self.placement.nodes.get(&id)
            {
                // A representation switch changes node measurements but keeps
                // the anchor at the same position relative to the canvas.
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
        painter.text(
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
