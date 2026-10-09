//! Typed task state, immutable reports and compact world-view composition.

mod chrome;
mod directory;
mod form;
mod inspector;
mod job;
mod layout;
mod providers;
mod startup;

use crate::{palette, widgets};
use directory::{Directory, Pick};
use egui::{Key, Modifiers, Ui};
use evm_abstract_protocol::{AnalysisReport, AnalyzeReply, AnalyzeRequest, SCHEMA_VERSION};
use form::AnalysisForm;
use inspector::Inspector;
pub use job::{Command, JobOperation, Message};
use job::{Task, TaskError, TaskPhase};
use layout::{AnalysisPanes, Pane, PaneLayout};

/// Browser request failures, distinct from the backend's typed analysis errors.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Request serialization failed.
    #[error("Could not encode the analysis request: {0}")]
    Encode(serde_json::Error),
    /// Response did not follow the shared protocol.
    #[error("Invalid analysis response (HTTP {status}): {cause}")]
    Decode {
        /// HTTP response status.
        status: u16,
        /// JSON decoding failure.
        cause: serde_json::Error,
    },
    /// An individual HTTP exchange exceeded its deadline; native work may continue.
    #[error("HTTP request timed out after {seconds}s")]
    Timeout {
        /// Deadline for this HTTP exchange, separate from analysis limits.
        seconds: u32,
    },
    /// Browser fetch failed.
    #[error("Cannot reach the analysis server: {message}")]
    Browser {
        /// Browser-provided failure detail.
        message: String,
    },
    /// DOM is unavailable.
    #[error("Browser window is unavailable")]
    MissingWindow,
    /// Unexpected fetch body type.
    #[error("Server returned a non-text response")]
    NonTextResponse,
    /// HTTP failure with an inconsistent success envelope.
    #[error("Analysis server returned HTTP {status}")]
    Http {
        /// HTTP response status.
        status: u16,
    },
}

const BRANCH_EXAMPLE: &str = "5f35600a576001600d565b60025b60030100";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub state: Option<usize>,
    pub pc: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
    Split,
    Disassembly,
    Graph,
    Ssa,
}

/// Browser-independent workbench. Reports remain immutable while a new native
/// task runs; stale callbacks cannot replace the selected result.
pub struct Workspace {
    report: Option<AnalysisReport>,
    task: Task,
    startup: startup::Startup,
    form: AnalysisForm,
    directory: Directory,
    inspector: Inspector,
    previous_report: bool,
    pub(crate) selection: Selection,
    pub(crate) view: View,
    program: Option<usize>,
    account: Option<String>,
    previous_state: Option<usize>,
    graph: widgets::Graph,
    layout: PaneLayout,
    disasm_focus: Selection,
    ssa_focus: Selection,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            report: None,
            task: Task::default(),
            startup: startup::Startup::default(),
            form: AnalysisForm::default(),
            directory: Directory::default(),
            inspector: Inspector::default(),
            previous_report: false,
            selection: Selection::default(),
            view: View::Split,
            program: None,
            account: None,
            previous_state: None,
            graph: widgets::Graph::default(),
            layout: PaneLayout::default(),
            disasm_focus: Selection::default(),
            ssa_focus: Selection::default(),
        }
    }
}

impl Workspace {
    /// End a graph wheel gesture cancelled by Fit before new events are applied.
    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        if !self.form.open {
            self.graph.prepare_input(input);
        }
    }

    /// Explicitly submit the current input through the asynchronous task lifecycle.
    pub fn initial_command(&mut self) -> Command {
        self.start(self.form.request())
    }

    /// Fetch the RPC catalog independently from any current analysis task.
    pub fn rpc_provider_command(&mut self) -> Option<Command> {
        self.form.provider_command()
    }

    /// Provider availability and selection for assistive tools.
    pub fn accessible_rpc_provider_status(&self) -> String {
        self.form.provider_status()
    }

    fn start(&mut self, request: AnalyzeRequest) -> Command {
        self.startup = startup::Startup::Finished;
        self.previous_report = self.report.is_some();
        self.task.start(request)
    }

    /// Render the pure UI. The browser adapter executes the returned command.
    pub fn show(&mut self, ui: &mut Ui) -> Option<Command> {
        let mut command = None;
        if !self.form.open && !ui.ctx().text_edit_focused() && !egui::Popup::is_any_open(ui.ctx()) {
            for (key, view) in [
                (Key::Num0, View::Split),
                (Key::Num1, View::Disassembly),
                (Key::Num2, View::Graph),
                (Key::Num3, View::Ssa),
            ] {
                if ui.input_mut(|input| input.consume_key(Modifiers::NONE, key)) {
                    self.view = view;
                }
            }
        }
        if !self.form.open && ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Enter)) {
            self.form.open = true;
        }
        let viewport = ui.available_size();
        let modal_open = self.form.open;
        ui.add_enabled_ui(!modal_open, |ui| {
            egui::Panel::top("workspace_header")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.header(ui));
            egui::Panel::bottom("workspace_status")
                .frame(palette::chrome_frame())
                .show(ui, |ui| {
                    command = self.status(ui);
                });
            self.sync_selection();
            if let Some(report) = &self.report {
                egui::Panel::top("result_context")
                    .frame(palette::chrome_frame())
                    .show(ui, |ui| {
                        chrome::result_context(ui, report, self.program, self.previous_report);
                    });
                if self.inspector.expanded {
                    let maximum = (viewport.y * 0.38).max(80.0);
                    egui::Panel::bottom("world_inspector")
                        .resizable(true)
                        .default_size(210.0)
                        .min_size(80.0)
                        .max_size(maximum)
                        .frame(palette::chrome_frame())
                        .show(ui, |ui| {
                            // Reserve the chosen panel height even for an empty snapshot;
                            // otherwise egui shrinks it after the graph's initial fit.
                            ui.set_min_height(ui.available_height());
                            self.inspector.show(
                                ui,
                                report,
                                &mut self.selection,
                                self.account.as_deref(),
                            );
                        });
                }
            }
            let mut pick = None;
            if self.directory.open
                && viewport.x >= 1050.0
                && let Some(report) = &self.report
            {
                egui::Panel::left("world_directory")
                    .resizable(true)
                    .default_size(196.0)
                    .min_size(140.0)
                    .max_size(320.0)
                    .frame(palette::chrome_frame())
                    .show(ui, |ui| {
                        pick =
                            self.directory
                                .show(ui, report, self.program, self.account.as_deref());
                    });
            }
            if let Some(pick) = pick {
                self.pick(pick);
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(palette::BACKGROUND)
                        .inner_margin(4.0),
                )
                .show(ui, |ui| {
                    if let Some(report) = &self.report {
                        let mut panes = AnalysisPanes {
                            report,
                            selection: &mut self.selection,
                            program: self.program,
                            graph: &mut self.graph,
                            disasm_focus: &mut self.disasm_focus,
                            ssa_focus: &mut self.ssa_focus,
                        };
                        match self.view {
                            View::Split => self.layout.show(ui, &mut panes),
                            View::Disassembly => panes.show(ui, Pane::Disassembly),
                            View::Graph => panes.show(ui, Pane::Graph),
                            View::Ssa => panes.show(ui, Pane::Ssa),
                        }
                    } else {
                        widgets::empty(
                            ui,
                            if self.startup.loading() {
                                "Loading the initial example…"
                            } else if self.task.busy() {
                                "Analysis is running. Progress and cancellation are below."
                            } else {
                                "Select New analysis to load bytecode or an RPC account."
                            },
                        );
                    }
                });
        });
        if self.directory.open && viewport.x < 1050.0 && !self.form.open {
            let mut open = true;
            let mut pick = None;
            if let Some(report) = &self.report {
                egui::Window::new("Programs and accounts")
                    .id(egui::Id::new("directory_window"))
                    .open(&mut open)
                    .default_width((viewport.x - 30.0).min(360.0))
                    .max_height((viewport.y - 80.0).max(80.0))
                    .show(ui.ctx(), |ui| {
                        pick =
                            self.directory
                                .show(ui, report, self.program, self.account.as_deref())
                    });
            }
            self.directory.open = open;
            if let Some(pick) = pick {
                self.pick(pick);
                self.directory.open = false;
            }
        }
        if self.form.open {
            self.startup = startup::Startup::Finished;
        }
        if let Some(request) = self.form.show(ui.ctx(), self.task.busy()) {
            command = Some(self.start(request));
        }
        self.sync_selection();
        if self.task.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        command
            .or_else(|| self.pending_startup_command())
            .or_else(|| self.task.next_command(ui.input(|input| input.time)))
    }

    fn pick(&mut self, pick: Pick) {
        let Some(report) = &self.report else {
            return;
        };
        match pick {
            Pick::Program(program) => {
                self.program = Some(program);
                self.account = None;
                self.selection = Selection {
                    state: report
                        .cfg
                        .iter()
                        .find(|state| state.program == Some(program))
                        .map(|state| state.id),
                    pc: None,
                };
            }
            Pick::Account(address) => {
                self.selection = Selection {
                    state: report
                        .cfg
                        .iter()
                        .find(|state| state.storage_address == address)
                        .map(|state| state.id),
                    pc: None,
                };
                self.program = self
                    .selection
                    .state
                    .and_then(|id| {
                        report
                            .cfg
                            .iter()
                            .find(|state| state.id == id)
                            .and_then(|state| state.program)
                    })
                    .or_else(|| {
                        report
                            .accounts
                            .iter()
                            .find(|account| account.address == address)
                            .and_then(|account| account.program)
                    });
                self.account = Some(address);
                self.inspector.expanded = true;
                self.inspector.tab = inspector::Tab::Storage;
            }
        }
        self.previous_state = self.selection.state;
    }

    fn sync_selection(&mut self) {
        if self.selection.state == self.previous_state {
            return;
        }
        self.previous_state = self.selection.state;
        if let Some(report) = &self.report
            && let Some(state) = report
                .cfg
                .iter()
                .find(|state| Some(state.id) == self.selection.state)
        {
            self.program = state.program;
            self.account = None;
        }
    }

    /// Apply an asynchronous operation result, rejecting superseded tasks.
    pub fn receive_message(&mut self, message: Message) {
        match message {
            Message::RpcProviders { generation, result } => {
                if self.form.receive_providers(generation, result) {
                    self.startup.receive_catalog(generation);
                }
            }
            Message::Status {
                generation,
                operation,
                result,
            } => self.task.receive_status(generation, operation, result),
            Message::Result {
                generation,
                id,
                result,
            } => {
                if !self.task.accepts_result(generation, id) {
                    return;
                }
                match *result {
                    Err(error) => self.task.retry_result(TaskError::Transport(error)),
                    Ok(AnalyzeReply { result: Err(error) })
                        if matches!(
                            error.code,
                            evm_abstract_protocol::ApiErrorCode::QueueFull
                                | evm_abstract_protocol::ApiErrorCode::Transport
                                | evm_abstract_protocol::ApiErrorCode::TaskNotReady
                        ) =>
                    {
                        self.task.retry_result(TaskError::Api(error))
                    }
                    result => self.receive(result),
                }
            }
        }
    }

    /// Apply a typed immutable result; schema failures never enter view state.
    pub fn receive(&mut self, result: Result<AnalyzeReply, TransportError>) {
        match result {
            Ok(AnalyzeReply { result: Ok(report) }) if report.schema_version == SCHEMA_VERSION => {
                self.selection = Selection {
                    state: report.cfg.first().map(|state| state.id),
                    pc: None,
                };
                self.program = report
                    .cfg
                    .first()
                    .map_or(report.metadata.root_program, |state| state.program);
                self.previous_state = self.selection.state;
                self.account = None;
                self.disasm_focus = Selection::default();
                self.ssa_focus = Selection::default();
                self.graph.reset_report();
                self.inspector.reset_report();
                self.previous_report = false;
                self.directory.open =
                    report.metadata.snapshot.is_some() || report.programs.len() > 1;
                self.report = Some(report);
                self.task.complete();
            }
            Ok(AnalyzeReply { result: Ok(report) }) => self.task.fail(TaskError::Schema {
                received: report.schema_version,
            }),
            Ok(AnalyzeReply { result: Err(error) }) => self.task.fail(TaskError::Api(error)),
            Err(error) => self.task.fail(TaskError::Transport(error)),
        }
    }

    /// Live status for assistive tools and real browser verification.
    pub fn accessible_status(&self) -> String {
        if self.startup.loading() {
            return "Loading the initial example…".into();
        }
        if let Some(error) = &self.task.error
            && self.task.phase == TaskPhase::Failed
        {
            return format!("Error: {error}");
        }
        if self.task.phase == TaskPhase::Ready
            && let Some(report) = &self.report
        {
            return format!(
                "Ready: {} instructions, {} CFG blocks, {} SSA blocks, {} SSA values; {:?}; SSA {}; CFG nodes: {}; {} programs, {} accounts, {} outcomes; Selected {}; source {}; {}",
                report
                    .disassembly
                    .iter()
                    .map(|block| block.instructions.len())
                    .sum::<usize>(),
                report.cfg.len(),
                report.ssa.blocks.len(),
                report.ssa.value_count,
                report.status,
                if report.ssa.complete {
                    "complete"
                } else {
                    "partial"
                },
                self.graph.content_label(),
                report.programs.len(),
                report.accounts.len(),
                report.outcomes.len(),
                self.selection
                    .state
                    .map_or("none".into(), |state| format!("S{state}")),
                self.program
                    .map_or("none".into(), |program| format!("P{program}")),
                self.inspector.accessible_summary(report, self.selection)
            );
        }
        let progress = self
            .task
            .snapshot
            .as_ref()
            .map(|snapshot| {
                format!(
                    " · {:?} · {} accounts · {} states · {} transfers",
                    snapshot.progress.phase,
                    snapshot.progress.accounts,
                    snapshot.progress.states,
                    snapshot.progress.transfers
                )
            })
            .unwrap_or_default();
        format!(
            "{}{progress}{}",
            self.task.label(),
            if self.previous_report {
                " · showing previous result"
            } else {
                ""
            }
        )
    }
}
