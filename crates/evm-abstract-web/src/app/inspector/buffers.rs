//! Sparse byte observations keep their length and unspecified-byte defaults.

use super::value;
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, ValueInfo};

pub(super) fn show(ui: &mut Ui, report: &AnalysisReport, id: usize, title: &str) {
    let Some(bytes) = report.byte_arrays.get(id) else {
        ui.colored_label(
            palette::ERROR,
            "The report references an unavailable byte-array snapshot",
        );
        return;
    };
    egui::Grid::new(("byte_array_header", title))
        .num_columns(2)
        .show(ui, |ui| {
            value::named(ui, "Length", &bytes.length);
            value::named(ui, "Unspecified in-range bytes", &bytes.default);
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
    if bytes.cells.is_empty() {
        ui.label("No explicit cells. Interpret the length together with the default above.");
        return;
    }
    egui::ScrollArea::both()
        .id_salt(("byte_cells", title, id))
        .auto_shrink([false, false])
        .show_rows(ui, 21.0, bytes.cells.len(), |ui, range| {
            for index in range {
                let cell = &bytes.cells[index];
                ui.horizontal(|ui| {
                    ui.monospace(format!("0x{:08x}", cell.offset));
                    if let Some(value) = bytes.values.get(cell.value) {
                        value::cell(ui, value);
                    } else {
                        ui.colored_label(palette::ERROR, "Unavailable byte-value reference");
                    }
                });
            }
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
