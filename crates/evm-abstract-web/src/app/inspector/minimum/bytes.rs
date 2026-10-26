//! Canonical minimum bytes and whole-word constraints share one data scope.

use super::value;
use egui::Ui;
use evm_abstract_protocol::{self as api, MinimumByteArray, MinimumByteContent};

pub(in crate::app::inspector) fn show(ui: &mut Ui, data: &MinimumByteArray) {
    egui::Grid::new("minimum_bytes_length")
        .num_columns(2)
        .show(ui, |ui| {
            value::named(ui, "Length", &data.length);
            value::text(ui, "Memory rules", data.memory.to_string());
        });
    match &data.content {
        MinimumByteContent::Zero => {
            ui.label("All content bytes are zero.");
        }
        MinimumByteContent::Exact(exact) => {
            super::super::buffers::exact(ui, exact, "minimum_exact_bytes");
        }
        MinimumByteContent::Cells(cells) => {
            ui.label("Unlisted byte cells are unconstrained.");
            byte_cells(ui, "minimum_byte_cells", cells);
        }
    }
    if !data.words.is_empty() {
        ui.label("Additional whole-word bounds");
        byte_cells(ui, "minimum_word_cells", &data.words);
    }
}

fn byte_cells(ui: &mut Ui, id: &str, cells: &[api::MinimumByteCell]) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(220.0)
        .show_rows(ui, 22.0, cells.len(), |ui, rows| {
            for row in rows {
                ui.horizontal(|ui| {
                    ui.monospace(format!("0x{:x}", cells[row].offset));
                    value::cell(ui, &cells[row].value);
                });
            }
        });
}
