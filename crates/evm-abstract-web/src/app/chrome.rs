//! Bounded navigation, input and status chrome. Details scroll independently so
//! long bytecode or diagnostics cannot consume the analysis viewport.

use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisStatus, Fork};

use super::{BRANCH_EXAMPLE, Phase, Selection, View, Workspace};
use crate::palette;

impl Workspace {
    pub(super) fn header(&mut self, ui: &mut Ui) {
        let compact = ui.available_width() < 720.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if compact {
                    "EVM LAB"
                } else {
                    "EVM ABSTRACT LAB"
                })
                .strong()
                .color(palette::ACCENT),
            );
            let views = [
                (View::Split, "Workspace", "Workspace  0"),
                (View::Disassembly, "Disassembly", "Disassembly  1"),
                (View::Graph, "Control flow", "Control flow  2"),
                (View::Ssa, "SSA", "SSA  3"),
            ];
            if compact {
                let label = views
                    .iter()
                    .find(|(view, _, _)| *view == self.view)
                    .map_or("Workspace", |(_, label, _)| *label);
                egui::ComboBox::from_id_salt("workspace_view")
                    .selected_text(label)
                    .width(108.0)
                    .show_ui(ui, |ui| {
                        for (view, _, shortcut) in views {
                            ui.selectable_value(&mut self.view, view, shortcut);
                        }
                    });
            } else {
                for (view, label, shortcut) in views {
                    ui.selectable_value(&mut self.view, view, label)
                        .on_hover_text(shortcut);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_input, "Bytecode")
                    .on_hover_text("Show or hide the runtime bytecode editor");
            });
        });
    }

    pub(super) fn input(&mut self, ui: &mut Ui, viewport_height: f32) -> bool {
        let mut submit = false;
        let wide = ui.available_width() >= 720.0;
        ui.horizontal_wrapped(|ui| {
            if wide {
                ui.label(RichText::new("RUNTIME BYTECODE").small().color(palette::MUTED))
                    .on_hover_text("Single runtime program with unknown calldata, environment and persistent state");
            }
            egui::ComboBox::from_id_salt("fork")
                .selected_text(format!("{:?}", self.request.fork))
                .width(72.0)
                .show_ui(ui, |ui| {
                    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
                        ui.selectable_value(&mut self.request.fork, fork, format!("{fork:?}"));
                    }
                })
                .response
                .on_hover_text("EVM fork");
            ui.menu_button("Examples", |ui| {
                if ui.button("Branch example").clicked() {
                    self.request.bytecode = BRANCH_EXAMPLE.into();
                    ui.close();
                }
                if ui.button("Arithmetic").clicked() {
                    self.request.bytecode = "600160020160030200".into();
                    ui.close();
                }
            });
            ui.menu_button("Limits", |ui| {
                egui::Grid::new("analysis_limits").show(ui, |ui| {
                    ui.label("Maximum states");
                    ui.add(egui::DragValue::new(&mut self.request.limits.max_states).range(1..=4096));
                    ui.end_row();
                    ui.label("Maximum transfers");
                    ui.add(egui::DragValue::new(&mut self.request.limits.max_transfers).range(1..=100_000));
                    ui.end_row();
                    ui.label("Jump context depth");
                    ui.add(egui::DragValue::new(&mut self.request.limits.context_depth).range(0..=16));
                    ui.end_row();
                    ui.label("Constants per value");
                    ui.add(egui::DragValue::new(&mut self.request.limits.max_constants).range(1..=32));
                    ui.end_row();
                });
            });
            submit = ui.add_enabled(
                self.phase != Phase::Loading,
                egui::Button::new(RichText::new("▶ Analyze").strong().color(palette::BACKGROUND))
                    .fill(palette::ACCENT),
            ).on_hover_text("Analyze runtime bytecode · Ctrl+Enter").clicked();
            if wide {
                ui.label(RichText::new("Ctrl+Enter").small().color(palette::MUTED));
            }
            if self.phase == Phase::Loading {
                ui.spinner();
            }
        });
        let editor_height = if viewport_height < 480.0 { 24.0 } else { 40.0 };
        crate::framework::bytecode_editor(ui, &mut self.request.bytecode, editor_height);
        submit
    }

    pub(super) fn status(&mut self, ui: &mut Ui, viewport_height: f32) {
        let details_height = (viewport_height * 0.20).clamp(36.0, 140.0);
        match &self.phase {
            Phase::Failed(message) => {
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.show_details, "Error details");
                    ui.add(
                        egui::Label::new(RichText::new(message).color(palette::ERROR)).truncate(),
                    )
                    .on_hover_text(message);
                });
                if self.show_details {
                    egui::ScrollArea::vertical()
                        .id_salt("error_details")
                        .min_scrolled_height(details_height)
                        .max_height(details_height)
                        .show(ui, |ui| {
                            ui.colored_label(palette::ERROR, message);
                        });
                }
            }
            Phase::Loading => {
                ui.colored_label(palette::ACCENT, "Analyzing bytecode…");
            }
            _ => {
                if let Some(report) = &self.report {
                    let complete = report.status == AnalysisStatus::Converged;
                    let summary = format!(
                        "{} bytes · {} states · {} edges · {} values · {} transfers",
                        report.byte_len,
                        report.cfg.len(),
                        report.edges.len(),
                        report.ssa.value_count,
                        report.transfers,
                    );
                    let status = match (complete, report.ssa.complete) {
                        (true, true) => "● Model converged",
                        (false, true) => "● Incomplete coverage",
                        (false, false) => "● Incomplete · partial SSA",
                        (true, false) => "● Partial SSA",
                    };
                    let coverage = if report.ssa.complete {
                        format!("Analysis: {:?}; SSA complete", report.status)
                    } else {
                        format!(
                            "Analysis: {:?}; partial SSA · {} deferred edges; missing paths are not proven infeasible",
                            report.status,
                            report.ssa.deferred_edges.len()
                        )
                    };
                    let narrow = ui.available_width() < 720.0;
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            if complete && report.ssa.complete {
                                palette::ACCENT
                            } else {
                                palette::WARNING
                            },
                            status,
                        )
                        .on_hover_text(&coverage);
                        ui.toggle_value(
                            &mut self.show_details,
                            if narrow {
                                format!(
                                    "Details {}",
                                    report.diagnostics.len() + report.frontiers.len()
                                )
                            } else {
                                format!(
                                    "{} diagnostics · {} frontiers",
                                    report.diagnostics.len(),
                                    report.frontiers.len()
                                )
                            },
                        )
                        .on_hover_text(format!(
                            "{} diagnostics · {} unexpanded frontiers\n{summary}\n{coverage}",
                            report.diagnostics.len(),
                            report.frontiers.len()
                        ));
                        if !narrow {
                            ui.add(egui::Label::new(&summary).truncate())
                                .on_hover_text(&summary);
                        }
                    });
                    if self.show_details {
                        egui::ScrollArea::vertical()
                            .id_salt("analysis_details")
                            .min_scrolled_height(details_height)
                            .max_height(details_height)
                            .show(ui, |ui| {
                                ui.label(&summary);
                                if !report.ssa.complete {
                                    ui.colored_label(palette::WARNING, &coverage);
                                }
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
                    ui.add(
                        egui::Label::new(
                            RichText::new("Select an instruction or graph node to link the views.")
                                .color(palette::MUTED),
                        )
                        .truncate(),
                    );
                }
            }
        }
    }
}
