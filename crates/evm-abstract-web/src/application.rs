//! URL-owned pages with lazily retained, independent workspace state.
//! An inactive page retains its view but never starts or polls background work.
use crate::{
    Workspace,
    controls::{self, Density},
    decompile, palette,
    routing::{Page, Route},
};

pub(crate) enum Command {
    Analysis(crate::Command),
    Decompile(decompile::Command),
}

pub(crate) enum Message {
    Analysis(crate::Message),
    Decompile(decompile::Message),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum History {
    Push,
    Replace,
}

pub(crate) struct Navigation {
    pub route: Route,
    pub history: History,
}

pub(crate) struct Application {
    analysis: Option<Workspace>,
    decompile: Option<decompile::Workspace>,
    route: Route,
    decompiler_route: Route,
    navigation: std::collections::VecDeque<Navigation>,
    navigation_error: Option<String>,
}

impl Default for Application {
    fn default() -> Self {
        Self::new(Route::default())
    }
}

impl Application {
    pub fn new(route: Route) -> Self {
        let mut app = Self {
            analysis: None,
            decompile: None,
            route: Route::default(),
            decompiler_route: Route::page(Page::Decompiler),
            navigation: std::collections::VecDeque::new(),
            navigation_error: None,
        };
        app.activate(route);
        app
    }

    /// Called by browser history as well as explicit workspace navigation.
    /// Keeping the same source resumes its retained task/view without reopening.
    pub fn activate(&mut self, route: Route) {
        self.navigation_error = None;
        match route.page {
            Page::Analysis => {
                self.analysis.get_or_insert_with(Workspace::default);
            }
            Page::Decompiler => {
                let fresh = self.decompile.is_none();
                let workspace = self
                    .decompile
                    .get_or_insert_with(decompile::Workspace::default);
                if fresh || route != self.decompiler_route {
                    workspace.open_route(&route);
                }
                self.decompiler_route = route.clone();
            }
        }
        self.route = route;
    }

    fn navigate(&mut self, route: Route) {
        self.activate(route.clone());
        self.navigation.push_back(Navigation {
            route,
            history: History::Push,
        });
    }

    pub fn take_navigation(&mut self) -> Option<Navigation> {
        self.navigation.pop_front()
    }

    /// Finish explicit cancellation even after the requesting page is hidden.
    pub fn cleanup_command(&mut self) -> Option<Command> {
        self.analysis
            .as_mut()
            .and_then(Workspace::cleanup_command)
            .map(Command::Analysis)
            .or_else(|| {
                self.decompile
                    .as_mut()?
                    .take_cleanup()
                    .map(Command::Decompile)
            })
    }

    pub fn navigation_failed(&mut self, error: String) {
        self.navigation_error = Some(error);
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let modal = self.route.page == Page::Analysis
            && self.analysis.as_ref().is_some_and(Workspace::input_open);
        let captured = self
            .analysis
            .as_ref()
            .is_some_and(|workspace| workspace.captured_decompile_input().is_some());
        let mut page = self.route.page;
        let mut capture = false;
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
                        controls::tab_value(ui, &mut page, Page::Analysis, "Analysis", density);
                        controls::tab_value(ui, &mut page, Page::Decompiler, "Decompiler", density);
                        if self.route.page == Page::Analysis {
                            capture = controls::action_enabled(
                                ui,
                                captured,
                                "Decompile captured source",
                                density,
                            )
                            .clicked();
                        }
                    });
                });
                if let Some(error) = &self.navigation_error {
                    ui.colored_label(palette::ERROR, error);
                }
            });
        if capture {
            let input = self
                .analysis
                .as_ref()
                .and_then(Workspace::captured_decompile_input)
                .map(|(bytes, fork)| (bytes.to_owned(), fork));
            if let Some((bytes, fork)) = input {
                self.navigate(Route::page(Page::Decompiler));
                self.decompile
                    .as_mut()
                    .expect("activated page")
                    .load_input(bytes, fork);
            }
        } else if page != self.route.page {
            let route = if page == Page::Decompiler {
                self.decompiler_route.clone()
            } else {
                Route::default()
            };
            self.navigate(route);
        }
        ui.push_id(
            match self.route.page {
                Page::Analysis => "analysis_workspace",
                Page::Decompiler => "decompiler_workspace",
            },
            |ui| match self.route.page {
                Page::Analysis => self
                    .analysis
                    .as_mut()
                    .expect("active page")
                    .show(ui)
                    .map(Command::Analysis),
                Page::Decompiler => self
                    .decompile
                    .as_mut()
                    .expect("active page")
                    .show(ui)
                    .map(Command::Decompile),
            },
        )
        .inner
    }

    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_mut()
                .expect("active page")
                .prepare_input(input),
            Page::Decompiler => self
                .decompile
                .as_mut()
                .expect("active page")
                .prepare_input(input),
        }
    }

    pub fn receive_message(&mut self, message: Message) {
        match message {
            Message::Analysis(message) => {
                if let Some(workspace) = &mut self.analysis {
                    workspace.receive_message(message);
                }
            }
            Message::Decompile(message) => {
                if let Some(workspace) = &mut self.decompile {
                    workspace.receive(message);
                    if let Some((route, history)) = workspace.take_location() {
                        self.decompiler_route = route.clone();
                        if self.route.page == Page::Decompiler {
                            self.route = route.clone();
                            self.navigation.push_back(Navigation { route, history });
                        }
                    }
                }
            }
        }
    }

    pub fn startup_command(&mut self) -> Option<Command> {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_mut()?
                .startup_command()
                .map(Command::Analysis),
            Page::Decompiler => None,
        }
    }

    pub fn rpc_provider_command(&mut self) -> Option<Command> {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_mut()?
                .rpc_provider_command()
                .map(Command::Analysis),
            Page::Decompiler => self
                .decompile
                .as_mut()?
                .provider_command()
                .map(Command::Decompile),
        }
    }

    pub fn inspector_shortcut_available(&self) -> bool {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_ref()
                .is_some_and(Workspace::inspector_shortcut_available),
            Page::Decompiler => self
                .decompile
                .as_ref()
                .is_some_and(decompile::Workspace::inspector_shortcut_available),
        }
    }

    pub fn toggle_inspector(&mut self) {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_mut()
                .expect("active page")
                .toggle_inspector(),
            Page::Decompiler => self
                .decompile
                .as_mut()
                .expect("active page")
                .toggle_inspector(),
        }
    }

    pub fn accessible_status(&self) -> String {
        let mut status = match self.route.page {
            Page::Analysis => format!(
                "{}; Workspace: Analysis",
                self.analysis
                    .as_ref()
                    .expect("active page")
                    .accessible_status()
            ),
            Page::Decompiler => self
                .decompile
                .as_ref()
                .expect("active page")
                .accessible_status(),
        };
        if let Some(error) = &self.navigation_error {
            status.push_str(&format!("; Navigation error: {error}"));
        }
        status
    }

    pub fn accessible_rpc_provider_status(&self) -> String {
        match self.route.page {
            Page::Analysis => self
                .analysis
                .as_ref()
                .expect("active page")
                .accessible_rpc_provider_status(),
            Page::Decompiler => String::new(),
        }
    }

    pub fn accessible_input_status(&self) -> Option<String> {
        match self.route.page {
            Page::Analysis => self.analysis.as_ref()?.accessible_input_status(),
            Page::Decompiler => None,
        }
    }
}

#[cfg(test)]
mod tests;
