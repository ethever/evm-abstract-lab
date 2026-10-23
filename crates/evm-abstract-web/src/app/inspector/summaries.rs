//! Explain summarized interactions beside the actual caller and return edges.
mod regional;

use super::process;
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, ProcedureSummaryKind, SummarySourceStatus};

pub(super) fn label(kind: ProcedureSummaryKind) -> &'static str {
    match kind {
        ProcedureSummaryKind::KnownProcess => "Conservative runtime process",
        ProcedureSummaryKind::UnknownEnvironment => "Abstract interactions",
        ProcedureSummaryKind::KnownRecursion => "Recursive bytecode",
        ProcedureSummaryKind::KnownCall => "Known bytecode call",
    }
}

pub(super) fn coverage_label(closed: bool, source: SummarySourceStatus) -> &'static str {
    match (source, closed) {
        (SummarySourceStatus::Current, true) => "closed for final source",
        (SummarySourceStatus::Current, false) => "open for final source",
        (SummarySourceStatus::Historical, true) => {
            "historical observation · closed for observed input"
        }
        (SummarySourceStatus::Historical, false) => {
            "historical observation · open for observed input"
        }
        (SummarySourceStatus::Uncertified, true) => {
            "closed for observed input · final source uncertified"
        }
        (SummarySourceStatus::Uncertified, false) => {
            "open for observed input · final source uncertified"
        }
    }
}

pub(super) fn show(ui: &mut Ui, report: &AnalysisReport, selected: Option<usize>) {
    let Some(state) = selected else { return };
    let latest = report
        .abstract_summaries
        .iter()
        .rposition(|summary| summary.source_state == state);
    for (id, summary) in report
        .abstract_summaries
        .iter()
        .enumerate()
        .filter(|(_, summary)| summary.source_state == state)
    {
        ui.separator();
        ui.label(RichText::new(label(summary.kind)).strong());
        let source = summary.source_status(Some(id) != latest);
        ui.label(coverage_label(summary.closed, source));
        if !summary.closed {
            ui.label("Inspect the reported frontiers for remaining obligations.");
        }
        ui.label(
            RichText::new(format!(
                "{} procedures · {} states · {} transfers · {} propagations · {} reused",
                summary.procedures.len(),
                summary.state_count,
                summary.transfers,
                summary.propagations,
                summary.reused,
            ))
            .small()
            .color(palette::MUTED),
        );
        ui.push_id(("abstract_interactions", id), |ui| {
            for (index, procedure) in summary.procedures.iter().enumerate() {
                let root = if summary.root_procedure == Some(index) {
                    " · entry"
                } else {
                    ""
                };
                ui.collapsing(
                    format!("Procedure {index} · {:?}{root}", procedure.kind),
                    |ui| {
                        if let Some(key) = &procedure.process {
                            process::frame(ui, key);
                        }
                        if let Some(input) = &procedure.process_input {
                            ui.label(format!(
                                "Joined ADDRESS: {} · CALLER: {} · CALLVALUE: {}",
                                input.address.summary, input.caller.summary, input.value.summary
                            ));
                        }
                        if let Some(address) = &procedure.code_address {
                            ui.monospace(format!("Code: {address}"));
                        }
                        if let Some(hash) = &procedure.code_hash {
                            ui.monospace(format!("Captured code hash: {hash}"));
                        }
                        ui.label(process::interpretation(procedure.semantics));
                        if let Some(address) = &procedure.storage_address {
                            ui.monospace(format!("Storage namespace: {address}"));
                        }
                        ui.label(format!("Static: {}", procedure.is_static));
                        if procedure.semantics == evm_abstract_protocol::TransferSemantics::Native {
                            ui.label(format!(
                                "Historically visited blocks: {:?}",
                                procedure.covered_blocks
                            ));
                        }
                        ui.label(format!(
                            "Callees: {:?} · results: {:?}",
                            procedure.dependencies, procedure.outcomes
                        ));
                        ui.label(format!(
                            "Input expansions: {} · output expansions: {}",
                            procedure.input_updates, procedure.output_updates
                        ));
                    },
                );
            }
            regional::show(ui, &summary.regional_cfg, source);
        });
    }
}
