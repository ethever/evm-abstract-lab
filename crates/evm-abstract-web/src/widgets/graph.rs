//! Compact CFG canvas. Automatic layout follows the viewport until an explicit
//! camera gesture; Fit restores it. Node bounds come from the painted content.

mod layout;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, CfgBlock, EdgeKind};

use super::heading;
use crate::{app::Selection, palette};
use layout::{EdgeRoute, Placement};

const PAD: f32 = 6.0;
const HEADER_HEIGHT: f32 = 33.0;
const LINE_HEIGHT: f32 = 14.0;
const PREVIEW_ROWS: usize = 4;

struct NodeText {
    title: String,
    detail: String,
    lines: Vec<(String, bool)>,
    coverage: BlockCoverage,
    frontier: bool,
    size: Vec2,
}

pub(crate) struct Graph {
    pub(crate) zoom: f32,
    pan: Vec2,
    nodes: BTreeMap<usize, NodeText>,
    placement: Placement,
    automatic: bool,
    layout_pending: bool,
    focus_pending: bool,
    viewport: Option<Vec2>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            nodes: BTreeMap::new(),
            placement: Placement::default(),
            automatic: true,
            layout_pending: true,
            focus_pending: false,
            viewport: None,
        }
    }
}

impl Graph {
    pub(crate) fn show(&mut self, ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
        heading(
            ui,
            "CONTROL FLOW",
            &format!("{} states · {} edges", report.cfg.len(), report.edges.len()),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Fit graph").on_hover_text("Fit all nodes and edges; follow future viewport changes").clicked() {
                self.restore_automatic();
            }
            if ui.add_enabled(selection.state.is_some(), egui::Button::new("Focus selected").small())
                .on_hover_text("Center the selected node and keep a manual camera").clicked()
            { self.focus_pending = true; }
            ui.label(egui::RichText::new(format!("{:.0}% · {}", self.zoom * 100.0,
                if self.automatic { "Auto" } else { "Manual" })).small().color(palette::MUTED))
                .on_hover_text("Drag to pan; scroll or pinch to zoom. Double-click to restore automatic fit. Edge labels show their control-flow kind.");
        });
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
                .map(|block| (block.id, node_text(&painter, report, block)))
                .collect();
            self.layout_pending = true;
        }
        self.update_viewport(report, canvas.size());
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
        if response.dragged() {
            self.automatic = false;
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let (scroll, pinch, pointer) = ui.input(|input| {
                (
                    input.smooth_scroll_delta().y,
                    input.zoom_delta(),
                    input.pointer.hover_pos(),
                )
            });
            if let Some(pointer) = pointer {
                let factor = if (pinch - 1.0).abs() > f32::EPSILON {
                    pinch
                } else {
                    (scroll * 0.002).exp()
                };
                self.zoom_at(pointer - canvas.min, factor);
            }
        }
        if response.double_clicked() {
            self.restore_automatic();
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
            response.on_hover_text(format!("State S{} · basic block B{}\nFrame depth: {} · code: {}\nContext: {:?} · coverage: {:?}\nCurrent entry stack (bottom → top): {}\n{}",
                block.id, block.basic_block, block.frame_depth, block.code_address, block.context, content.coverage, block.entry_stack.join(", "), exit));
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
            // Saved node space should not become a blank band before the entry.
            self.pan = Vec2::new(
                (size.x - bounds.width() * self.zoom) * 0.5,
                layout::FIT_MARGIN,
            ) - bounds.min.to_vec2() * self.zoom;
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

fn node_text(painter: &Painter, report: &AnalysisReport, block: &CfgBlock) -> NodeText {
    let coverage = super::coverage(report, block.id);
    let frontier = report
        .frontiers
        .iter()
        .any(|frontier| frontier.from == Some(block.id));
    let title = format!(
        "S{}  ·  B{}{}",
        block.id,
        block.basic_block,
        if coverage == BlockCoverage::Current {
            String::new()
        } else {
            format!("  {coverage:?}")
        }
    );
    let detail = format!(
        "{} · frame {}{}",
        block
            .start_pc
            .map_or_else(|| "no bytecode".into(), |pc| format!("0x{pc:04x}")),
        block.frame_depth,
        if frontier { " · !" } else { "" }
    );
    let mut lines: Vec<_> = block
        .instructions
        .iter()
        .take(PREVIEW_ROWS)
        .map(|instruction| {
            let operand = instruction
                .immediate
                .as_ref()
                .map_or(String::new(), |value| {
                    if value.len() > 15 {
                        format!(" {}…", &value[..14])
                    } else {
                        format!(" {value}")
                    }
                });
            (
                format!("{:04x}  {}{operand}", instruction.pc, instruction.name),
                coverage == BlockCoverage::Current && block.executed_pcs.contains(&instruction.pc),
            )
        })
        .collect();
    if block.instructions.len() > PREVIEW_ROWS {
        lines.push((
            format!("+{} instructions", block.instructions.len() - PREVIEW_ROWS),
            false,
        ));
    }
    let mut width =
        measured_width(painter, &title, 12.0).max(measured_width(painter, &detail, 10.0));
    for (line, _) in &lines {
        width = width.max(measured_width(painter, line, 11.0));
    }
    let size = Vec2::new(
        (width + PAD * 2.0).max(104.0),
        HEADER_HEIGHT + lines.len() as f32 * LINE_HEIGHT + PAD,
    );
    NodeText {
        title,
        detail,
        lines,
        coverage,
        frontier,
        size,
    }
}

fn measured_width(painter: &Painter, text: &str, size: f32) -> f32 {
    painter
        .layout_no_wrap(text.into(), FontId::monospace(size), palette::TEXT)
        .size()
        .x
        .ceil()
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
    for (index, (line, executed)) in content.lines.iter().enumerate() {
        label(
            HEADER_HEIGHT + index as f32 * LINE_HEIGHT,
            line,
            if *executed {
                palette::TEXT
            } else {
                palette::MUTED
            },
            11.0,
        );
    }
}

fn edge_label(id: usize, kind: EdgeKind) -> String {
    format!(
        "e{id} {}",
        match kind {
            EdgeKind::BranchTrue => "true",
            EdgeKind::BranchFalse => "false",
            EdgeKind::Jump => "jump",
            EdgeKind::Fallthrough => "next",
            EdgeKind::Call => "call",
            EdgeKind::Return => "return",
            EdgeKind::Failure => "failure",
            EdgeKind::Revert => "revert",
        }
    )
}

fn paint_edge(
    painter: &Painter,
    route: &EdgeRoute,
    origin: Pos2,
    kind: EdgeKind,
    id: usize,
    zoom: f32,
    selected: bool,
) {
    let color = match kind {
        EdgeKind::BranchTrue | EdgeKind::Jump => palette::ACCENT,
        EdgeKind::BranchFalse => palette::WARNING,
        EdgeKind::Call | EdgeKind::Return => palette::PURPLE,
        EdgeKind::Failure | EdgeKind::Revert => palette::ERROR,
        EdgeKind::Fallthrough => palette::BLUE,
    };
    let points: Vec<_> = route
        .points
        .iter()
        .map(|point| origin + point.to_vec2() * zoom)
        .collect();
    let stroke = Stroke::new(
        if selected { 1.6 } else { 1.0 },
        if selected {
            color
        } else {
            color.gamma_multiply(0.65)
        },
    );
    painter.add(Shape::line(points.clone(), stroke));
    let end = points[points.len() - 1];
    let direction = (end - points[points.len() - 2]).normalized();
    let perpendicular = Vec2::new(-direction.y, direction.x);
    let size = (6.0 * zoom).max(2.0);
    painter.add(Shape::convex_polygon(
        vec![
            end,
            end - direction * size + perpendicular * size * 0.5,
            end - direction * size - perpendicular * size * 0.5,
        ],
        stroke.color,
        Stroke::NONE,
    ));
    if zoom > 0.35 {
        let position = origin + route.label.to_vec2() * zoom;
        let galley =
            painter.layout_no_wrap(edge_label(id, kind), FontId::monospace(10.0 * zoom), color);
        painter.rect_filled(
            Rect::from_min_size(position, galley.size()).expand(1.0),
            1.0,
            palette::BACKGROUND,
        );
        painter.galley(position, galley, color);
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
