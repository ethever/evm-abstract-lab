//! Virtual table rows and graph nodes have their own hit targets;
//! selections contain protocol identities instead of screen coordinates.

mod disassembly;
mod graph;
mod ssa;
#[cfg(test)]
mod tests;

pub(crate) use disassembly::disassembly;
pub(crate) use graph::Graph;
pub(crate) use ssa::ssa;

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Ui, Vec2};

use crate::palette;

pub(crate) const ROW_HEIGHT: f32 = 22.0;

pub(crate) fn heading(ui: &mut Ui, title: &str, detail: &str) {
    let response = ui
        .allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(title)
                        .size(11.5)
                        .strong()
                        .color(palette::TEXT),
                )
                .on_hover_text(if title == "SSA" {
                    "Static single assignment"
                } else {
                    title
                });
                ui.add(
                    egui::Label::new(RichText::new(detail).size(10.5).color(palette::MUTED))
                        .truncate(),
                )
                .on_hover_text(detail);
            },
        )
        .response;
    let rect = response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, palette::BORDER),
    );
}

pub(crate) fn empty(ui: &mut Ui, text: &str) {
    ui.centered_and_justified(|ui| {
        ui.add(egui::Label::new(RichText::new(text).size(14.0).color(palette::MUTED)).wrap());
    });
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

#[cfg(test)]
pub(crate) use graph::layout as test_layout;
#[cfg(test)]
pub(crate) use ssa::content_columns as test_content_columns;

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
