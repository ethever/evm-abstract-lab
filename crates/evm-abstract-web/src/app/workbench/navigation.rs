//! Source/context navigation and meaningful scope labels for the shared shell.

use crate::app::model::{
    navigation::{EvidenceFocus, Focus, Intent},
    presentation::ResolvedContext,
};
use crate::app::{View, Workspace, directory, inspector, minimum};
use crate::{
    controls::{self, Density},
    palette, widgets,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisNamespace, ProcessLocation, ProcessOwner};

impl Workspace {
    pub(in crate::app) fn choose_context(&mut self, namespace: AnalysisNamespace) {
        self.directory.choose_context(namespace);
        self.directory.open = true;
        if self.panels.compact() {
            self.inspector.expanded = false;
        }
    }

    pub(super) fn show_model_modes(&mut self, ui: &mut Ui) {
        let Some(report) = &self.report else { return };
        let current = self.report_view.namespace();
        let mut requested = None;
        let density = if self.panels.compact() {
            Density::Touch
        } else {
            Density::Compact
        };
        density.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (namespace, label) in [
                    (AnalysisNamespace::Baseline, "Global model"),
                    (AnalysisNamespace::Native, "Detailed paths"),
                ] {
                    let available = namespace == AnalysisNamespace::Baseline
                        || report.graph(namespace).is_some();
                    let response = ui
                        .add_enabled_ui(available, |ui| {
                            controls::tab(ui, current == namespace, label, density)
                        })
                        .inner;
                    if response
                        .on_hover_text(
                            "Choose an exact source, owner and execution context in Sources",
                        )
                        .clicked()
                    {
                        requested = Some(namespace);
                    }
                }
                if current == AnalysisNamespace::Native
                    && let Some(hybrid) = &report.hybrid
                {
                    if let evm_abstract_protocol::HybridNativeBinding::SymbolicOwnerNamespace(
                        binding,
                    ) = &hybrid.native_coverage().detail_binding
                    {
                        ui.label(format!("Root ADDRESS = {}", binding.namespace));
                    }
                    if hybrid
                        .rpc
                        .as_ref()
                        .is_some_and(|rpc| !rpc.detail_at_current_revision)
                    {
                        ui.colored_label(palette::WARNING, "Earlier input conditions");
                    }
                }
            })
        });
        if let Some(namespace) = requested {
            self.choose_context(namespace);
        }
    }

    pub(super) fn navigation_bar(&mut self, ui: &mut Ui) {
        let mut intent = None;
        let navigator = self.report_view.navigator();
        let can_back = navigator.can_back();
        let can_forward = navigator.can_forward();
        let focus = navigator.current().map(|location| location.focus.clone());
        Density::Compact.scope(ui, |ui| {
            ui.horizontal(|ui| {
                if controls::action_enabled(ui, can_back, "Back", Density::Compact).clicked() {
                    intent = Some(Intent::Back);
                }
                if controls::action_enabled(ui, can_forward, "Forward", Density::Compact).clicked()
                {
                    intent = Some(Intent::Forward);
                }
                if controls::action(ui, "Entry", Density::Compact).clicked()
                    && let Some(report) = &self.report
                    && let Ok(entry) = self
                        .report_view
                        .presentation(report)
                        .entry_focus(self.report_view.identity())
                {
                    intent = Some(Intent::Open(entry));
                }
                if let (Some(focus), Some(report)) = (&focus, &self.report) {
                    let presentation = self.report_view.presentation(report);
                    let source = focus
                        .source
                        .as_ref()
                        .and_then(|source| source.program())
                        .and_then(|id| presentation.program(id));
                    let label = source.map_or_else(
                        || "Uncaptured source".to_owned(),
                        |source| {
                            format!(
                                "{:?} · {}",
                                source.kind,
                                directory::short(
                                    source.code_address.as_deref().unwrap_or("Input code")
                                )
                            )
                        },
                    );
                    ui.add(egui::Label::new(RichText::new(label).color(palette::TEXT)).truncate())
                        .on_hover_ui(|ui| {
                            if let Some(source) = source {
                                ui.monospace(
                                    source
                                        .code_address
                                        .as_deref()
                                        .unwrap_or("Supplied or constructor bytes"),
                                );
                                ui.monospace(&source.code_hash);
                            }
                        });
                    let scope = match presentation.resolve_context(focus) {
                        Ok(Some(ResolvedContext::Minimum { procedure, .. })) => {
                            procedure.process.as_ref().map_or_else(
                                || "Procedure bounds".to_owned(),
                                |frame| {
                                    format!(
                                        "MAY · {} · {}",
                                        owner(&frame.owner),
                                        if frame.is_static { "static" } else { "mutable" }
                                    )
                                },
                            )
                        }
                        Ok(Some(ResolvedContext::Graph {
                            state,
                            frame,
                            at_current_revision,
                            ..
                        })) => format!(
                            "Native · {} · {}{}",
                            directory::short(
                                state.storage_address.as_deref().unwrap_or("logical owner")
                            ),
                            if frame.is_some_and(|frame| frame.is_static) {
                                "static"
                            } else {
                                "mutable"
                            },
                            if at_current_revision {
                                ""
                            } else {
                                " · earlier input"
                            },
                        ),
                        _ => "Source only · choose context".into(),
                    };
                    ui.add(
                        egui::Label::new(RichText::new(scope).small().color(palette::MUTED))
                            .truncate(),
                    );
                }
                if let Some(error) = self.navigation_error {
                    ui.colored_label(palette::WARNING, error.to_string());
                }
            })
        });
        if let Some(intent) = intent {
            self.navigate(intent, None);
            ui.ctx().request_discard("navigation location changed");
        }
    }

    pub(super) fn directory_action(&mut self, action: directory::Action) {
        match action {
            directory::Action::Navigate(intent) => {
                let route = match &intent {
                    Intent::Open(Focus { context: None, .. }) => Some(View::Disassembly),
                    Intent::Open(_) => Some(View::Graph),
                    _ => None,
                };
                self.navigate(intent, route);
            }
            directory::Action::Account(address) => {
                self.account = Some(address);
                self.inspector.select_original();
                self.inspector.expanded = true;
                self.inspector.tab = inspector::Tab::Storage;
            }
            directory::Action::MinimumAccount(index) => {
                self.minimum_selection.account = Some(index);
                self.inspector.select_original();
                self.inspector.expanded = true;
            }
            directory::Action::RootInput => {
                self.account = None;
                self.minimum_selection.account = None;
                self.inspector.select_original();
                self.inspector.expanded = true;
                self.inspector.tab = inspector::Tab::Environment;
            }
        }
    }

    pub(super) fn native_intent(&mut self, intent: widgets::NativeIntent) {
        match intent {
            widgets::NativeIntent::Select(selection) => self.accept_native_selection(selection),
            widgets::NativeIntent::Open(selection) => {
                let Some(report) = &self.report else { return };
                match self.report_view.presentation(report).graph_focus(
                    self.report_view.identity(),
                    selection,
                    None,
                ) {
                    Ok(focus) => self.navigate(Intent::Open(focus), Some(View::Graph)),
                    Err(error) => self.navigation_error = Some(error),
                }
            }
        }
    }

    pub(super) fn graph_intent(&mut self, intent: widgets::GraphIntent) {
        match intent {
            widgets::GraphIntent::Select(target) | widgets::GraphIntent::Open(target) => {
                let selection = minimum::Selection {
                    summary: target.summary,
                    procedure: target.procedure,
                    node: Some(target.origin.node),
                    control: target.origin.location,
                    ..Default::default()
                };
                let Some(report) = &self.report else { return };
                let focus = self.report_view.presentation(report).minimum_focus(
                    self.report_view.identity(),
                    selection,
                    None,
                );
                match focus {
                    Ok(focus) => {
                        if matches!(intent, widgets::GraphIntent::Open(_)) {
                            self.navigate(Intent::Open(focus), Some(View::Graph));
                        } else {
                            self.accept_focus(focus);
                        }
                    }
                    Err(error) => self.navigation_error = Some(error),
                }
            }
            widgets::GraphIntent::OpenSource {
                program, location, ..
            } => {
                let Some(report) = &self.report else { return };
                let presentation = self.report_view.presentation(report);
                let Some(source) = presentation.source(program) else {
                    return;
                };
                let block = match location {
                    ProcessLocation::Block(index) => presentation
                        .program(program)
                        .and_then(|program| program.blocks.get(index))
                        .map(|block| block.id),
                    _ => None,
                };
                let focus = Focus {
                    evidence: EvidenceFocus::Source { block, pc: None },
                    ..Focus::source(self.report_view.identity(), source)
                };
                self.navigate(Intent::Open(focus), Some(View::Disassembly));
            }
        }
    }
}

fn owner(owner: &ProcessOwner) -> String {
    match owner {
        ProcessOwner::RootNamespace => "root owner".into(),
        ProcessOwner::Named(address) => format!("owner {}", directory::short(address)),
        ProcessOwner::Residual => "residual owner".into(),
    }
}
