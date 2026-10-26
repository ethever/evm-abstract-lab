//! Compact workbench navigation, anchored panels and task status.

use super::{Command, View, Workspace, directory, inspector, terminal_coverage};
use crate::{
    controls::{self, Density},
    notation, palette,
};
use egui::{RichText, Ui};
use evm_abstract_notation::{Symbol, WideSymbol};
use evm_abstract_protocol::AnalysisStatus;

mod panels;
pub(super) use panels::PanelLayout;

impl Workspace {
    pub(super) fn header(&mut self, ui: &mut Ui) {
        let compact = ui.available_width() < 680.0;
        let density = if compact {
            Density::Touch
        } else {
            Density::Regular
        };
        density.scope(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if compact { "EVM" } else { "EVM LAB" })
                        .strong()
                        .color(palette::ACCENT),
                );
                let views = [
                    (View::Graph, "CFG"),
                    (View::Disassembly, "Code"),
                    (View::Ssa, "SSA"),
                    (View::Split, "Compare"),
                ];
                if compact {
                    let title = views
                        .iter()
                        .find(|(view, _)| *view == self.view)
                        .map_or("CFG", |(_, title)| *title);
                    let route = egui::ComboBox::from_id_salt("workspace_view")
                        .width(82.0)
                        .selected_text(title)
                        .show_ui(ui, |ui| {
                            for (view, title) in views {
                                controls::tab_value(ui, &mut self.view, view, title, density);
                            }
                        });
                    controls::observe(ui, &route.response, "Workspace view", "menu", None);
                } else {
                    for (view, title) in views {
                        let index = match view {
                            View::Split => 0,
                            View::Disassembly => 1,
                            View::Graph => 2,
                            View::Ssa => 3,
                        };
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
                    if controls::toggle(ui, &mut self.inspector.expanded, "Inspect", density)
                        .on_hover_text(self.inspector.visibility_hint(ui.ctx()))
                        .changed()
                        && self.inspector.expanded
                        && self.panels.compact()
                    {
                        self.directory.open = false;
                    }
                    if controls::toggle(ui, &mut self.directory.open, "Sources", density)
                        .on_hover_text(
                            "Captured code, compatible contexts and original account observations",
                        )
                        .changed()
                        && self.directory.open
                        && self.panels.compact()
                    {
                        self.inspector.expanded = false;
                    }
                });
            });
        });
    }

    pub(super) fn status(&mut self, ui: &mut Ui) -> Option<Command> {
        let mut command = None;
        let wide = ui.available_width() > 780.0;
        // Keep full decimal IDs and their controls outside the truncating
        // status row, including when a previous report remains visible.
        if self.task.snapshot.is_some() || self.report_job_id.is_some() {
            Density::Inline.scope(ui, |ui| {
                for (label, id) in self.job_identities() {
                    ui.push_id(label, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(egui::Label::new(format!("{label}: {}", id.0)).wrap());
                            if controls::action(ui, "Copy", Density::Inline)
                                .on_hover_text(format!("Copy {label}"))
                                .clicked()
                            {
                                ui.ctx().copy_text(id.0.to_string());
                            }
                        });
                    });
                }
                if self.report.is_some() && self.report_job_id.is_none() {
                    ui.label("Result Job ID: unavailable");
                }
            });
        }
        // Match the native initial row height so this bottom panel does not
        // shrink after the graph has fitted its first canvas.
        Density::Regular.scope(ui, |ui| {
            ui.horizontal(|ui| {
                if self.task.cancellable() && controls::action(ui, "Cancel", Density::Regular).clicked() {command=self.task.cancel();}
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
                } else if let Some(model)=&self.report {
                    if let Some(minimum)=self.report_view.minimum(model) {
                        ui.colored_label(if minimum.current{palette::ACCENT}else{palette::WARNING},terminal_coverage::detail_status(model));
                        ui.label(format!("{} qualified relations",minimum.summaries.len()));
                        ui.label(super::minimum::rich_status(model));
                        return;
                    }
                    let report = &self.report_view.shown(model);
                    let complete=report.hybrid.as_ref().map_or(report.status==AnalysisStatus::Converged&&report.ssa.complete, |hybrid| hybrid.coverage.baseline_current);
                    let label=terminal_coverage::detail_status(model);
                    ui.colored_label(if complete{palette::ACCENT}else{palette::WARNING},label);
                    if wide {ui.label(format!("{} states · {} programs · {} outcomes · {} work",report.cfg.len(),report.programs.len(),report.outcomes.len(),model.work));}
                    if let Some(hybrid)=&report.hybrid
                        && controls::action(ui, format!("Coverage · {} prefixes", hybrid.local_prefixes.len()), Density::Regular)
                            .on_hover_text(super::hybrid::summary(model).unwrap_or_default()).clicked() {
                        self.inspector.expanded=true;self.inspector.tab=inspector::Tab::Coverage;
                    }
                    if controls::action(ui, format!("Diagnostics {}",report.diagnostics.len()+report.frontiers.len()), Density::Regular)
                        .on_hover_text(format!("{} abstract-state diagnostic entries and {} unexpanded frontiers. Repeated PCs retain separate state evidence; counts are not instructions or failed transactions.",report.diagnostics.len(),report.frontiers.len())).clicked() {
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
        if let Some(report) = &self.report
            && let Some(status) = terminal_coverage::status(report)
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(status).color(palette::ACCENT));
                if controls::action(ui, "Inspect terminal coverage", Density::Compact).clicked() {
                    self.inspector.expanded = true;
                    self.inspector.tab = inspector::Tab::Outcomes;
                }
            });
        }
        command
    }
}

/// A narrow source drawer consumes space below navigation instead of covering
/// mode controls. Both representations use this geometry and the same IDs.
pub(super) fn source_panel(ui: &Ui, viewport: egui::Vec2) -> egui::Panel {
    if viewport.x >= 900.0 {
        egui::Panel::left("world_directory")
            .resizable(true)
            .default_size(208.0)
            .min_size(160.0)
            .max_size(320.0)
            .frame(palette::chrome_frame())
    } else {
        let maximum = (ui.available_height() * 0.42).clamp(0.0, 240.0);
        egui::Panel::top("sources_drawer")
            .resizable(true)
            .default_size(180.0_f32.min(maximum))
            .min_size(72.0_f32.min(maximum))
            .max_size(maximum)
            .frame(palette::chrome_frame())
    }
}

pub(super) fn source_heading(ui: &mut Ui, open: &mut bool) {
    Density::Compact.scope(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("SOURCES")
                    .small()
                    .strong()
                    .color(palette::MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if controls::action(ui, "Close", Density::Compact).clicked() {
                    *open = false;
                }
            });
        });
    });
}

#[cfg(test)]
mod tests;

pub(super) fn result_context(
    ui: &mut Ui,
    report: &crate::report::ReportView<'_>,
    program: Option<usize>,
    previous: bool,
    display: &mut crate::ssa_display::SsaDisplay,
) {
    let source = program.and_then(|id| report.programs.iter().find(|program| program.id == id));
    let program_label =
        source.map_or_else(
            || "No captured program".into(),
            |program| {
                format!(
                    "{} · {:?} · {}",
                    Symbol::Program(program.id),
                    program.kind,
                    directory::short(program.code_address.as_deref().unwrap_or(
                        match program.kind {
                            evm_abstract_protocol::CodeKind::Runtime => "Supplied input program",
                            evm_abstract_protocol::CodeKind::Initcode => "Constructor source",
                            _ => "No physical code account",
                        }
                    ))
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
                            ui.monospace(program.code_address.as_deref().unwrap_or(match program.kind {
                                evm_abstract_protocol::CodeKind::Runtime => "Supplied input program",
                                evm_abstract_protocol::CodeKind::Initcode => "Constructor source",
                                _ => "No physical code account",
                            }));
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
