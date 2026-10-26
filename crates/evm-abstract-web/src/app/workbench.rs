//! Composes a single active route with anchored source and inspector panels.
//! The report and selection remain owned by Workspace; this layer only borrows
//! them for the current frame, including conservative-only publications.

use super::layout::{AnalysisPanes, Pane};
use super::{View, Workspace, chrome};
use crate::{palette, widgets};
use egui::Ui;

pub(super) struct Preparation {
    pub(super) shown: bool,
    pub(super) ready: bool,
}

impl Workspace {
    pub(super) fn show_workbench(&mut self, ui: &mut Ui, viewport: egui::Vec2) -> Preparation {
        let mut view_shown = false;
        let mut view_ready = true;
        // Freeze this frame's presentation: a title action takes effect on
        // the next frame, keeping the inspector's widget IDs unique.
        let inspector_maximized = self.report.is_some() && self.inspector.maximized();
        if self
            .report
            .as_ref()
            .is_some_and(|report| report.hybrid.is_some())
        {
            egui::Panel::top("model_scope")
                .frame(palette::chrome_frame())
                .show(ui, |ui| self.model_selector(ui));
        }
        if self
            .report
            .as_ref()
            .is_some_and(|report| self.report_view.minimum(report).is_some())
        {
            return Preparation {
                ready: self.show_minimum(ui, viewport),
                shown: !ui.is_sizing_pass(),
            };
        }
        if let Some(report) = &self.report {
            let report = &self.report_view.shown(report);
            egui::Panel::top("result_context")
                .frame(palette::chrome_frame())
                .show(ui, |ui| {
                    chrome::result_context(
                        ui,
                        report,
                        self.program,
                        self.previous_report,
                        &mut self.ssa_display,
                    );
                });
            if self.inspector.expanded && !inspector_maximized {
                let response = self.inspector.docked_panel(ui, viewport.y).show(ui, |ui| {
                    // Reserve the chosen panel height even for an empty snapshot;
                    // otherwise egui shrinks it after the graph's initial fit.
                    ui.set_min_height(ui.available_height());
                    self.inspector
                        .show(ui, report, &mut self.selection, self.account.as_deref());
                });
                self.inspector.remember_resize(ui, response.response.rect);
            }
        }
        let mut pick = None;
        if self.directory.open
            && !inspector_maximized
            && let Some(report) = &self.report
        {
            let report = &self.report_view.shown(report);
            let mut open = true;
            chrome::source_panel(ui, viewport).show(ui, |ui| {
                chrome::source_heading(ui, &mut open);
                pick = self
                    .directory
                    .show(ui, report, self.program, self.account.as_deref());
            });
            self.directory.open = open && (viewport.x >= 900.0 || pick.is_none());
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
                    let report = &self.report_view.shown(report);
                    if inspector_maximized {
                        self.inspector.show(
                            ui,
                            report,
                            &mut self.selection,
                            self.account.as_deref(),
                        );
                        view_shown = !ui.is_sizing_pass();
                        return;
                    }
                    let mut panes = AnalysisPanes {
                        report,
                        selection: &mut self.selection,
                        program: self.program,
                        graph: &mut self.graph,
                        disasm_focus: &mut self.disasm_focus,
                        ssa_focus: &mut self.ssa_focus,
                        ssa_cache: &mut self.ssa_cache,
                        ssa_display: self.ssa_display,
                        view_shown: &mut view_shown,
                        view_ready: &mut view_ready,
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
        Preparation {
            shown: view_shown,
            ready: view_ready,
        }
    }
}
