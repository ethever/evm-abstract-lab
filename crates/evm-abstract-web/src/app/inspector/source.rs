//! A source-only focus cannot inherit an unrelated procedure or Native snapshot.

use super::{InspectionSource, Inspector, Tab, buffers, evidence, process, scroll, value};
use egui::Ui;
use evm_abstract_protocol::AnalysisReport;

impl Inspector {
    pub(super) fn source_contents(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        source: InspectionSource<'_>,
    ) {
        self.navigation(ui);
        ui.label("Source only · no execution context selected");
        scroll(ui, "source_inspection", |ui| {
            if self.tab == Tab::Timing {
                super::super::timing::show(ui, report, &self.timings);
                return;
            }
            if self.tab == Tab::Acquisition {
                evidence::acquisition_rows(
                    ui,
                    report.acquisition.as_ref(),
                    report
                        .minimum
                        .as_ref()
                        .map_or(0, |model| model.accounts.len()),
                );
                return;
            }
            let program = match source {
                InspectionSource::Program(id) => report
                    .programs
                    .iter()
                    .find(|program| program.id == id)
                    .map(AsRef::as_ref),
                InspectionSource::Process(source) => {
                    process::source(ui, source);
                    report.process_program(source)
                }
            };
            if let Some(program) = program {
                egui::Grid::new("inspector_source_identity")
                    .num_columns(2)
                    .show(ui, |ui| {
                        value::text(
                            ui,
                            "Code source",
                            program
                                .code_address
                                .as_deref()
                                .unwrap_or("Supplied or constructor bytes"),
                        );
                        value::text(ui, "Kind", format!("{:?}", program.kind));
                        value::text(ui, "Fork", format!("{:?}", program.fork));
                        value::text(ui, "Code hash", &program.code_hash);
                        value::text(
                            ui,
                            "Decoded source blocks",
                            program.blocks.len().to_string(),
                        );
                    });
                ui.collapsing("Captured bytecode", |ui| {
                    buffers::exact(ui, &program.bytecode, "source_bytes")
                });
                ui.collapsing("Advanced source identity", |ui| {
                    ui.monospace(format!("Program {}", program.id));
                });
            } else {
                ui.label("No captured bytecode is attached to this source.");
            }
            ui.label("Choose a compatible context in Sources or the CFG to inspect procedure bounds, joint outcomes or Native block snapshots.");
            if !matches!(self.tab, Tab::Frame | Tab::Coverage) {
                ui.label("This field needs an explicit observation. Source geometry does not supply state, effects or SSA facts.");
            }
        });
    }
}
