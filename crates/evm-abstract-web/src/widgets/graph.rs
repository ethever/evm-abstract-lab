//! Pan/zoom graph canvas with native state identities, typed edge colors and
//! independently clickable nodes. Layout is deterministic and handles cycles.

use std::collections::{BTreeMap, VecDeque};

use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, CfgBlock, EdgeKind};

use super::heading;
use crate::{app::Selection, palette};

const NODE_SIZE: Vec2 = Vec2::new(252.0, 148.0);
const SPACING: Vec2 = Vec2::new(296.0, 216.0);

pub(crate) struct Graph {
    pub(crate) zoom: f32,
    pan: Vec2,
    positions: BTreeMap<usize, Pos2>,
    fit_pending: bool,
    focus_pending: bool,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            positions: BTreeMap::new(),
            fit_pending: true,
            focus_pending: false,
        }
    }
}

impl Graph {
    pub(crate) fn show(&mut self, ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
        heading(
            ui,
            "CONTROL FLOW",
            &format!(
                "{} states · {} edges · drag to pan, scroll to zoom",
                report.cfg.len(),
                report.edges.len()
            ),
        );
        ui.horizontal(|ui| {
            if ui.button("Fit graph").clicked() {
                self.fit_pending = true;
            }
            if ui.button("Focus selected").clicked() {
                self.focus_pending = true;
            }
            ui.label(
                egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                    .small()
                    .color(palette::MUTED),
            );
        });
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(canvas, 4.0, palette::BACKGROUND);
        if self.positions.len() != report.cfg.len() {
            self.positions = layout(report);
        }
        if self.fit_pending {
            self.fit(canvas);
            self.fit_pending = false;
        }
        if self.focus_pending {
            if let Some(origin) = selection.state.and_then(|id| self.positions.get(&id)) {
                self.zoom = 1.0;
                self.pan = canvas.size() * 0.5 - origin.to_vec2() - NODE_SIZE * 0.5;
            }
            self.focus_pending = false;
        }
        if response.dragged() {
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
            self.fit_pending = true;
        }
        self.grid(&painter, canvas);
        for edge in &report.edges {
            let (Some(from), Some(to)) =
                (self.positions.get(&edge.from), self.positions.get(&edge.to))
            else {
                continue;
            };
            let from = self.node_rect(canvas, *from);
            let to = self.node_rect(canvas, *to);
            paint_edge(
                &painter,
                from,
                to,
                edge.kind,
                edge.id,
                self.zoom,
                selection.state == Some(edge.from) || selection.state == Some(edge.to),
            );
        }
        for block in &report.cfg {
            let Some(origin) = self.positions.get(&block.id) else {
                continue;
            };
            let rect = self.node_rect(canvas, *origin);
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
                block,
                report,
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
            let coverage = super::coverage(report, block.id);
            let exit = match coverage {
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
                block.id, block.basic_block, block.frame_depth, block.code_address, block.context, coverage,
                block.entry_stack.join(", "), exit));
        }
        if report.cfg.is_empty() {
            painter.text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "No reachable states",
                FontId::proportional(15.0),
                palette::MUTED,
            );
        }
        let legend = Rect::from_min_size(
            canvas.left_bottom() + Vec2::new(10.0, -28.0),
            Vec2::new(280.0, 22.0),
        );
        painter.rect_filled(legend, 4.0, palette::PANEL);
        painter.text(
            legend.center(),
            Align2::CENTER_CENTER,
            "true / jump     false     call / return",
            FontId::monospace(10.0),
            palette::MUTED,
        );
    }

    pub(crate) fn zoom_at(&mut self, pointer: Vec2, factor: f32) {
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.08, 2.5);
        self.pan = pointer - (pointer - self.pan) * (self.zoom / old);
    }

    fn node_rect(&self, canvas: Rect, origin: Pos2) -> Rect {
        Rect::from_min_size(
            canvas.min + self.pan + origin.to_vec2() * self.zoom,
            NODE_SIZE * self.zoom,
        )
    }

    fn fit(&mut self, canvas: Rect) {
        let bounds = self
            .positions
            .values()
            .fold(Rect::NOTHING, |bounds, origin| {
                bounds.union(Rect::from_min_size(*origin, NODE_SIZE))
            });
        if bounds.is_finite() {
            self.zoom = ((canvas.width() - 40.0) / bounds.width())
                .min((canvas.height() - 55.0) / bounds.height())
                .clamp(0.08, 1.0);
            self.pan = (canvas.size() - bounds.size() * self.zoom) * 0.5
                - bounds.min.to_vec2() * self.zoom;
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
                painter.circle_filled(Pos2::new(x, y), 0.7, palette::BORDER.gamma_multiply(0.55));
                y += spacing;
            }
            x += spacing;
        }
    }
}

pub(crate) fn layout(report: &AnalysisReport) -> BTreeMap<usize, Pos2> {
    let mut levels = BTreeMap::new();
    let mut queue = VecDeque::new();
    if let Some(root) = report.cfg.first() {
        levels.insert(root.id, 0usize);
        queue.push_back(root.id);
    }
    let mut outgoing = BTreeMap::<usize, Vec<usize>>::new();
    for edge in &report.edges {
        outgoing.entry(edge.from).or_default().push(edge.to);
    }
    while let Some(id) = queue.pop_front() {
        let level = levels[&id] + 1;
        for target in outgoing.get(&id).into_iter().flatten() {
            if !levels.contains_key(target) {
                levels.insert(*target, level);
                queue.push_back(*target);
            }
        }
    }
    let mut rows = BTreeMap::<usize, Vec<usize>>::new();
    let last_level = levels.values().copied().max().unwrap_or(0) + 1;
    for block in &report.cfg {
        rows.entry(levels.get(&block.id).copied().unwrap_or(last_level))
            .or_default()
            .push(block.id);
    }
    let mut result = BTreeMap::new();
    let mut row_index = 0usize;
    for ids in rows.values() {
        // Wrap broad branches into rows so 4096-state snapshots remain navigable.
        for chunk in ids.chunks(6) {
            let width = chunk.len() as f32 * SPACING.x;
            for (column, id) in chunk.iter().enumerate() {
                result.insert(
                    *id,
                    Pos2::new(
                        column as f32 * SPACING.x - width * 0.5,
                        row_index as f32 * SPACING.y,
                    ),
                );
            }
            row_index += 1;
        }
    }
    result
}

fn paint_node(
    painter: &Painter,
    rect: Rect,
    block: &CfgBlock,
    report: &AnalysisReport,
    selected: bool,
    hovered: bool,
    zoom: f32,
) {
    let coverage = super::coverage(report, block.id);
    let frontier = report
        .frontiers
        .iter()
        .any(|frontier| frontier.from == Some(block.id));
    let border = if selected {
        palette::ACCENT
    } else if frontier || coverage != BlockCoverage::Current {
        palette::WARNING
    } else if hovered {
        palette::BLUE
    } else {
        palette::BORDER
    };
    painter.rect(
        rect,
        6.0,
        if selected {
            palette::SELECTED
        } else {
            palette::PANEL
        },
        Stroke::new(if selected { 2.0 } else { 1.0 }, border),
        StrokeKind::Inside,
    );
    if zoom < 0.28 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let origin = rect.min;
    let label = |x: f32, y: f32, value: String, color: Color32, size: f32| {
        painter.text(
            origin + Vec2::new(x, y) * zoom,
            Align2::LEFT_TOP,
            value,
            FontId::monospace(size * zoom),
            color,
        );
    };
    label(
        12.0,
        10.0,
        format!("S{}  ·  B{}", block.id, block.basic_block),
        palette::ACCENT,
        13.0,
    );
    label(
        12.0,
        30.0,
        format!(
            "pc {}  ·  frame {}{}",
            block
                .start_pc
                .map_or_else(|| "—".into(), |pc| format!("0x{pc:04x}")),
            block.frame_depth,
            if frontier { "  !" } else { "" }
        ),
        palette::MUTED,
        10.0,
    );
    if coverage != BlockCoverage::Current {
        label(165.0, 12.0, format!("{coverage:?}"), palette::WARNING, 9.0);
    }
    painter.hline(
        rect.x_range(),
        rect.top() + 50.0 * zoom,
        Stroke::new(1.0, palette::BORDER),
    );
    for (index, instruction) in block.instructions.iter().take(4).enumerate() {
        label(
            12.0,
            59.0 + index as f32 * 18.0,
            format!(
                "{:04x}  {}{}",
                instruction.pc,
                instruction.name,
                instruction
                    .immediate
                    .as_ref()
                    .map_or(String::new(), |value| {
                        if value.len() > 15 {
                            format!(" {}…", &value[..14])
                        } else {
                            format!(" {value}")
                        }
                    })
            ),
            if coverage == BlockCoverage::Current && block.executed_pcs.contains(&instruction.pc) {
                palette::TEXT
            } else {
                palette::MUTED
            },
            11.0,
        );
    }
    if block.instructions.len() > 4 {
        label(
            12.0,
            131.0,
            format!("+{} instructions", block.instructions.len() - 4),
            palette::MUTED,
            9.0,
        );
    }
}

fn paint_edge(
    painter: &Painter,
    from: Rect,
    to: Rect,
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
    let lane = (12.0 + (id % 5) as f32 * 6.0) * zoom;
    let points = if to.top() > from.bottom() {
        let port = match kind {
            EdgeKind::BranchTrue => 0.3,
            EdgeKind::BranchFalse => 0.7,
            _ => 0.5,
        };
        let start = Pos2::new(from.left() + from.width() * port, from.bottom());
        let end = to.center_top();
        let middle = (start.y + end.y) * 0.5;
        vec![
            start,
            Pos2::new(start.x, middle),
            Pos2::new(end.x, middle),
            end,
        ]
    } else {
        let start = from.right_center();
        let end = to.right_center();
        let side = from.right().max(to.right()) + lane;
        vec![
            start,
            Pos2::new(side, start.y),
            Pos2::new(side, end.y - 12.0 * zoom),
            Pos2::new(end.x + 10.0 * zoom, end.y - 12.0 * zoom),
            end,
        ]
    };
    let stroke = Stroke::new(
        if selected { 1.8 } else { 1.0 },
        if selected {
            color
        } else {
            color.gamma_multiply(0.6)
        },
    );
    painter.add(Shape::line(points.clone(), stroke));
    let end = points[points.len() - 1];
    let direction = (end - points[points.len() - 2]).normalized();
    let perpendicular = Vec2::new(-direction.y, direction.x);
    let size = (7.0 * zoom).max(3.0);
    painter.add(Shape::convex_polygon(
        vec![
            end,
            end - direction * size + perpendicular * size * 0.5,
            end - direction * size - perpendicular * size * 0.5,
        ],
        stroke.color,
        Stroke::NONE,
    ));
    if zoom > 0.4 {
        let label_position = points[1] + Vec2::new(4.0, -10.0 * zoom);
        let label = format!(
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
        );
        let galley = painter.layout_no_wrap(label, FontId::monospace(10.0 * zoom), color);
        painter.rect_filled(
            Rect::from_min_size(label_position, galley.size()).expand(2.0),
            2.0,
            palette::BACKGROUND,
        );
        painter.galley(label_position, galley, color);
    }
}
