//! Typed workspace state and view composition, shared by browser and host tests.

mod chrome;
mod layout;

use egui::{Key, Modifiers, Ui};
use evm_abstract_protocol::{
    AnalysisLimits, AnalysisReport, AnalyzeReply, AnalyzeRequest, SCHEMA_VERSION,
};

use crate::{palette, widgets};

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

#[derive(Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// Pure egui application state. Analysis is sent and received as shared DTOs;
/// browser callbacks and engine implementation types never enter this layer.
pub struct Workspace {
    request: AnalyzeRequest,
    report: Option<AnalysisReport>,
    phase: Phase,
    pub(crate) selection: Selection,
    pub(crate) view: View,
    graph: widgets::Graph,
    layout: PaneLayout,
    disasm_focus: Selection,
    ssa_focus: Selection,
    show_input: bool,
    show_details: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            request: AnalyzeRequest {
                bytecode: BRANCH_EXAMPLE.into(),
                limits: AnalysisLimits {
                    context_depth: 0,
                    ..AnalysisLimits::default()
                },
                ..AnalyzeRequest::default()
            },
            report: None,
            phase: Phase::Idle,
            selection: Selection::default(),
            view: View::Split,
            graph: widgets::Graph::default(),
            layout: PaneLayout::default(),
            disasm_focus: Selection::default(),
            ssa_focus: Selection::default(),
            show_input: true,
            show_details: false,
        }
    }
}

impl Workspace {
    /// Paint the workspace and return a newly submitted, fully typed request.
    pub fn show(&mut self, ui: &mut Ui) -> Option<AnalyzeRequest> {
        let mut submit = false;
        if !ui.ctx().text_edit_focused() {
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
        submit |= ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Enter));
        let viewport_height = ui.available_height();
        egui::Panel::top("workspace_header")
            .frame(palette::chrome_frame())
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("workspace_status")
            .frame(palette::chrome_frame())
            .show(ui, |ui| self.status(ui, viewport_height));
        if self.show_input {
            egui::Panel::top("bytecode_input")
                .min_size(48.0)
                .frame(palette::chrome_frame())
                .show(ui, |ui| submit |= self.input(ui, viewport_height));
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
                    let message = match self.phase {
                        Phase::Loading => "Analyzing bytecode…",
                        Phase::Failed(_) => "Update the bytecode and select Analyze to try again.",
                        _ => "Paste runtime bytecode above, then select Analyze.",
                    };
                    widgets::empty(ui, message);
                }
            });
        (submit && self.phase != Phase::Loading).then(|| self.begin_analysis())
    }

    /// Clear the old snapshot and start a new request; input changes alone never
    /// relabel an existing result as if it were analyzed under different rules.
    pub fn begin_analysis(&mut self) -> AnalyzeRequest {
        self.report = None;
        self.phase = Phase::Loading;
        self.selection = Selection::default();
        self.disasm_focus = Selection::default();
        self.ssa_focus = Selection::default();
        self.graph.reset_report();
        self.request.clone()
    }

    /// Accept a typed transport result, including schema and backend failures.
    pub fn receive(&mut self, result: Result<AnalyzeReply, TransportError>) {
        self.report = None;
        match result {
            Ok(AnalyzeReply { result: Ok(report) }) if report.schema_version == SCHEMA_VERSION => {
                self.selection.state = report.cfg.first().map(|block| block.id);
                self.phase = Phase::Ready;
                self.report = Some(report);
            }
            Ok(AnalyzeReply { result: Ok(report) }) => {
                self.phase = Phase::Failed(format!(
                    "Unsupported response schema {}; this workspace expects {}",
                    report.schema_version, SCHEMA_VERSION
                ));
            }
            Ok(AnalyzeReply { result: Err(error) }) => {
                self.phase = Phase::Failed(format!("{:?}: {}", error.code, error.message))
            }
            Err(error) => self.phase = Phase::Failed(error.to_string()),
        }
    }

    /// Text equivalent of the rendered snapshot's completion and coverage state.
    pub fn accessible_status(&self) -> String {
        match (&self.phase, &self.report) {
            (Phase::Ready, Some(report)) => format!(
                "Ready: {} instructions, {} CFG blocks, {} SSA blocks, {} SSA values; {:?}; SSA {}; CFG nodes: {}",
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
                self.graph.content_label()
            ),
            (Phase::Failed(message), _) => format!("Error: {message}"),
            (Phase::Loading, _) => "Loading: analyzing bytecode".into(),
            _ => "Idle: enter bytecode to analyze".into(),
        }
    }
}
