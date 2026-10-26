//! Worker activity and the retained model have independent status and identity.

use crate::app::job::presentation::{DetailPresentation, ModelPresentation, TaskIntent};
use crate::app::{Command, Workspace, inspector, terminal_coverage};
use crate::{
    controls::{self, Density},
    notation, palette,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::JobId;

impl Workspace {
    pub(in crate::app) fn job_identities(&self) -> impl Iterator<Item = (&'static str, JobId)> {
        let presentation = self.task_presentation();
        let current = presentation.job_id;
        let result = presentation.report_job_id;
        let label = if self.report.is_some() && result != current {
            "Current Job ID"
        } else {
            "Job ID"
        };
        [
            (label, current),
            ("Result Job ID", result.filter(|id| Some(*id) != current)),
        ]
        .into_iter()
        .filter_map(|(label, id)| id.map(|id| (label, id)))
    }

    pub(in crate::app) fn status(&mut self, ui: &mut Ui) -> Option<Command> {
        let presentation = self.task_presentation();
        let wide = ui.available_width() > 780.0;
        let mut command = None;
        if presentation.job_id.is_some() || presentation.report_job_id.is_some() {
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
                        })
                    });
                }
                if self.report.is_some() && presentation.report_job_id.is_none() {
                    ui.label("Result Job ID: unavailable");
                }
            });
        }
        Density::Regular.scope(ui, |ui| ui.horizontal(|ui| {
            if presentation.cancellable && controls::action(ui, "Cancel", Density::Regular).clicked() {
                command = self.task.apply_intent(TaskIntent::Cancel);
            }
            if let Some(wait) = self.task.wait.summary(super::super::timing::now()) {
                let response = if self.report.is_some() { controls::action(ui, wait, Density::Regular) } else { ui.label(wait) };
                if response.clicked() && self.report.is_some() {
                    self.inspector.expanded = true;
                    self.inspector.tab = inspector::Tab::Timing;
                }
                response.on_hover_text(self.accessible_timing());
            }
            if presentation.busy || self.preparing_current_view() {
                ui.spinner();
                let label = if self.preparing_current_view() { self.task_label() } else { presentation.label() };
                ui.add(egui::Label::new(RichText::new(label).color(palette::ACCENT)).truncate()).on_hover_text(label);
                if wide && let Some(snapshot) = &self.task.snapshot {
                    ui.label(format!("{:?} · {} accounts · {} states · {} transfers", snapshot.progress.phase,
                        snapshot.progress.accounts, snapshot.progress.states, snapshot.progress.transfers));
                }
                if let Some(error) = &self.task.error {
                    ui.colored_label(palette::WARNING, "Connection interrupted").on_hover_text(format!(
                        "{error}\nTask status is retained; polling will retry. Cancel can be retried."));
                }
            } else if let Some(error) = &self.task.error {
                if matches!(presentation.model, ModelPresentation::Current { ready: true }) {
                    ui.colored_label(palette::ACCENT, "Model ready");
                }
                if controls::action(ui, "Edit inputs", Density::Regular).clicked() {
                    self.form.open = true;
                    self.form.error = Some(error.to_string());
                }
                ui.add(egui::Label::new(notation::widget(ui, RichText::new(error.to_string()).color(palette::ERROR))).truncate())
                    .on_hover_ui(|ui| {
                        if let crate::app::job::TaskError::Api(error) = error { inspector::api_error_details(ui, error); }
                        else { ui.label(notation::widget(ui, error.to_string())); }
                    });
            } else if let Some(model) = &self.report {
                let ready = matches!(presentation.model, ModelPresentation::Current { ready: true } | ModelPresentation::Previous { ready: true });
                let full = terminal_coverage::detail_status(model);
                let label = if wide { full } else if ready { "Model ready" } else { "Model partial" };
                ui.colored_label(if ready { palette::ACCENT } else { palette::WARNING }, label).on_hover_text(full);
                ui.add(egui::Label::new(detail_label(presentation.detail)).truncate()).on_hover_text(super::super::minimum::rich_status(model));
                if wide {
                    if let Some(minimum) = self.report_view.minimum(model) {
                        ui.label(format!("{} qualified relations", minimum.summaries.len()));
                    } else {
                        let shown = self.report_view.shown(model);
                        ui.label(format!("{} states · {} programs · {} work", shown.cfg.len(), shown.programs.len(), model.work));
                    }
                }
                let shown = (!self.report_view.identity().is_minimum()).then(|| self.report_view.shown(model));
                let diagnostics = shown.as_ref().map(|report| report.diagnostics.len() + report.frontiers.len());
                if wide && let Some(count) = diagnostics
                    && controls::action(ui, format!("Diagnostics {count}"), Density::Regular).clicked() {
                    self.inspector.expanded = true;
                    self.inspector.tab = inspector::Tab::Diagnostics;
                }
                if wide && let Some(hybrid) = &model.hybrid
                    && controls::action(ui, format!("Coverage · {} prefixes", hybrid.local_prefixes.len()), Density::Regular)
                        .on_hover_text(crate::app::hybrid::summary(model).unwrap_or_default()).clicked() {
                    self.inspector.expanded = true;
                    self.inspector.tab = inspector::Tab::Coverage;
                }
            } else if self.task.phase == crate::app::job::TaskPhase::Cancelled {
                ui.colored_label(palette::WARNING, "Cancelled · worker cleanup acknowledged");
            } else if self.startup.loading() {
                ui.spinner();
                ui.label("Loading the initial example…");
            } else {
                ui.label(presentation.label());
            }
            if self.previous_report {
                ui.label(RichText::new("Previous result").small().color(palette::WARNING))
                    .on_hover_text("The visible report belongs to the previous completed task.");
            }
        }));
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

fn detail_label(detail: DetailPresentation) -> &'static str {
    match detail {
        DetailPresentation::Unavailable => "Detail unavailable",
        DetailPresentation::Running => "Detail running",
        DetailPresentation::Incomplete => "Detail partial",
        DetailPresentation::Ready => "Detail ready",
        DetailPresentation::Disabled => "Detail off",
        DetailPresentation::Refused => "Detail refused",
        DetailPresentation::Cancelled => "Detail cancelled",
    }
}
