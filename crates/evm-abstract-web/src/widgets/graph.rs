//! Compact CFG canvas. Automatic layout follows the viewport until an explicit
//! camera gesture; Fit restores it. Node bounds come from the painted content.

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
    automatic: bool,
    layout_pending: bool,
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
            automatic: true,
            layout_pending: true,
            focus_pending: false,
            cancel_wheel: false,
            viewport: None,
        }
    }
}

impl Graph {
    pub(crate) fn show(&mut self, ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
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
            if ui.small_button("Fit graph").on_hover_text("F / Shift+F: fit and center all nodes and edges; follow future viewport changes").clicked() {
                self.request_fit(ui, true);
            }
            if ui.add_enabled(selection.state.is_some(), egui::Button::new("Focus selected").small())
                .on_hover_text("Center the selected node and keep a manual camera").clicked()
            { self.focus_pending = true; }
            ui.label(egui::RichText::new(format!("{:.0}% · {}", self.zoom * 100.0,
                if self.automatic { "Auto" } else { "Manual" })).small().color(palette::MUTED))
                .on_hover_text("Drag or two-finger scroll to pan; pinch or Ctrl/Cmd+scroll to zoom. Double-click to restore automatic fit. Edge labels show their control-flow kind.");
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
        self.update_viewport(report, canvas.size());
        if let Some((id, offset)) = self.content_anchor.take()
            && let Some(rect) = self.placement.nodes.get(&id)
        {
            self.pan = canvas.size() * 0.5 + offset - rect.center().to_vec2() * self.zoom;
        }
        if self.focus_pending {
            if let Some(rect) = selection
                .state
                .and_then(|id| self.placement.nodes.get(&id))
                .copied()
            {
                self.automatic = false;
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
        self.content_anchor = if self.automatic {
            None
        } else {
            self.viewport.and_then(|viewport| {
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
                    .map(|(id, rect)| {
                        (
                            id,
                            self.pan + rect.center().to_vec2() * self.zoom - viewport * 0.5,
                        )
                    })
            })
        };
        self.content = content;
        self.nodes.clear();
        self.layout_pending = true;
    }

    fn restore_automatic(&mut self) {
        self.automatic = true;
        self.layout_pending = true;
    }

    fn update_viewport(&mut self, report: &AnalysisReport, size: Vec2) {
        let resized = self
            .viewport
            .is_none_or(|previous| (previous - size).length_sq() > 0.25);
        if self.layout_pending || (self.automatic && resized) {
            let sizes = self
                .nodes
                .iter()
                .map(|(id, node)| (*id, node.size))
                .collect();
            self.placement = layout::adaptive(report, &sizes, size);
            self.layout_pending = false;
            if self.automatic {
                self.fit(size);
            }
        } else if resized && let Some(previous) = self.viewport {
            // Keep the same world point under the viewport center in manual mode.
            self.pan += (size - previous) * 0.5;
        }
        self.viewport = Some(size);
    }

    pub(crate) fn zoom_at(&mut self, pointer: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 || (factor - 1.0).abs() <= f32::EPSILON {
            return;
        }
        self.automatic = false;
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
