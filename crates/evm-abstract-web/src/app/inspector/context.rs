//! Frame and effective-environment context beside the shared source selection.

use super::value;
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{AnalysisReport, EnvironmentSnapshot, FrameSnapshot};

pub(super) fn frame(ui: &mut Ui, frame: &FrameSnapshot) {
    if let Some(key) = &frame.process {
        super::process::frame(ui, key);
        if let Some(operands) = &frame.process_operands {
            egui::Grid::new("process_operands")
                .num_columns(2)
                .show(ui, |ui| {
                    value::named(ui, "Logical ADDRESS", &operands.address);
                    value::named(ui, "CALLER", &operands.caller);
                    value::named(ui, "CALLVALUE", &operands.value);
                });
        }
        if let Some(descriptor) = &frame.process_creation {
            super::process::creation(ui, descriptor);
        }
        ui.label(format!(
            "Rollback snapshot: {}",
            Symbol::Store(frame.rollback_store)
        ));
        return;
    }
    egui::Grid::new("frame_context")
        .num_columns(2)
        .show(ui, |ui| {
            let abstract_environment =
                frame.kind == evm_abstract_protocol::CodeKind::AbstractEnvironment;
            value::text(
                ui,
                if abstract_environment {
                    "Code namespace"
                } else {
                    "Code account"
                },
                frame.code_address.as_deref().unwrap_or("Not recorded"),
            );
            value::text(
                ui,
                if frame.address_operand.is_some() {
                    "Storage namespace"
                } else {
                    "Storage owner"
                },
                frame.storage_address.as_deref().unwrap_or("Not recorded"),
            );
            if let Some(address) = &frame.address_operand {
                value::named(ui, "Logical ADDRESS", address);
            } else {
                value::text(
                    ui,
                    "Logical ADDRESS",
                    frame
                        .address_value
                        .as_ref()
                        .map(value::address)
                        .unwrap_or_else(|| "Not recorded".into()),
                );
            }
            if let Some(caller) = &frame.caller_operand {
                value::named(ui, "CALLER", caller);
            } else {
                value::text(
                    ui,
                    "CALLER",
                    frame
                        .caller
                        .as_ref()
                        .map(value::address)
                        .unwrap_or_else(|| "Not recorded".into()),
                );
            }
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
            value::text(
                ui,
                "Code hash",
                if abstract_environment {
                    "Not observed"
                } else {
                    frame.code_hash.as_deref().unwrap_or("Not recorded")
                },
            );
            value::text(
                ui,
                "Block / context",
                if abstract_environment {
                    "Abstract environment".into()
                } else {
                    format!(
                        "{} · {:?}",
                        frame.basic_block.map_or_else(
                            || "No instruction block".into(),
                            |id| Symbol::Block(id).to_string()
                        ),
                        frame.context
                    )
                },
            );
            value::text(
                ui,
                "Rollback snapshot",
                Symbol::Store(frame.rollback_store).to_string(),
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
                        .map_or("None".into(), |block| Symbol::Block(block).to_string()),
                );
                value::named(ui, "Output offset", &continuation.output_offset);
                value::named(ui, "Output span", &continuation.output_size);
                if let Some(address) = &continuation.creation {
                    super::process::creation(ui, address);
                }
            });
    }
}

pub(super) fn environment(ui: &mut Ui, report: &AnalysisReport) {
    ui.label(RichText::new("Report work").strong());
    egui::Grid::new("report_work")
        .num_columns(2)
        .show(ui, |ui| {
            for (label, work) in [
                ("Execution work", report.metadata.work),
                (
                    "Postprocessing allowance",
                    report.metadata.postprocessing_max_work,
                ),
                ("Postprocessing work", report.metadata.postprocessing_work),
                ("Projection work", report.metadata.export_usage.work),
            ] {
                value::text(ui, label, work.to_string());
            }
        });
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
    effective_environment(
        ui,
        &report.metadata.environment,
        &report.metadata.fingerprint,
    );
}

pub(super) fn effective_environment(ui: &mut Ui, env: &EnvironmentSnapshot, fingerprint: &str) {
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
            value::text(ui, "World fingerprint", fingerprint);
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
