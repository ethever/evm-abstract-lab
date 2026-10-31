//! Independent local decompiler product workspace. The core owns every
//! semantic stage, transformation and origin; this layer owns presentation.

mod details;
mod diff;
mod form;
mod index;
mod inspector;
mod model;
mod rows;
mod structured;
mod task;
mod views;

use crate::{
    controls::{self, Density},
    palette,
};
use evm_abstract_protocol::{DecompileArtifact, Fork};
pub(crate) use task::{Command, Message};

pub(crate) struct Workspace {
    artifact: Option<DecompileArtifact>,
    task: task::Task,
    form: form::Form,
    state: model::ViewState,
    views: views::Views,
    inspector: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            artifact: None,
            task: task::Task::default(),
            form: form::Form::default(),
            state: model::ViewState::default(),
            views: views::Views::default(),
            inspector: true,
        }
    }
}

impl Workspace {
    fn previous_artifact(&self) -> bool {
        self.state.publication.is_some_and(|publication| {
            self.task.snapshot.as_ref().map(|snapshot| snapshot.id) != Some(publication.job)
        })
    }
    pub fn load_input(&mut self, bytecode: String, fork: Fork) {
        self.form.load(bytecode, fork);
    }
    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        self.views.prepare_input(input);
    }
    pub fn inspector_shortcut_available(&self) -> bool {
        self.artifact.is_some() && !self.form.open
    }
    pub fn toggle_inspector(&mut self) {
        self.inspector = !self.inspector;
    }

    pub fn background_command(&mut self, context: &egui::Context) -> Option<Command> {
        if self.task.needs_refresh() {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.task.next(context.input(|input| input.time))
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        egui::Panel::top("decompile_header")
            .frame(palette::chrome_frame())
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("DECOMPILER")
                            .strong()
                            .color(palette::ACCENT),
                    );
                    if controls::action(ui, "Edit bytecode", Density::Regular).clicked() {
                        self.form.open = !self.form.open;
                    }
                    controls::toggle(
                        ui,
                        &mut self.inspector,
                        "Transform inspector",
                        Density::Regular,
                    );
                    if self.artifact.is_some() {
                        if controls::action_enabled(
                            ui,
                            self.state.can_back(),
                            "Back",
                            Density::Compact,
                        )
                        .clicked()
                        {
                            self.state
                                .history(self.artifact.as_ref().expect("artifact present"), false);
                        }
                        if controls::action_enabled(
                            ui,
                            self.state.can_forward(),
                            "Forward",
                            Density::Compact,
                        )
                        .clicked()
                        {
                            self.state
                                .history(self.artifact.as_ref().expect("artifact present"), true);
                        }
                    }
                });
            });
        let request = egui::Panel::top("decompile_input_panel")
            .frame(palette::chrome_frame())
            .show(ui, |ui| self.form.show(ui, self.task.busy()))
            .inner;
        let mut command = request.map(|request| {
            self.form.open = false;
            self.task.start(request)
        });
        egui::Panel::bottom("decompile_status")
            .frame(palette::chrome_frame())
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.task.busy() {
                        ui.spinner();
                    }
                    ui.label(self.task.label());
                    if self.task.busy()
                        && controls::action(ui, "Cancel decompile", Density::Regular).clicked()
                    {
                        self.task.request_cancel();
                    }
                    if let Some(snapshot) = &self.task.snapshot {
                        ui.label(format!(
                            "{:?} · {} blocks · {} operations · {} work",
                            snapshot.progress.phase,
                            snapshot.progress.blocks,
                            snapshot.progress.operations,
                            snapshot.progress.work
                        ));
                    }
                    if let Some(artifact) = &self.artifact {
                        if self.previous_artifact() {
                            ui.colored_label(palette::WARNING, "Previous artifact");
                        }
                        ui.label(format!(
                            "Artifact: {:?} · {:?} · {:?}",
                            artifact.completion, artifact.input.fork, artifact.config.strategy
                        ));
                    }
                });
                if let Some(error) = &self.task.error {
                    ui.colored_label(palette::ERROR, error.to_string());
                }
            });
        if let Some(artifact) = &self.artifact {
            egui::Panel::top("decompile_stage_controls")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.views.controls(ui, artifact, &mut self.state));
            let wide = ui.available_width() >= 1000.0;
            if self.inspector {
                let panel = if wide {
                    egui::Panel::right("decompile_inspector")
                        .resizable(true)
                        .default_size(340.0)
                        .min_size(260.0)
                        .max_size(520.0)
                } else {
                    let height = (ui.available_height() * 0.42).clamp(0.0, 280.0);
                    egui::Panel::bottom("decompile_inspector_compact")
                        .resizable(true)
                        .default_size(height)
                        .max_size(height)
                        .min_size(height.min(80.0))
                };
                let panel_response = panel
                    .frame(palette::chrome_frame())
                    .show(ui, |ui| inspector::show(ui, artifact, &mut self.state));
                controls::observe(
                    ui,
                    &panel_response.response,
                    "Decompiler inspector",
                    "decompile-inspector",
                    None,
                );
            }
            self.views.show(ui, artifact, &mut self.state);
        } else {
            crate::widgets::empty(
                ui,
                if self.task.busy() {
                    "Recovering local CFG and building traceable IR…"
                } else {
                    "Enter runtime bytecode, choose a fork and decompile to explore structured code, stage graphs, IR and transformation origins."
                },
            );
        }
        if command.is_none() {
            command = self.task.next(ui.input(|input| input.time));
        }
        if self.task.needs_refresh() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        command
    }

    pub fn receive(&mut self, message: Message) {
        match message {
            Message::Status {
                generation,
                operation,
                result,
            } => self.task.receive_status(generation, operation, *result),
            Message::Result {
                generation,
                id,
                result,
            } => {
                if !self.task.accepts_result(generation, id) {
                    return;
                }
                let result = (*result)
                    .map_err(task::Error::Transport)
                    .and_then(|reply| reply.result.map_err(task::Error::Api));
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        self.task.result_failed(error);
                        return;
                    }
                };
                if result.id != id {
                    self.task.reject(
                        result.publication,
                        task::Error::Identity {
                            expected: id,
                            received: result.id,
                        },
                    );
                    return;
                }
                if !self.task.accept_publication(result.publication) {
                    self.task.result_failed(task::Error::Publication);
                    return;
                }
                if let Err(error) = model::check_artifact(&result.artifact) {
                    self.task
                        .reject(result.publication, task::Error::Artifact(error));
                    return;
                }
                if self.artifact.is_some()
                    && result.artifact.current().is_none()
                    && matches!(
                        result.artifact.completion,
                        evm_abstract_protocol::decompile_model::Completion::Failed
                            | evm_abstract_protocol::decompile_model::Completion::Cancelled
                            | evm_abstract_protocol::decompile_model::Completion::Limited
                    )
                {
                    let message = result
                        .artifact
                        .diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    self.task.reject(
                        result.publication,
                        task::Error::UnvalidatedAttempt {
                            completion: result.artifact.completion,
                            message,
                        },
                    );
                    return;
                }
                self.state.install(
                    &result.artifact,
                    model::Publication {
                        job: id,
                        version: result.publication,
                    },
                );
                self.artifact = Some(result.artifact);
                self.task.delivered(result.publication);
            }
        }
    }

    pub fn accessible_status(&self) -> String {
        let mut status = format!(
            "Workspace: Decompiler; Decompile task: {}",
            self.task.label()
        );
        if let Some(snapshot) = &self.task.snapshot {
            status.push_str(&format!(
                "; Decompile progress: {:?}; Blocks: {}; Operations: {}; Work: {}",
                snapshot.progress.phase,
                snapshot.progress.blocks,
                snapshot.progress.operations,
                snapshot.progress.work
            ));
        }
        if let Some(error) = &self.task.error {
            status.push_str(&format!("; Error: {error}"));
        }
        if let Some(artifact) = &self.artifact {
            if let Some(publication) = self.state.publication {
                status.push_str(&format!(
                    "; Artifact job: {}; Publication: {}; Previous artifact: {}",
                    publication.job,
                    publication.version.0,
                    self.previous_artifact()
                ));
            }
            status.push_str(&format!("; Artifact: {}; Completion: {:?}; Fork: {:?}; Strategy: {:?}; Stages: {}; Route: {}", artifact.id.0, artifact.completion, artifact.input.fork, artifact.config.strategy, artifact.stages.len(), self.views.route_label()));
            if let Some(stage) = self.state.current.and_then(|id| artifact.stage(id)) {
                status.push_str(&format!(
                    "; Current stage: {} {:?}; Validation: {:?}",
                    stage.id.0, stage.kind, stage.validation.status
                ));
                status.push_str(&if stage.kind
                    == evm_abstract_protocol::decompile_model::StageKind::Decoded
                {
                    "; Fact scope: not recovered (Decoded)".into()
                } else {
                    format!("; Fact scope: {:?}", stage.ir.fact_scope)
                });
            }
            if let Some(stage) = self.state.before.and_then(|id| artifact.stage(id)) {
                status.push_str(&format!("; Before stage: {} {:?}", stage.id.0, stage.kind));
            }
            if let Some(selected) = &self.state.selected {
                status.push_str(&format!(
                    "; Selected entity: {}/stage:{}/entity:{}",
                    selected.artifact.0, selected.at.stage.0, selected.at.entity.0
                ));
            }
            if let Some(pc) = self.state.source_pc {
                status.push_str(&format!("; Original PC: 0x{pc:04x}"));
                if let Some(count) = self.state.source_descendant_count(pc) {
                    status.push_str(&format!("; Descendants: {count}"));
                }
            }
            if let Some(event) = self.state.event {
                status.push_str(&format!("; Transform event: {event}"));
            }
            if self.state.before.is_some() && self.state.current.is_some() {
                status.push_str(&format!(
                    "; Differences: {}",
                    self.views.differences.summary()
                ));
            }
            status.push_str(&format!("; Diagnostics: {}", artifact.diagnostics.len()));
        }
        status.push_str(&format!(
            "; Transform inspector: {}",
            if self.inspector { "open" } else { "closed" }
        ));
        let input = self.form.status();
        if !input.is_empty() {
            status.push_str("; ");
            status.push_str(&input);
        }
        status
    }
}

#[cfg(test)]
mod tests;
