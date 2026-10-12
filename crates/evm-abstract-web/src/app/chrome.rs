//! Compact global navigation and task status; configuration lives in a modal.

use super::{Command, View, Workspace, directory, inspector};
use crate::{
    controls::{self, Density},
    notation, palette,
};
use egui::{RichText, Ui};
use evm_abstract_notation::{Symbol, WideSymbol};
use evm_abstract_protocol::{AnalysisReport, AnalysisStatus};

impl Workspace {
    pub(super) fn header(&mut self, ui: &mut Ui) {
        let compact = ui.available_width() < 800.0;
        let density = Density::Regular;
        density.scope(ui, |ui| {
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
                                controls::tab_value(ui, &mut self.view, view, title, density);
                            }
                        });
                } else {
                    for (index, (view, title)) in views.into_iter().enumerate() {
                        controls::tab_value(ui, &mut self.view, view, title, density)
                            .on_hover_text(format!("Shortcut {index}"));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if controls::action(ui, if compact { "Input" } else { "New analysis" }, density)
                        .clicked()
                    {
                        self.form.open = true;
                    }
                    if compact {
                        controls::menu_button(ui, "Panels", density, |ui| {
                            ui.checkbox(&mut self.directory.open, "Programs / accounts");
                            ui.checkbox(&mut self.inspector.expanded, "State inspector")
                                .on_hover_text(self.inspector.visibility_hint(ui.ctx()));
                        });
                    } else {
                        controls::toggle(ui, &mut self.inspector.expanded, "Inspect", density)
                            .on_hover_text(self.inspector.visibility_hint(ui.ctx()));
                        controls::toggle(ui, &mut self.directory.open, "World", density);
                    }
                });
            });
        });
    }

    pub(super) fn status(&mut self, ui: &mut Ui) -> Option<Command> {
        let mut command = None;
        let wide = ui.available_width() > 780.0;
        // Match the native initial row height so this bottom panel does not
        // shrink after the graph has fitted its first canvas.
        Density::Regular.scope(ui, |ui| {
            ui.horizontal(|ui| {
                // Reserve elapsed time before truncating status/progress. It
                // remains visible on narrow screens and during disconnection.
                if let Some(wait) = self.task.wait.summary(super::timing::now()) {
                    let response = if self.report.is_some() {
                        controls::action(ui, wait, Density::Regular)
                    } else {
                        ui.label(wait)
                    };
                    if response.clicked() && self.report.is_some() {
                        self.inspector.expanded = true;
                        self.inspector.tab = inspector::Tab::Timing;
                    }
                    response.on_hover_text(self.accessible_timing());
                }
                if self.task.cancellable() && controls::action(ui, "Cancel", Density::Regular).clicked() {command=self.task.cancel();}
                if self.task.busy() || self.preparing_current_view() {
                    ui.spinner();
                    ui.add(egui::Label::new(RichText::new(self.task_label()).color(palette::ACCENT)).truncate()).on_hover_text(self.task_label());
                    if wide && let Some(snapshot)=&self.task.snapshot {
                            ui.label(notation::widget(ui,format!("{} · {:?} · {} accounts · {} slots · {} states · {} transfers",WideSymbol::Task(snapshot.id.0),snapshot.progress.phase,snapshot.progress.accounts,snapshot.progress.slots,snapshot.progress.states,snapshot.progress.transfers)));
                    }
                    if let Some(error)=&self.task.error {
                        ui.colored_label(palette::WARNING,"Connection interrupted").on_hover_text(notation::widget(ui, format!("{error}\nTask status is retained; polling will retry. Cancel can be retried.")));
                    }
                } else if let Some(error)=&self.task.error {
                    if controls::action(ui, "Edit inputs", Density::Regular).clicked() {self.form.open=true;self.form.error=Some(error.to_string());}
                    ui.add(egui::Label::new(notation::widget(ui,RichText::new(error.to_string()).color(palette::ERROR))).truncate()).on_hover_ui(|ui| {
                        if let super::job::TaskError::Api(error)=error { inspector::api_error_details(ui,error); }
                        else { ui.label(notation::widget(ui,error.to_string())); }
                    });
                } else if self.task.phase==super::job::TaskPhase::Cancelled {
                    ui.colored_label(palette::WARNING,"Cancelled · worker cleanup acknowledged");
                } else if let Some(report)=&self.report {
                    let complete=report.status==AnalysisStatus::Converged&&report.ssa.complete;
                    let label=if complete{"Model converged"}else{"Incomplete / partial SSA"};
                    ui.colored_label(if complete{palette::ACCENT}else{palette::WARNING},label);
                    if wide {ui.label(format!("{} states · {} programs · {} outcomes · {} work",report.cfg.len(),report.programs.len(),report.outcomes.len(),report.metadata.work));}
                    if controls::action(ui, format!("Diagnostics {}",report.diagnostics.len()+report.frontiers.len()), Density::Regular).clicked() {
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
        });
        command
    }
}

pub(super) fn result_context(
    ui: &mut Ui,
    report: &AnalysisReport,
    program: Option<usize>,
    previous: bool,
    display: &mut crate::ssa_display::SsaDisplay,
) {
    let source = program.and_then(|id| report.programs.iter().find(|program| program.id == id));
    let program_label = source.map_or_else(
        || "No captured program".into(),
        |program| {
            format!(
                "{} · {:?} · {}",
                Symbol::Program(program.id),
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
    // Metadata retains the original small-button height rather than reserving
    // a toolbar row above the analysis panes.
    Density::Inline.scope(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut full = *display == crate::ssa_display::SsaDisplay::Full;
                if controls::toggle(ui, &mut full, "SSA details", Density::Inline)
                    .on_hover_text("Show completed stack operations, effect chains and completion annotations in SSA views")
                    .changed()
                {
                    *display = if full {
                        crate::ssa_display::SsaDisplay::Full
                    } else {
                        crate::ssa_display::SsaDisplay::Compact
                    };
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(notation::widget(
                            ui,
                            RichText::new(label).small().color(palette::MUTED),
                        ))
                        .truncate(),
                    )
                    .on_hover_ui(|ui| {
                        if let Some(program) = source {
                            ui.monospace(&program.code_address);
                            ui.monospace(&program.code_hash);
                        }
                        if let Some(snapshot) = &report.metadata.snapshot {
                            ui.monospace(&snapshot.block_hash);
                            if controls::action(ui, "Copy block hash", Density::Compact).clicked() {
                                ui.ctx().copy_text(snapshot.block_hash.clone());
                            }
                        }
                        ui.monospace(&report.metadata.fingerprint);
                    });
                });
            });
        });
    });
}
