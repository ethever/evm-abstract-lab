//! Compact global navigation and task status; configuration lives in a modal.

use super::{Command, View, Workspace, directory, inspector};
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisReport, AnalysisStatus};

impl Workspace {
    pub(super) fn header(&mut self, ui: &mut Ui) {
        let compact = ui.available_width() < 800.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if compact {
                    "EVM LAB"
                } else {
                    "EVM ABSTRACT LAB"
                })
                .strong()
                .color(palette::ACCENT),
            );
            let views = [
                (View::Split, "Workspace"),
                (View::Disassembly, "Code"),
                (View::Graph, "CFG"),
                (View::Ssa, "SSA"),
            ];
            if compact {
                let title = views
                    .iter()
                    .find(|(view, _)| *view == self.view)
                    .map_or("Workspace", |(_, title)| *title);
                egui::ComboBox::from_id_salt("workspace_view")
                    .width(94.0)
                    .selected_text(title)
                    .show_ui(ui, |ui| {
                        for (view, title) in views {
                            ui.selectable_value(&mut self.view, view, title);
                        }
                    });
            } else {
                for (index, (view, title)) in views.into_iter().enumerate() {
                    ui.selectable_value(&mut self.view, view, title)
                        .on_hover_text(format!("Shortcut {index}"));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if compact { "Input" } else { "New analysis" })
                    .clicked()
                {
                    self.form.open = true;
                }
                if compact {
                    ui.menu_button("Panels", |ui| {
                        ui.checkbox(&mut self.directory.open, "Programs / accounts");
                        ui.checkbox(&mut self.inspector.expanded, "State inspector");
                    });
                } else {
                    ui.toggle_value(&mut self.inspector.expanded, "Inspect");
                    ui.toggle_value(&mut self.directory.open, "World");
                }
            });
        });
    }

    pub(super) fn status(&mut self, ui: &mut Ui) -> Option<Command> {
        let mut command = None;
        let wide = ui.available_width() > 780.0;
        ui.horizontal(|ui| {
            if self.task.cancellable() && ui.button("Cancel").clicked() {command=self.task.cancel();}
            if self.task.busy() {
                ui.spinner();
                ui.add(egui::Label::new(RichText::new(self.task.label()).color(palette::ACCENT)).truncate()).on_hover_text(self.task.label());
                if wide && let Some(snapshot)=&self.task.snapshot {
                        ui.label(format!("#{:?} · {:?} · {} accounts · {} slots · {} states · {} transfers",snapshot.id.0,snapshot.progress.phase,snapshot.progress.accounts,snapshot.progress.slots,snapshot.progress.states,snapshot.progress.transfers));
                }
                if let Some(error)=&self.task.error {
                    ui.colored_label(palette::WARNING,"Connection interrupted").on_hover_text(format!("{error}\nTask status is retained; polling will retry. Cancel can be retried."));
                }
            } else if let Some(error)=&self.task.error {
                if ui.button("Edit inputs").clicked() {self.form.open=true;self.form.error=Some(error.to_string());}
                ui.add(egui::Label::new(RichText::new(error.to_string()).color(palette::ERROR)).truncate()).on_hover_ui(|ui| {
                    if let super::job::TaskError::Api(error)=error { inspector::api_error_details(ui,error); }
                    else { ui.label(error.to_string()); }
                });
            } else if self.task.phase==super::job::TaskPhase::Cancelled {
                ui.colored_label(palette::WARNING,"Cancelled · worker cleanup acknowledged");
            } else if let Some(report)=&self.report {
                let complete=report.status==AnalysisStatus::Converged&&report.ssa.complete;
                let label=if complete{"Model converged"}else{"Incomplete / partial SSA"};
                ui.colored_label(if complete{palette::ACCENT}else{palette::WARNING},label);
                if wide {ui.label(format!("{} states · {} programs · {} outcomes · {} work",report.cfg.len(),report.programs.len(),report.outcomes.len(),report.metadata.work));}
                if ui.button(format!("Diagnostics {}",report.diagnostics.len()+report.frontiers.len())).clicked() {
                    self.inspector.expanded=true;self.inspector.tab=inspector::Tab::Diagnostics;
                }
            } else if self.startup.loading() {
                ui.spinner();
                ui.label(RichText::new("Loading the initial example…").color(palette::MUTED));
            } else {
                ui.label(RichText::new(self.task.label()).color(palette::MUTED));
            }
            if self.previous_report {
                ui.label(RichText::new("Previous result").small().color(palette::WARNING)).on_hover_text("The visible report belongs to the previous completed task.");
            }
        });
        command
    }
}

pub(super) fn result_context(
    ui: &mut Ui,
    report: &AnalysisReport,
    program: Option<usize>,
    previous: bool,
) {
    let source = program.and_then(|id| report.programs.iter().find(|program| program.id == id));
    let program_label = source.map_or_else(
        || "No captured program".into(),
        |program| {
            format!(
                "P{} · {:?} · {}",
                program.id,
                program.kind,
                directory::short(&program.code_address)
            )
        },
    );
    let snapshot = report.metadata.snapshot.as_ref().map_or_else(
        || "Standalone bytecode".into(),
        |snapshot| {
            format!(
                "chain {} · block {} · {}",
                snapshot.chain_id,
                snapshot.number.as_deref().unwrap_or("unreported"),
                directory::short(&snapshot.block_hash)
            )
        },
    );
    let label = format!(
        "{}{} · {:?} · {snapshot}",
        if previous { "Previous result · " } else { "" },
        program_label,
        report.fork
    );
    ui.add(egui::Label::new(RichText::new(label).small().color(palette::MUTED)).truncate())
        .on_hover_ui(|ui| {
            if let Some(program) = source {
                ui.monospace(&program.code_address);
                ui.monospace(&program.code_hash);
            }
            if let Some(snapshot) = &report.metadata.snapshot {
                ui.monospace(&snapshot.block_hash);
                if ui.small_button("Copy block hash").clicked() {
                    ui.ctx().copy_text(snapshot.block_hash.clone());
                }
            }
            ui.monospace(&report.metadata.fingerprint);
        });
}
