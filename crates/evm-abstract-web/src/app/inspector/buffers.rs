//! Sparse byte observations keep their length and unspecified-byte defaults.
use crate::report::ReportView;

use super::value;
use crate::palette;
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use evm_abstract_protocol::ValueInfo;

pub(super) fn show(ui: &mut Ui, report: &ReportView<'_>, id: usize, title: &str) {
    let Some(bytes) = report.byte_arrays.get(id) else {
        ui.colored_label(
            palette::ERROR,
            "The report references an unavailable byte-array snapshot",
        );
        return;
    };
    egui::ScrollArea::both()
        .id_salt(("byte_cells", title, id))
        .min_scrolled_height(0.0)
        .content_margin(egui::Margin::ZERO)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new(("byte_array_header", title))
                .num_columns(2)
                .show(ui, |ui| {
                    value::named(ui, "Length", &bytes.length);
                    value::named(ui, "Base byte default", &bytes.default);
                });
            ui.label(
                RichText::new(format!(
                    "{} explicit byte cells{}",
                    bytes.cells.len(),
                    if bytes.memory {
                        " · word-rounded EVM memory"
                    } else {
                        ""
                    }
                ))
                .small()
                .color(palette::MUTED),
            );
            super::symbolic::show(
                ui,
                report,
                bytes.symbolic.node,
                "Sources, writes and copy ranges",
            );
            super::symbolic::facts(ui, &bytes.symbolic_facts, "State-local scalar facts");
            if bytes.cells.is_empty() {
                ui.label(
            "No materialized cells. Read the symbolic view together with length and numeric facts.",
        );
                return;
            }
            TableBuilder::new(ui)
                .id_salt(("byte_values", title, id))
                .vscroll(false)
                .column(Column::auto().at_least(96.0))
                .column(Column::remainder().at_least(100.0))
                .body(|body| {
                    body.rows(21.0, bytes.cells.len(), |mut row| {
                        let cell = &bytes.cells[row.index()];
                        row.col(|ui| {
                            ui.monospace(format!("0x{:08x}", cell.offset));
                        });
                        row.col(|ui| {
                            if let Some(value) = bytes.values.get(cell.value) {
                                value::cell(ui, value);
                            } else {
                                ui.colored_label(
                                    palette::ERROR,
                                    "Unavailable byte-value reference",
                                );
                            }
                        });
                    });
                });
        });
}

pub(super) fn stack(ui: &mut Ui, stack: &[ValueInfo]) {
    ui.label(
        RichText::new(format!("{} stack values · top first", stack.len()))
            .small()
            .color(palette::MUTED),
    );
    egui::ScrollArea::both()
        .id_salt("frame_stack")
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show_rows(ui, 21.0, stack.len(), |ui, range| {
            for top in range {
                let index = stack.len() - top - 1;
                ui.horizontal(|ui| {
                    ui.monospace(format!("[{index:>4}]"));
                    value::cell(ui, &stack[index]);
                });
            }
        });
}
