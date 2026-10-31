//! Same application, independent workspace models and task domains. The shell
//! preserves each workspace's input, immutable result, selection and camera.

use crate::{
    Workspace,
    controls::{self, Density},
    decompile, palette,
};

pub(crate) enum Command {
    Analysis(crate::Command),
    Decompile(decompile::Command),
}

#[cfg(test)]
mod tests;

pub(crate) enum Message {
    Analysis(crate::Message),
    Decompile(decompile::Message),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Analysis,
    Decompiler,
}

#[derive(Default)]
pub(crate) struct Application {
    analysis: Workspace,
    decompile: decompile::Workspace,
    mode: Mode,
}

impl Application {
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let input = self.analysis.captured_decompile_input();
        let modal = self.mode == Mode::Analysis && self.analysis.input_open();
        egui::Panel::top("application_workspace")
            .frame(palette::chrome_frame())
            .show(ui, |ui| {
                ui.add_enabled_ui(!modal, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let density = if ui.available_width() < 680.0 {
                            Density::Touch
                        } else {
                            Density::Compact
                        };
                        controls::tab_value(
                            ui,
                            &mut self.mode,
                            Mode::Analysis,
                            "Analysis",
                            density,
                        );
                        controls::tab_value(
                            ui,
                            &mut self.mode,
                            Mode::Decompiler,
                            "Decompiler",
                            density,
                        );
                        if self.mode == Mode::Analysis
                            && controls::action_enabled(
                                ui,
                                input.is_some(),
                                "Decompile captured source",
                                density,
                            )
                            .clicked()
                            && let Some((bytecode, fork)) = input
                        {
                            self.decompile.load_input(bytecode.to_owned(), fork);
                            self.mode = Mode::Decompiler;
                        }
                    });
                });
            });
        ui.push_id(
            match self.mode {
                Mode::Analysis => "analysis_workspace",
                Mode::Decompiler => "decompiler_workspace",
            },
            |ui| match self.mode {
                Mode::Analysis => self.analysis.show(ui).map(Command::Analysis),
                Mode::Decompiler => self.decompile.show(ui).map(Command::Decompile),
            },
        )
        .inner
    }

    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        match self.mode {
            Mode::Analysis => self.analysis.prepare_input(input),
            Mode::Decompiler => self.decompile.prepare_input(input),
        }
    }

    pub fn receive_message(&mut self, message: Message) {
        match message {
            Message::Analysis(message) => self.analysis.receive_message(message),
            Message::Decompile(message) => self.decompile.receive(message),
        }
    }

    pub fn startup_command(&mut self) -> Option<Command> {
        (self.mode == Mode::Analysis)
            .then(|| self.analysis.startup_command())
            .flatten()
            .map(Command::Analysis)
    }

    pub fn rpc_provider_command(&mut self) -> Option<Command> {
        self.analysis.rpc_provider_command().map(Command::Analysis)
    }

    pub fn background_command(&mut self, context: &egui::Context) -> Option<Command> {
        match self.mode {
            Mode::Analysis => self
                .decompile
                .background_command(context)
                .map(Command::Decompile),
            Mode::Decompiler => self
                .analysis
                .background_command(context)
                .map(Command::Analysis),
        }
    }

    pub fn inspector_shortcut_available(&self) -> bool {
        match self.mode {
            Mode::Analysis => self.analysis.inspector_shortcut_available(),
            Mode::Decompiler => self.decompile.inspector_shortcut_available(),
        }
    }

    pub fn toggle_inspector(&mut self) {
        match self.mode {
            Mode::Analysis => self.analysis.toggle_inspector(),
            Mode::Decompiler => self.decompile.toggle_inspector(),
        }
    }

    pub fn accessible_status(&self) -> String {
        match self.mode {
            Mode::Analysis => format!("{}; Workspace: Analysis", self.analysis.accessible_status()),
            Mode::Decompiler => self.decompile.accessible_status(),
        }
    }

    pub fn accessible_rpc_provider_status(&self) -> String {
        self.analysis.accessible_rpc_provider_status()
    }

    pub fn accessible_input_status(&self) -> Option<String> {
        match self.mode {
            Mode::Analysis => self.analysis.accessible_input_status(),
            Mode::Decompiler => None,
        }
    }
}
