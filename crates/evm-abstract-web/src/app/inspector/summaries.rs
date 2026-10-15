//! Explain summarized interactions beside the actual caller and return edges.

use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, ProcedureSummaryKind};

pub(super) fn label(kind: ProcedureSummaryKind) -> &'static str {
    match kind {
        ProcedureSummaryKind::UnknownEnvironment => "Abstract interactions",
        ProcedureSummaryKind::KnownRecursion => "Recursive bytecode",
    }
}

pub(super) fn show(ui: &mut Ui, report: &AnalysisReport, selected: Option<usize>) {
    let Some(state) = selected else { return };
    for (id, summary) in report
        .abstract_summaries
        .iter()
        .enumerate()
        .filter(|(_, summary)| summary.source_state == state)
    {
        ui.separator();
        ui.label(RichText::new(label(summary.kind)).strong());
        ui.label(if summary.closed {
            "Covered by a closed conservative relation"
        } else {
            "Coverage remains open; inspect the reported frontiers"
        });
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
                        ui.monospace(format!("Code: {}", procedure.code_address));
                        ui.monospace(format!("Storage namespace: {}", procedure.storage_address));
                        ui.label(format!(
                            "Static: {} · analyzed blocks: {:?}",
                            procedure.is_static, procedure.covered_blocks
                        ));
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
        });
    }
}
