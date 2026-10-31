//! Typed task state, immutable reports and compact world-view composition.

mod chrome;
mod directory;
mod form;
mod hybrid;
mod inspector;
mod job;
mod layout;
pub(crate) mod minimum;
pub(crate) mod model;
mod providers;
mod startup;
mod terminal_coverage;
pub(crate) mod timing;
mod workbench;

use crate::{palette, widgets};
use directory::Directory;
use egui::{Key, Modifiers, Ui};
use evm_abstract_notation::{Symbol, WideSymbol};
use evm_abstract_protocol::{
    AnalysisNamespace, AnalysisReport, AnalyzeReply, AnalyzeRequest, JobId, JobPublication,
    JobResultReply, SCHEMA_VERSION,
};
use form::AnalysisForm;
use inspector::Inspector;
#[cfg(target_arch = "wasm32")]
pub(crate) use inspector::shortcut::{
    allowed as inspector_shortcut_allowed, matches as inspector_chord,
};
pub use job::{Command, JobOperation, Message};
use job::{Task, TaskError, TaskPhase};
use layout::PaneLayout;

/// Browser request failures, distinct from the backend's typed analysis errors.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Request serialization failed.
    #[error("Could not encode the request: {0}")]
    Encode(serde_json::Error),
    /// Response did not follow the shared protocol.
    #[error("Invalid response (HTTP {status}): {cause}")]
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
    #[error("Cannot reach the server: {message}")]
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
    #[error("Server returned HTTP {status}")]
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
    pub value: Option<usize>,
}

impl Selection {
    pub(crate) fn same_native(self, other: Self) -> bool {
        self.state == other.state && self.pc == other.pc
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    Split,
    Disassembly,
    #[default]
    Graph,
    Ssa,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Split => "Compare",
            Self::Disassembly => "Code",
            Self::Graph => "CFG",
            Self::Ssa => "SSA",
        }
    }
}

/// Browser-independent workbench. Reports remain immutable while a new native
/// task runs; stale callbacks cannot replace the selected result.
pub struct Workspace {
    report: Option<AnalysisReport>,
    report_view: model::ReportViewState,
    minimum_selection: minimum::Selection,
    source_cache: widgets::SourceCache,
    navigation_error: Option<model::navigation::NavigationError>,
    original_inspection: Option<workbench::OriginalInspection>,
    // Only a validated report from an accepted server result owns this ID.
    // It remains bound to the visible report across later task generations.
    report_job_id: Option<JobId>,
    task: Task,
    startup: startup::Startup,
    form: AnalysisForm,
    directory: Directory,
    inspector: Inspector,
    panels: chrome::PanelLayout,
    previous_report: bool,
    pub(crate) selection: Selection,
    pub(crate) view: View,
    program: Option<usize>,
    account: Option<String>,
    previous_state: Option<usize>,
    pub(crate) graph: widgets::Graph,
    layout: PaneLayout,
    ssa_focus: Selection,
    ssa_cache: widgets::SsaCache,
    ssa_display: crate::ssa_display::SsaDisplay,
    report_timing: Option<timing::ReportTiming>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            report: None,
            report_view: model::ReportViewState::default(),
            minimum_selection: minimum::Selection::default(),
            source_cache: widgets::SourceCache::default(),
            navigation_error: None,
            original_inspection: None,
            report_job_id: None,
            task: Task::default(),
            startup: startup::Startup::default(),
            form: AnalysisForm::default(),
            directory: Directory::default(),
            inspector: Inspector::default(),
            panels: chrome::PanelLayout::default(),
            previous_report: false,
            selection: Selection::default(),
            view: View::Graph,
            program: None,
            account: None,
            previous_state: None,
            graph: widgets::Graph::default(),
            layout: PaneLayout::default(),
            ssa_focus: Selection::default(),
            ssa_cache: widgets::SsaCache::default(),
            ssa_display: crate::ssa_display::SsaDisplay::default(),
            report_timing: None,
        }
    }
}

impl Workspace {
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn input_open(&self) -> bool {
        self.form.open
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn background_command(&mut self, context: &egui::Context) -> Option<Command> {
        if self.task.needs_refresh() {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.task.next_command(context.input(|input| input.time))
    }

    /// Transfer only actual captured runtime bytes and their decoding fork.
    /// The receiving workspace edits its own draft before explicit submission.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn captured_decompile_input(&self) -> Option<(&str, evm_abstract_protocol::Fork)> {
        let report = self.report.as_ref()?;
        let program = match self.report_view.navigator().current() {
            Some(location) => location.focus.source.as_ref()?.program()?,
            None => self.program?,
        };
        report
            .programs
            .iter()
            .find(|source| {
                source.id == program
                    && matches!(
                        source.kind,
                        evm_abstract_protocol::CodeKind::Runtime
                            | evm_abstract_protocol::CodeKind::Empty
                    )
            })
            .map(|source| (source.bytecode.as_str(), source.fork))
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn inspector_shortcut_available(&self) -> bool {
        self.report.is_some() && !self.form.open
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn toggle_inspector(&mut self) {
        self.inspector.expanded = !self.inspector.expanded;
        if self.inspector.expanded && self.panels.compact() {
            self.directory.open = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn select_source_ssa(&mut self) {
        self.ssa_cache.select_source_scope();
    }

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
        let viewport = ui.available_size();
        if self.report.is_some() {
            self.panels.prepare(
                viewport,
                self.report_view.identity(),
                self.view == View::Graph,
                &mut self.directory.open,
                &mut self.inspector.expanded,
            );
        }
        let prepare_started = timing::now();
        let preparing_report = self
            .report_timing
            .as_ref()
            .is_some_and(|timing| timing.pending);
        let mut view_shown = false;
        let mut view_ready = true;
        self.inspector.timings = self.timing_details();
        let mut command = None;
        // Web keyboard ownership is decided synchronously by the DOM bridge,
        // before the browser can open its downloads page. Native input uses
        // the same exact chord and focus guards directly in egui.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let expanded = self.inspector.expanded;
            self.inspector.handle_shortcut(
                ui.ctx(),
                self.report.is_some() && !self.form.open && ui.is_enabled(),
            );
            if !expanded && self.inspector.expanded && self.panels.compact() {
                self.directory.open = false;
            }
        }
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
        let modal_open = self.form.open;
        ui.add_enabled_ui(!modal_open, |ui| {
            egui::Panel::top("workspace_header")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.header(ui));
            let available_bottom = ui.available_rect_before_wrap().bottom();
            let status_response = egui::Panel::bottom("workspace_status")
                .frame(palette::chrome_frame())
                .show(ui, |ui| {
                    command = self.status(ui);
                });
            // A cached status height can leave the graph's first-fit viewport stale.
            if (status_response.response.rect.bottom() - available_bottom).abs() > 1.0 {
                ui.ctx().request_discard("status panel size changed");
            }
            self.sync_selection();
            let preparation = self.show_workbench(ui, viewport);
            view_shown = preparation.shown;
            view_ready = preparation.ready;
        });
        if self.form.open {
            self.startup = startup::Startup::Finished;
        }
        if let Some(request) = self.form.show(ui.ctx(), self.task.busy()) {
            command = Some(self.start(request));
        }
        self.sync_selection();
        if preparing_report && let Some(timing) = &mut self.report_timing {
            let finished = timing::now();
            let ready = view_shown && view_ready && !ui.ctx().will_discard();
            timing.record_view(
                timing::elapsed_ns(prepare_started, finished),
                ready,
                finished,
            );
            if ready {
                if timing.generation == self.task.generation() {
                    self.task.wait.finish(finished);
                }
                // This pass painted the live label before view preparation.
                ui.ctx().request_repaint();
            }
        }
        if self.task.needs_refresh()
            || self
                .report_timing
                .as_ref()
                .is_some_and(|timing| timing.pending)
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        command
            .or_else(|| self.pending_startup_command())
            .or_else(|| self.task.next_command(ui.input(|input| input.time)))
    }

    fn sync_selection(&mut self) {
        if self.report_view.identity().is_minimum() {
            return;
        }
        self.inspector.select_value(self.selection.value);
        if self.selection.state == self.previous_state {
            return;
        }
        self.previous_state = self.selection.state;
        if let Some(report) = &self.report
            && let Some(state) = self
                .report_view
                .shown(report)
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
            } => self.task.receive_status(generation, operation, *result),
            Message::Result {
                generation,
                id,
                result,
                transfer_ns,
                parsing_ns,
            } => {
                if !self.task.accepts_result(generation, id) {
                    return;
                }
                self.task.frontend.transfer_ns =
                    self.task.frontend.transfer_ns.saturating_add(transfer_ns);
                self.task.frontend.parsing_ns =
                    self.task.frontend.parsing_ns.saturating_add(parsing_ns);
                match *result {
                    Err(error) => self.task.retry_result(TaskError::Transport(error)),
                    Ok(JobResultReply { result: Err(error) })
                        if matches!(
                            error.code,
                            evm_abstract_protocol::ApiErrorCode::QueueFull
                                | evm_abstract_protocol::ApiErrorCode::Transport
                                | evm_abstract_protocol::ApiErrorCode::TaskNotReady
                        ) =>
                    {
                        self.task.retry_result(TaskError::Api(error))
                    }
                    Ok(JobResultReply { result: Err(error) }) => {
                        self.task.reject_result(None, TaskError::Api(error))
                    }
                    Ok(JobResultReply { result: Ok(result) }) => {
                        if result.id != id {
                            self.task.retry_result(TaskError::Identity {
                                expected: id,
                                received: result.id,
                            });
                            return;
                        }
                        if !self.task.accepts_publication(&result.publication) {
                            self.task.discard_result();
                            return;
                        }
                        let publication = &result.publication;
                        if (result.report.minimum.is_some()
                            != publication.minimum_version.is_some())
                            || (!self.previous_report
                                && self.report_job_id == Some(id)
                                && self
                                    .report
                                    .as_ref()
                                    .is_some_and(|old| old.minimum.is_some())
                                && result.report.minimum.is_none())
                            || publication.model_closed
                                != (result.report.status
                                    == evm_abstract_protocol::AnalysisStatus::Converged)
                            || result.report.global.is_some()
                                != publication.global_version.is_some()
                            || result.report.native.is_some()
                                != publication.native_version.is_some()
                            || [
                                publication.minimum_version,
                                publication.global_version,
                                publication.native_version,
                            ]
                            .into_iter()
                            .flatten()
                            .any(|version| version > publication.version)
                            || self
                                .task
                                .advertised_publication()
                                .is_some_and(|advertised| {
                                    advertised.version == publication.version
                                        && advertised != publication
                                })
                        {
                            self.task
                                .reject_result(Some(publication.version), TaskError::Publication);
                            return;
                        }
                        self.install_report(result.report, Some(id), Some(result.publication));
                    }
                }
            }
        }
    }

    /// Apply a typed immutable result without a server Job ID association.
    /// Direct replacement also retires any outstanding result callback.
    pub fn receive(&mut self, result: Result<AnalyzeReply, TransportError>) {
        match result {
            Ok(AnalyzeReply { result: Ok(report) }) => self.install_report(report, None, None),
            Ok(AnalyzeReply { result: Err(error) }) => self.task.fail(TaskError::Api(error)),
            Err(error) => self.task.fail(TaskError::Transport(error)),
        }
    }

    fn install_report(
        &mut self,
        report: AnalysisReport,
        report_job_id: Option<JobId>,
        publication: Option<JobPublication>,
    ) {
        let prepare_started = timing::now();
        if report.schema_version != SCHEMA_VERSION {
            self.task.reject_result(
                publication.as_ref().map(|publication| publication.version),
                TaskError::Schema {
                    received: report.schema_version,
                },
            );
            return;
        }
        if let Err(failure) = report.validate() {
            self.task.reject_result(
                publication.as_ref().map(|publication| publication.version),
                TaskError::Api(evm_abstract_protocol::ApiError {
                    code: evm_abstract_protocol::ApiErrorCode::WorkerFailed,
                    message: failure.to_string(),
                    details: evm_abstract_protocol::ErrorDetails::Report(failure),
                }),
            );
            return;
        }
        let replacement =
            report_job_id.is_none() || self.previous_report || self.report_job_id != report_job_id;
        self.report = Some(report);
        self.activate_view(model::Activation::Report {
            replacement,
            publication: publication.clone(),
            job_id: report_job_id,
        });
        self.previous_report = false;
        self.report_job_id = report_job_id;
        if let Some(publication) = publication {
            self.task.published(publication);
        } else {
            self.task.complete();
        }
        if replacement {
            let mut frontend = self.task.frontend.clone();
            frontend.view_ns = timing::elapsed_ns(prepare_started, timing::now());
            self.report_timing = Some(timing::ReportTiming {
                generation: self.task.generation(),
                wait: self.task.wait.clone(),
                frontend,
                pending: true,
            });
        }
    }

    /// Live status for assistive tools and real browser verification.
    pub fn accessible_status(&self) -> String {
        let mut status = format!(
            "{}; Route: {}; Input: {}; Sources: {}; Inspector panel: {}",
            self.accessible_task_status(),
            self.view.label(),
            if self.form.open { "open" } else { "closed" },
            if self.form.open && self.directory.open {
                "suspended during input"
            } else if self.directory.open && self.inspector.maximized() {
                "suspended during maximized inspection"
            } else if self.directory.open && self.panels.compact() && self.inspector.expanded {
                "suspended during compact inspection"
            } else if self.directory.open {
                "open"
            } else {
                "closed"
            },
            if self.form.open && self.inspector.expanded {
                "suspended during input"
            } else if self.inspector.expanded {
                "open"
            } else {
                "closed"
            },
        );
        if let Some(location) = self.report_view.navigator().current() {
            use model::navigation::EvidenceFocus;
            status.push_str(match location.focus.evidence {
                EvidenceFocus::Source { .. } => "; Evidence: decoded source",
                EvidenceFocus::Minimum(_) => "; Evidence: MAY control",
                EvidenceFocus::Graph(_) => "; Evidence: graph state",
            });
        }
        let timing = self.accessible_timing();
        if timing.is_empty() {
            status
        } else {
            format!("{status}; {timing}")
        }
    }

    fn accessible_task_status(&self) -> String {
        let mut status = self.task_status_summary();
        for (label, id) in self.job_identities() {
            status.push_str(&format!("; {label}: {}", id.0));
        }
        if self.report.is_some() && self.report_job_id.is_none() {
            status.push_str("; Result Job ID: unavailable");
        }
        if let Some(publication) = self.report_view.publication() {
            let version = |value: Option<evm_abstract_protocol::PublicationVersion>| {
                value.map_or("unavailable".into(), |version| version.0.to_string())
            };
            status.push_str(&format!(
                "; Publication: {}; Minimum model: {}; Global graph: {}; Native graph: {}; Closed model: {}",
                publication.version.0,
                version(publication.minimum_version),
                version(publication.global_version),
                version(publication.native_version),
                publication.model_closed
            ));
        }
        if let Some(report) = &self.report
            && let Some(coverage) = terminal_coverage::status(report)
        {
            status.push_str(&format!(
                "; {}; {coverage}",
                terminal_coverage::detail_status(report)
            ));
        }
        if let Some(summary) = self.report.as_ref().and_then(hybrid::summary) {
            status.push_str(&format!("; {summary}"));
            status.push_str(match self.report_view.namespace() {
                AnalysisNamespace::Baseline => "; View: Global model",
                AnalysisNamespace::Native => "; View: Detailed paths",
            });
            if self.report_view.namespace() == AnalysisNamespace::Native
                && let Some(hybrid) = self
                    .report
                    .as_ref()
                    .and_then(|report| report.hybrid.as_ref())
                && let evm_abstract_protocol::HybridNativeBinding::SymbolicOwnerNamespace(binding) =
                    &hybrid.native_coverage().detail_binding
            {
                status.push_str(&format!(
                    "; Additional condition: root ADDRESS = {}",
                    binding.namespace
                ));
            }
        }
        status
    }

    fn task_status_summary(&self) -> String {
        if self.startup.loading() {
            return "Loading the initial example…".into();
        }
        if let Some(error) = &self.task.error
            && self.task.phase == TaskPhase::Failed
        {
            return format!("Error: {error}");
        }
        if !self.previous_report
            && let Some(report) = &self.report
        {
            if let Some(minimum) = self.report_view.minimum(report) {
                return self.minimum_status(report, minimum);
            }
            let report = &self.report_view.shown(report);
            let inspection = if self
                .original_inspection
                .as_ref()
                .is_some_and(|subject| subject.view == self.report_view.identity())
            {
                format!(
                    "Inspector: {} · Original observations",
                    self.inspector.tab.label()
                )
            } else if self
                .report_view
                .navigator()
                .current()
                .is_some_and(|location| location.focus.context.is_none())
            {
                format!("Inspector: {} · Source only", self.inspector.tab.label())
            } else {
                self.inspector.accessible_summary(report, self.selection)
            };
            return format!(
                "{}: {} instructions, {} CFG states, {} SSA blocks, {} SSA values; {:?}; SSA {}; CFG nodes: {}; {} programs, {} accounts, {} outcomes; Selected {}; source {}; {}; {}; SSA details: {:?}",
                self.task_label(),
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
                    .map_or("none".into(), |state| Symbol::State(state).to_string()),
                self.program
                    .map_or("none".into(), |program| Symbol::Program(program)
                        .to_string()),
                inspection,
                self.graph.accessible_summary(),
                self.ssa_display
            );
        }
        let progress = self
            .task
            .snapshot
            .as_ref()
            .map(|snapshot| {
                format!(
                    " · {} · {:?} · {} accounts · {} states · {} transfers",
                    WideSymbol::Task(snapshot.id.0),
                    snapshot.progress.phase,
                    snapshot.progress.accounts,
                    snapshot.progress.states,
                    snapshot.progress.transfers
                )
            })
            .unwrap_or_default();
        format!(
            "{}{progress}{}",
            self.task_label(),
            if self.previous_report {
                " · showing previous result"
            } else {
                ""
            }
        )
    }

    fn timing_details(&self) -> timing::Details {
        timing::Details {
            task_wait: self.task.wait.summary(timing::now()),
            task_label: self.task_label(),
            task_backend: self
                .task
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.timings.clone()),
            report: self.report_timing.clone(),
            previous: self.previous_report,
        }
    }

    fn preparing_current_view(&self) -> bool {
        self.task.phase == TaskPhase::Ready
            && self
                .report_timing
                .as_ref()
                .is_some_and(|timing| timing.pending && timing.generation == self.task.generation())
    }

    fn task_label(&self) -> &'static str {
        if self.preparing_current_view() {
            "Preparing result view"
        } else {
            self.task.label()
        }
    }

    fn accessible_timing(&self) -> String {
        let mut parts = Vec::new();
        if let Some(wait) = self.task.wait.summary(timing::now()) {
            parts.push(wait);
        }
        if self.previous_report {
            if let Some(snapshot) = &self.task.snapshot {
                parts.push(format!(
                    "Current task {}",
                    timing::backend_summary(&snapshot.timings)
                ));
            }
            parts.push("Previous report timing".into());
        }
        if let Some(report) = &self.report {
            parts.push(timing::backend_summary(&report.timings));
            if let Some(timing) = &self.report_timing {
                if self.previous_report
                    && let Some(wait) = timing.wait.summary(timing::now())
                {
                    parts.push(format!("Previous report {wait}"));
                }
                parts.push(timing.frontend.summary());
                if timing.pending {
                    parts.push("Preparing the first visible result view".into());
                }
            }
        } else if let Some(snapshot) = &self.task.snapshot {
            parts.push(timing::backend_summary(&snapshot.timings));
        }
        parts.join("; ")
    }
}
