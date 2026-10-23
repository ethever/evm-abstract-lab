//! Explain summarized interactions beside the actual caller and return edges.
mod regional;

use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, ProcedureSummaryKind, SummarySourceStatus};

pub(super) fn label(kind: ProcedureSummaryKind) -> &'static str {
    match kind {
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
                        ui.monospace(format!("Code: {}", procedure.code_address));
                        ui.monospace(format!("Storage namespace: {}", procedure.storage_address));
                        ui.label(format!(
                            "Static: {} · historically visited blocks: {:?}",
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
            regional::show(ui, &summary.regional_cfg, source);
        });
    }
}
