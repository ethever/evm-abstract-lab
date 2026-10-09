//! Frame and effective-environment context beside the shared source selection.

use super::value;
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, FrameSnapshot};

pub(super) fn frame(ui: &mut Ui, frame: &FrameSnapshot) {
    egui::Grid::new("frame_context")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(ui, "Code account", &frame.code_address);
            value::text(ui, "Storage owner", &frame.storage_address);
            value::text(ui, "Logical ADDRESS", value::address(&frame.address_value));
            value::text(ui, "CALLER", value::address(&frame.caller));
            value::named(ui, "CALLVALUE", &frame.call_value);
            value::text(
                ui,
                "Mode",
                format!(
                    "{:?}{}",
                    frame.kind,
                    if frame.is_static { " · static" } else { "" }
                ),
            );
            value::text(ui, "Code hash", &frame.code_hash);
            value::text(
                ui,
                "Block / context",
                format!("B{} · {:?}", frame.basic_block, frame.context),
            );
            value::text(
                ui,
                "Rollback snapshot",
                format!("Store #{}", frame.rollback_store),
            );
        });
    if let Some(continuation) = &frame.continuation {
        ui.separator();
        ui.label(RichText::new("Caller continuation").color(palette::MUTED));
        egui::Grid::new("frame_continuation")
            .num_columns(2)
            .show(ui, |ui| {
                value::text(
                    ui,
                    "Return block",
                    continuation
                        .return_block
                        .map_or("None".into(), |block| format!("B{block}")),
                );
                value::named(ui, "Output offset", &continuation.output_offset);
                value::named(ui, "Output span", &continuation.output_size);
                if let Some(address) = &continuation.creation {
                    value::text(ui, "Creation target", address);
                }
            });
    }
}

pub(super) fn environment(ui: &mut Ui, report: &AnalysisReport) {
    if let Some(snapshot) = &report.metadata.snapshot {
        ui.label(RichText::new("Captured chain snapshot").strong());
        egui::Grid::new("snapshot_context")
            .num_columns(2)
            .show(ui, |ui| {
                value::text(ui, "Chain ID", &snapshot.chain_id);
                value::text(ui, "Canonical block hash", &snapshot.block_hash);
                for (name, observation) in [
                    ("Block number", &snapshot.number),
                    ("Parent hash", &snapshot.parent_hash),
                    ("Timestamp", &snapshot.timestamp),
                    ("Beneficiary", &snapshot.coinbase),
                    ("PREVRANDAO", &snapshot.prevrandao),
                    ("Gas limit", &snapshot.gas_limit),
                    ("Base fee", &snapshot.base_fee),
                    ("Blob base fee", &snapshot.blob_base_fee),
                    ("Excess blob gas", &snapshot.excess_blob_gas),
                    ("Blob gas used", &snapshot.blob_gas_used),
                ] {
                    value::text(ui, name, observation.as_deref().unwrap_or("Not reported"));
                }
            });
        ui.separator();
    }
    ui.label(RichText::new("Effective execution environment").strong());
    ui.label(
        RichText::new(
            "Includes explicit overrides; execution values are separate from snapshot identity.",
        )
        .small()
        .color(palette::MUTED),
    );
    let env = &report.metadata.environment;
    egui::Grid::new("effective_environment")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(ui, "ADDRESS", value::address(&env.to));
            value::text(ui, "CALLER", value::address(&env.caller));
            value::text(ui, "ORIGIN", value::address(&env.origin));
            value::named(ui, "CALLVALUE", &env.call_value);
            value::named(ui, "Gas price", &env.gas_price);
            value::text(ui, "Beneficiary", value::address(&env.coinbase));
            for (name, observed) in [
                ("Timestamp", &env.timestamp),
                ("Block number", &env.number),
                ("PREVRANDAO", &env.prevrandao),
                ("Block gas limit", &env.gas_limit),
                ("Execution CHAINID", &env.chain_id),
                ("Base fee", &env.base_fee),
                ("Blob base fee", &env.blob_base_fee),
                ("Blob count", &env.blob_count),
            ] {
                value::named(ui, name, observed);
            }
            value::text(
                ui,
                "Initial gas upper bound",
                env.gas_upper_bound.as_deref().unwrap_or("Unknown"),
            );
            value::text(ui, "Static", env.is_static.to_string());
            value::text(ui, "World fingerprint", &report.metadata.fingerprint);
        });
    for (name, hashes) in [
        ("Block hashes", &env.block_hashes),
        ("Blob hashes", &env.blob_hashes),
    ] {
        if !hashes.is_empty() {
            ui.collapsing(format!("{name} ({})", hashes.len()), |ui| {
                for hash in hashes {
                    ui.monospace(format!("{} → {}", hash.index, hash.hash));
                }
            });
        }
    }
}
