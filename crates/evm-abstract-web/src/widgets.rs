//! Custom painter widgets. Each row and graph node has its own hit target;
//! selections contain protocol identities instead of screen coordinates.

mod disassembly;
mod graph;
mod ssa;

pub(crate) use disassembly::disassembly;
pub(crate) use graph::Graph;
pub(crate) use ssa::ssa;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::palette;

pub(crate) const ROW_HEIGHT: f32 = 28.0;

pub(crate) fn heading(ui: &mut Ui, title: &str, detail: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 47.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.text(
        rect.left_top() + Vec2::new(8.0, 5.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(16.0),
        palette::TEXT,
    );
    painter.text(
        rect.left_top() + Vec2::new(8.0, 27.0),
        Align2::LEFT_TOP,
        detail,
        FontId::proportional(11.0),
        palette::MUTED,
    );
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, palette::BORDER),
    );
}

pub(crate) fn empty(ui: &mut Ui, text: &str) {
    let rect = ui.available_rect_before_wrap();
    ui.painter_at(rect).text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        palette::MUTED,
    );
    ui.allocate_rect(rect, Sense::hover());
}

pub(crate) fn row_background(ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
    if selected || hovered {
        ui.painter().rect_filled(
            rect,
            3.0,
            if selected {
                palette::SELECTED
            } else {
                palette::RAISED
            },
        );
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            1.0,
            palette::ACCENT,
        );
    }
}

pub(crate) fn text(ui: &Ui, at: Pos2, value: impl AsRef<str>, color: Color32, size: f32) -> Rect {
    ui.painter().text(
        at,
        Align2::LEFT_CENTER,
        value.as_ref(),
        FontId::monospace(size),
        color,
    )
}

pub(crate) fn badge(ui: &Ui, rect: Rect, value: &str, color: Color32) {
    ui.painter().rect(
        rect,
        4.0,
        palette::PANEL,
        Stroke::new(1.0, color.gamma_multiply(0.4)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        value,
        FontId::monospace(10.0),
        color,
    );
}

#[cfg(test)]
pub(crate) use graph::layout as test_layout;
#[cfg(test)]
pub(crate) use ssa::content_columns as test_content_columns;

/// Center a selected row vertically without moving the independently scrolled
/// horizontal viewport. Centering the full row would hide its leading columns.
pub(crate) fn focus_row(ui: &Ui, rect: Rect) {
    let target = Rect::from_center_size(
        Pos2::new(ui.clip_rect().center().x, rect.center().y),
        Vec2::new(0.0, rect.height()),
    );
    ui.scroll_to_rect(target, Some(egui::Align::Center));
}

pub(crate) fn coverage(
    report: &evm_abstract_protocol::AnalysisReport,
    state: usize,
) -> evm_abstract_protocol::BlockCoverage {
    report
        .ssa
        .blocks
        .iter()
        .find(|block| block.state == state)
        .map_or(evm_abstract_protocol::BlockCoverage::Unexecuted, |block| {
            block.coverage
        })
}
