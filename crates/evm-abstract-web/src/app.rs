//! Typed workspace state and view composition, shared by browser and host tests.

use egui::{Key, Modifiers, RichText, Ui};
use evm_abstract_protocol::{
    AnalysisLimits, AnalysisReport, AnalysisStatus, AnalyzeReply, AnalyzeRequest, Fork,
    SCHEMA_VERSION,
};

use crate::{palette, widgets};

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
        egui::Panel::top("workspace_header").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("◇  EVM ABSTRACT LAB")
                        .strong()
                        .size(18.0)
                        .color(palette::ACCENT),
                );
                ui.add_space(18.0);
                for (view, label) in [
                    (View::Split, "Workspace  0"),
                    (View::Disassembly, "Disassembly  1"),
                    (View::Graph, "Control flow  2"),
                    (View::Ssa, "SSA  3"),
                ] {
                    ui.selectable_value(&mut self.view, view, label);
                }
                ui.separator();
                ui.toggle_value(&mut self.show_input, "Bytecode");
            });
            ui.add_space(8.0);
        });
        egui::Panel::bottom("workspace_status").show(ui, |ui| {
            self.status(ui);
        });
        if self.show_input {
            egui::Panel::top("bytecode_input").show(ui, |ui| {
                submit |= self.input(ui);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(report) = &self.report {
                let narrow = ui.available_width() < 1050.0;
                if narrow && self.view == View::Split {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Compact view").color(palette::MUTED));
                        ui.selectable_value(&mut self.view, View::Disassembly, "Disassembly");
                        ui.selectable_value(&mut self.view, View::Ssa, "SSA");
                    });
                }
                match self.view {
                    View::Split if !narrow => {
                        egui::Panel::left("disassembly_panel")
                            .default_size(360.0)
                            .min_size(220.0)
                            .max_size(600.0)
                            .show(ui, |ui| {
                                widgets::disassembly(
                                    ui,
                                    report,
                                    &mut self.selection,
                                    &mut self.disasm_focus,
                                )
                            });
                        egui::Panel::right("ssa_panel")
                            .default_size(390.0)
                            .min_size(280.0)
                            .max_size(800.0)
                            .show(ui, |ui| {
                                widgets::ssa(ui, report, &mut self.selection, &mut self.ssa_focus)
                            });
                        self.graph.show(ui, report, &mut self.selection);
                    }
                    View::Split | View::Graph => self.graph.show(ui, report, &mut self.selection),
                    View::Disassembly => widgets::disassembly(
                        ui,
                        report,
                        &mut self.selection,
                        &mut self.disasm_focus,
                    ),
                    View::Ssa => widgets::ssa(ui, report, &mut self.selection, &mut self.ssa_focus),
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

    fn input(&mut self, ui: &mut Ui) -> bool {
        let mut submit = false;
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("RUNTIME BYTECODE")
                    .small()
                    .color(palette::MUTED),
            );
            egui::ComboBox::from_id_salt("fork")
                .selected_text(format!("{:?}", self.request.fork))
                .show_ui(ui, |ui| {
                    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
                        ui.selectable_value(&mut self.request.fork, fork, format!("{fork:?}"));
                    }
                });
            if ui.button("Branch example").clicked() {
                self.request.bytecode = BRANCH_EXAMPLE.into();
            }
            if ui.button("Arithmetic").clicked() {
                self.request.bytecode = "600160020160030200".into();
            }
            ui.menu_button("Analysis limits", |ui| {
                ui.label("Maximum states");
                ui.add(egui::DragValue::new(&mut self.request.limits.max_states).range(1..=4096));
                ui.label("Maximum transfers");
                ui.add(
                    egui::DragValue::new(&mut self.request.limits.max_transfers).range(1..=100_000),
                );
                ui.label("Jump context depth");
                ui.add(egui::DragValue::new(&mut self.request.limits.context_depth).range(0..=16));
                ui.label("Constants per abstract value");
                ui.add(egui::DragValue::new(&mut self.request.limits.max_constants).range(1..=32));
            });
            submit = ui
                .add_enabled(
                    self.phase != Phase::Loading,
                    egui::Button::new(
                        RichText::new("▶  Analyze")
                            .strong()
                            .color(palette::BACKGROUND),
                    )
                    .fill(palette::ACCENT),
                )
                .clicked();
            ui.label(RichText::new("Ctrl+Enter").small().color(palette::MUTED));
            if self.phase == Phase::Loading {
                ui.spinner();
            }
        });
        crate::framework::bytecode_editor(ui, &mut self.request.bytecode);
        ui.label(
            RichText::new(
                "Single runtime program · unknown calldata, environment and persistent state",
            )
            .small()
            .color(palette::MUTED),
        );
        ui.add_space(4.0);
        submit
    }

    fn status(&mut self, ui: &mut Ui) {
        match &self.phase {
            Phase::Failed(message) => {
                ui.colored_label(palette::ERROR, message);
            }
            Phase::Loading => {
                ui.colored_label(palette::ACCENT, "Analyzing bytecode…");
            }
            _ => {
                if let Some(report) = &self.report {
                    let complete = report.status == AnalysisStatus::Converged;
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            if complete {
                                palette::ACCENT
                            } else {
                                palette::WARNING
                            },
                            if complete {
                                "●  Model converged"
                            } else {
                                "●  Incomplete coverage"
                            },
                        );
                        ui.separator();
                        ui.label(format!(
                            "{} bytes · {} states · {} edges · {} values · {} transfers",
                            report.byte_len,
                            report.cfg.len(),
                            report.edges.len(),
                            report.ssa.value_count,
                            report.transfers
                        ));
                        ui.separator();
                        ui.toggle_value(
                            &mut self.show_details,
                            format!(
                                "{} diagnostics · {} frontiers",
                                report.diagnostics.len(),
                                report.frontiers.len()
                            ),
                        );
                    });
                    if !report.ssa.complete {
                        ui.colored_label(palette::WARNING, format!("Partial SSA · {} deferred edges; missing paths are not proven infeasible", report.ssa.deferred_edges.len()));
                    }
                    if self.show_details {
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                if report.diagnostics.is_empty() && report.frontiers.is_empty() {
                                    ui.label("No diagnostics or unexpanded frontiers reported.");
                                }
                                for diagnostic in &report.diagnostics {
                                    if ui
                                        .selectable_label(
                                            false,
                                            format!(
                                                "S{} · 0x{:04x} · {:?}: {}",
                                                diagnostic.state,
                                                diagnostic.pc,
                                                diagnostic.kind,
                                                diagnostic.detail
                                            ),
                                        )
                                        .clicked()
                                    {
                                        self.selection = Selection {
                                            state: Some(diagnostic.state),
                                            pc: Some(diagnostic.pc),
                                        };
                                    }
                                }
                                for frontier in &report.frontiers {
                                    if ui
                                        .selectable_label(
                                            false,
                                            RichText::new(format!(
                                                "{:?} · S{:?} · pc {:?}: {}",
                                                frontier.kind,
                                                frontier.from,
                                                frontier.pc,
                                                frontier.detail
                                            ))
                                            .color(palette::WARNING),
                                        )
                                        .clicked()
                                    {
                                        self.selection = Selection {
                                            state: frontier.from,
                                            pc: frontier.pc,
                                        };
                                    }
                                }
                            });
                    }
                } else {
                    ui.colored_label(
                        palette::MUTED,
                        "Select an instruction or graph node to follow its disassembly and SSA.",
                    );
                }
            }
        }
    }

    /// Clear the old snapshot and start a new request; input changes alone never
    /// relabel an existing result as if it were analyzed under different rules.
    pub fn begin_analysis(&mut self) -> AnalyzeRequest {
        self.report = None;
        self.phase = Phase::Loading;
        self.selection = Selection::default();
        self.disasm_focus = Selection::default();
        self.ssa_focus = Selection::default();
        self.graph = widgets::Graph::default();
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
                "Ready: {} instructions, {} CFG blocks, {} SSA blocks, {} SSA values; {:?}; SSA {}",
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
                }
            ),
            (Phase::Failed(message), _) => format!("Error: {message}"),
            (Phase::Loading, _) => "Loading: analyzing bytecode".into(),
            _ => "Idle: enter bytecode to analyze".into(),
        }
    }
}
