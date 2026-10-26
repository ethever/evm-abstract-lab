//! One shell for source-only, conservative and Native publications. Renderers
//! borrow qualified evidence and emit events; the shell applies them centrally.

mod focus;
mod inspection;
pub(super) use inspection::OriginalInspection;
mod navigation;

use super::model::navigation::{ContextFocus, Focus};
use super::{View, Workspace, chrome, inspector, layout::AnalysisPanes};
use crate::{palette, widgets};
use egui::Ui;

pub(super) struct Preparation {
    pub(super) shown: bool,
    pub(super) ready: bool,
}

impl Workspace {
    pub(super) fn show_workbench(&mut self, ui: &mut Ui, viewport: egui::Vec2) -> Preparation {
        self.reconcile_widget_selection();
        if self
            .report
            .as_ref()
            .is_some_and(|report| report.hybrid.is_some())
        {
            egui::Panel::top("model_scope")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.show_model_modes(ui));
        }
        if self.report.is_some() {
            egui::Panel::top("result_context")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.navigation_bar(ui));
        }
        let maximized = self.report.is_some() && self.inspector.maximized() && !self.form.open;
        if self.report.is_some() && self.inspector.expanded && !maximized && !self.form.open {
            let panel = self.inspector.docked_panel(ui, viewport.y);
            let response = panel.show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                self.show_inspection(ui);
            });
            self.inspector.remember_resize(ui, response.response.rect);
        }
        let source_sheet = self.report.is_some()
            && self.directory.open
            && self.panels.compact()
            && !self.form.open;
        if self.report.is_some()
            && self.directory.open
            && !source_sheet
            && !maximized
            && !self.form.open
        {
            let mut open = true;
            let mut action = None;
            chrome::source_panel(ui, viewport).show(ui, |ui| {
                chrome::source_heading(ui, &mut open);
                if let (Some(report), Some(location)) =
                    (&self.report, self.report_view.navigator().current())
                {
                    action = self.directory.show_scoped(
                        ui,
                        self.report_view.presentation(report),
                        &location.focus,
                        self.account.as_deref(),
                    );
                }
            });
            self.directory.open = open;
            if let Some(action) = action {
                self.directory_action(action);
                if viewport.x < 900.0 {
                    self.directory.open = false;
                }
                ui.ctx().request_discard("source/context navigation");
            }
        }
        let mut ready = true;
        let mut sheet_action = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(palette::BACKGROUND)
                    .inner_margin(4.0),
            )
            .show(ui, |ui| {
                if self.report.is_none() {
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
                } else if source_sheet {
                    let mut open = self.directory.open;
                    chrome::source_heading(ui, &mut open);
                    self.directory.open = open;
                    if let (Some(report), Some(location)) =
                        (&self.report, self.report_view.navigator().current())
                    {
                        sheet_action = self.directory.show_scoped(
                            ui,
                            self.report_view.presentation(report),
                            &location.focus,
                            self.account.as_deref(),
                        );
                    }
                } else if maximized {
                    self.show_inspection(ui);
                } else {
                    ready = self.show_route(ui);
                }
            });
        if let Some(action) = sheet_action {
            self.directory_action(action);
            self.directory.open = false;
            ui.ctx().request_discard("source sheet navigation");
        }
        Preparation {
            shown: !ui.is_sizing_pass(),
            ready,
        }
    }

    fn show_inspection(&mut self, ui: &mut Ui) {
        if self
            .original_inspection
            .as_ref()
            .is_some_and(|subject| subject.view != self.report_view.identity())
        {
            self.original_inspection = None;
        }
        if let (Some(subject), Some(report)) = (&self.original_inspection, &self.report) {
            self.inspector.inspect(
                ui,
                inspector::InspectionTarget::Original {
                    report,
                    account: subject.borrowed(),
                },
            );
            return;
        }

        let Some(focus) = self
            .report_view
            .navigator()
            .current()
            .map(|location| location.focus.clone())
        else {
            return;
        };
        let Some(report) = &self.report else { return };
        let action = match focus.context {
            Some(ContextFocus::Minimum { .. }) => report.minimum.as_deref().and_then(|model| {
                self.inspector.inspect(
                    ui,
                    inspector::InspectionTarget::Global {
                        report,
                        model,
                        selection: self.minimum_selection,
                    },
                )
            }),
            Some(ContextFocus::Graph { .. }) => {
                let shown = self.report_view.shown(report);
                self.inspector.inspect(
                    ui,
                    inspector::InspectionTarget::Native {
                        report: &shown,
                        selection: self.selection,
                        account: self.account.as_deref(),
                    },
                )
            }
            None => match focus.source.as_ref() {
                Some(super::model::navigation::SourceFocus::Captured(source)) => {
                    self.inspector.inspect(
                        ui,
                        inspector::InspectionTarget::Source {
                            report,
                            source: inspector::InspectionSource::Program(source.program),
                        },
                    )
                }
                Some(super::model::navigation::SourceFocus::Uncaptured { source, .. }) => {
                    self.inspector.inspect(
                        ui,
                        inspector::InspectionTarget::Source {
                            report,
                            source: inspector::InspectionSource::Process(source),
                        },
                    )
                }
                None => {
                    widgets::empty(ui, "Choose a source or context to inspect its evidence");
                    None
                }
            },
        };
        match action {
            Some(inspector::InspectionAction::Native(selection)) => {
                self.accept_native_selection(selection)
            }
            Some(inspector::InspectionAction::Global(selection)) => {
                self.accept_minimum_selection(selection)
            }
            None => {}
        }
    }

    fn show_route(&mut self, ui: &mut Ui) -> bool {
        let Some(focus) = self
            .report_view
            .navigator()
            .current()
            .map(|location| location.focus.clone())
        else {
            widgets::empty(
                ui,
                "No qualified source or context is available in this publication",
            );
            return true;
        };
        if self.view == View::Disassembly {
            if let Some(report) = &self.report
                && let Ok(capabilities) = self.report_view.presentation(report).capabilities(&focus)
                && let super::model::presentation::Capability::Unavailable(reason) =
                    capabilities.code
            {
                widgets::heading(ui, "CODE", "Source availability");
                availability(ui, reason.label());
                return true;
            }
            let picked = self.report.as_ref().and_then(|report| {
                self.source_cache
                    .show_context(ui, self.report_view.presentation(report), &focus)
            });
            if let Some(focus) = picked {
                self.accept_focus(focus);
            }
            return true;
        }
        if self.view == View::Ssa {
            return self.show_ssa_route(ui, &focus);
        }
        if focus.context.is_none() {
            widgets::heading(ui, "CONTROL FLOW", "Choose an execution context");
            availability(
                ui,
                "This source has decoded code. A compatible context is required to show its control evidence.",
            );
            if crate::controls::action(ui, "Choose context", crate::controls::Density::Regular)
                .clicked()
            {
                self.directory.open = true;
            }
            return true;
        }
        if focus.view.is_minimum() {
            if self.view == View::Split && ui.available_width() >= 900.0 {
                let mut picked = None;
                egui::Panel::left("compare_source")
                    .resizable(true)
                    .default_size(280.0)
                    .show(ui, |ui| {
                        if let Some(report) = &self.report {
                            picked = self.source_cache.show_context(
                                ui,
                                self.report_view.presentation(report),
                                &focus,
                            );
                        }
                    });
                if let Some(focus) = picked {
                    self.accept_focus(focus);
                }
            }
            let intent = self.graph.show_minimum_scoped(
                ui,
                self.report.as_ref().unwrap(),
                focus.view,
                &mut self.minimum_selection,
            );
            if let Some(intent) = intent {
                self.graph_intent(intent);
            } else {
                self.reconcile_widget_selection();
            }
            return self.graph.prepared();
        }
        let before = self.selection;
        let report = self.report.as_ref().unwrap();
        let shown = self.report_view.shown(report);
        let mut painted = false;
        let mut native_intent = None;
        let mut source_pick = None;
        let mut ready = true;
        if self.view == View::Split {
            let mut panes = AnalysisPanes {
                report: &shown,
                presentation: self.report_view.presentation(report),
                focus: &focus,
                source_cache: &mut self.source_cache,
                source_pick: &mut source_pick,
                graph_intent: &mut native_intent,
                program: self.program,
                selection: &mut self.selection,
                graph: &mut self.graph,
                ssa_focus: &mut self.ssa_focus,
                ssa_cache: &mut self.ssa_cache,
                ssa_display: self.ssa_display,
                view_shown: &mut painted,
                view_ready: &mut ready,
            };
            self.layout.show(ui, &mut panes);
        } else {
            self.graph.set_ssa_display(self.ssa_display, self.selection);
            native_intent = self.graph.show_scoped(ui, &shown, &mut self.selection);
            ready = self.graph.prepared();
        }
        if let Some(intent) = native_intent {
            self.native_intent(intent);
        } else if self.selection != before {
            self.accept_native_selection(self.selection);
        }
        if let Some(focus) = source_pick {
            self.accept_focus(focus);
        }
        ready
    }

    fn show_ssa_route(&mut self, ui: &mut Ui, focus: &Focus) -> bool {
        let Some(report) = &self.report else {
            return true;
        };
        let capability = self
            .report_view
            .presentation(report)
            .capabilities(focus)
            .map(|capabilities| capabilities.ssa);
        if !capability.is_ok_and(|capability| capability.available()) {
            widgets::heading(ui, "SSA", "Qualified instruction evidence");
            let reason = match capability {
                Ok(super::model::presentation::Capability::Unavailable(reason)) => {
                    reason.label().to_string()
                }
                Err(error) => error.to_string(),
                _ => "No retained instruction evidence is available".into(),
            };
            availability(ui, &reason);
            if crate::controls::action(
                ui,
                "Choose Native context",
                crate::controls::Density::Regular,
            )
            .clicked()
            {
                self.choose_context(evm_abstract_protocol::AnalysisNamespace::Native);
            }
            return true;
        }
        let before = self.selection;
        let shown = self.report_view.shown(report);
        widgets::ssa_cached(
            ui,
            &shown,
            self.program,
            self.ssa_display,
            &mut self.selection,
            &mut self.ssa_focus,
            &mut self.ssa_cache,
        );
        if self.selection != before {
            self.accept_native_selection(self.selection);
        }
        true
    }
}

fn availability(ui: &mut Ui, message: &str) {
    let response = ui.label(message);
    crate::controls::observe(ui, &response, message, "availability", None);
}
