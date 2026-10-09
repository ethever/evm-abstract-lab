//! Structured transaction inputs and opt-in block-environment overrides.

use egui::{RichText, Ui};
use evm_abstract_protocol::{AddressSetting, CalldataInput, IndexedHash, WordInput};

use super::{AnalysisForm, Source, optional_text};
use crate::{framework, palette};

impl AnalysisForm {
    pub(super) fn environment(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Transaction environment").show(ui, |ui| {
            address(
                ui,
                "Caller",
                "environment_caller",
                &mut self.environment.caller,
            );
            let mut origin = self.environment.origin.is_some();
            if ui
                .checkbox(&mut origin, "Separate transaction origin")
                .changed()
            {
                self.environment.origin = origin.then_some(AddressSetting::Unknown);
            }
            if let Some(origin) = &mut self.environment.origin {
                address(ui, "Origin", "environment_origin", origin);
            } else {
                ui.label(
                    RichText::new("Origin shares the caller's identity")
                        .small()
                        .color(palette::MUTED),
                );
            }
            word(
                ui,
                "Call value",
                "environment_value",
                &mut self.environment.call_value,
            );
            word(
                ui,
                "Gas price",
                "environment_gas_price",
                &mut self.environment.gas_price,
            );
            ui.horizontal(|ui| {
                ui.label("Calldata");
                let exact = matches!(self.environment.calldata, CalldataInput::Exact(_));
                if ui.selectable_label(!exact, "Unknown").clicked() {
                    self.environment.calldata = CalldataInput::Unknown;
                }
                if ui.selectable_label(exact, "Exact").clicked() && !exact {
                    self.environment.calldata = CalldataInput::Exact(String::new());
                }
            });
            if let CalldataInput::Exact(bytes) = &mut self.environment.calldata {
                framework::line_editor(
                    ui,
                    "environment_calldata",
                    bytes,
                    "0x… (empty = no calldata)",
                );
            }
            optional_text(
                ui,
                "Initial gas upper bound",
                "environment_gas",
                &mut self.environment.gas_upper_bound,
                "Unknown initial gas; GAS remains an upper-bound observation",
                "0x…",
            );
            ui.checkbox(&mut self.environment.is_static, "Static root call");
        });
        egui::CollapsingHeader::new("Block environment overrides").show(ui, |ui| {
            ui.label(RichText::new(match self.source {
                Source::Rpc => "Unchecked fields come from the pinned block when available. The report shows the actual observations used.",
                Source::Bytecode => "Unchecked fields stay unknown for standalone bytecode.",
            }).small().color(palette::MUTED));
            for (label, id, value) in [
                ("Block number", "environment_number", &mut self.environment.number),
                ("Timestamp", "environment_timestamp", &mut self.environment.timestamp),
                ("Beneficiary", "environment_coinbase", &mut self.environment.coinbase),
                ("PREVRANDAO", "environment_prevrandao", &mut self.environment.prevrandao),
                ("Block gas limit", "environment_gas_limit", &mut self.environment.gas_limit),
                ("Execution chain ID", "environment_chain_id", &mut self.environment.chain_id),
                ("Base fee", "environment_base_fee", &mut self.environment.base_fee),
                ("Blob base fee", "environment_blob_fee", &mut self.environment.blob_base_fee),
            ] {
                optional_text(ui, label, id, value, "Use default observation", "0x…");
            }
        });
        egui::CollapsingHeader::new("Block and blob hash observations").show(ui, |ui| {
            word(
                ui,
                "Blob count",
                "environment_blob_count",
                &mut self.environment.blob_count,
            );
            hash_rows(
                ui,
                "block_hashes",
                "Block hashes",
                &mut self.environment.block_hashes,
            );
            hash_rows(
                ui,
                "blob_hashes",
                "Blob hashes",
                &mut self.environment.blob_hashes,
            );
        });
    }
}

fn address(ui: &mut Ui, label: &str, id: &str, value: &mut AddressSetting) {
    ui.horizontal(|ui| {
        ui.label(label);
        let known = matches!(value, AddressSetting::Concrete(_));
        if ui.selectable_label(!known, "Unknown").clicked() {
            *value = AddressSetting::Unknown;
        }
        if ui.selectable_label(known, "Address").clicked() && !known {
            *value = AddressSetting::Concrete(String::new());
        }
    });
    if let AddressSetting::Concrete(address) = value {
        framework::line_editor(ui, id, address, "0x… (20 bytes)");
    }
}

fn word(ui: &mut Ui, label: &str, id: &str, value: &mut WordInput) {
    ui.horizontal(|ui| {
        ui.label(label);
        let known = matches!(value, WordInput::Concrete(_));
        if ui.selectable_label(!known, "Unknown").clicked() {
            *value = WordInput::Unknown;
        }
        if ui.selectable_label(known, "Exact").clicked() && !known {
            *value = WordInput::Concrete(String::new());
        }
    });
    if let WordInput::Concrete(word) = value {
        framework::line_editor(ui, id, word, "0x… (256-bit word)");
    }
}

fn hash_rows(ui: &mut Ui, id: &str, title: &str, values: &mut Vec<IndexedHash>) {
    ui.push_id(id, |ui| {
        ui.label(RichText::new(title).color(palette::MUTED));
        let mut remove = None;
        for (index, value) in values.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Index");
                    let width = (ui.available_width() - 72.0).max(60.0);
                    framework::scoped_editor(ui, "index", &mut value.index, "0x…", width);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                let width = ui.available_width();
                framework::scoped_editor(ui, "hash", &mut value.hash, "0x… (32-byte hash)", width);
            });
        }
        if let Some(index) = remove {
            values.remove(index);
        }
        if ui.small_button("Add hash").clicked() {
            values.push(IndexedHash {
                index: "0x0".into(),
                hash: String::new(),
            });
        }
    });
}
