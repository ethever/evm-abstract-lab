//! Compact workbench navigation, anchored panels and task status.

use super::{View, Workspace, directory};
use crate::{
    controls::{self, Density},
    notation, palette,
};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;

mod panels;
mod status;
pub(super) use panels::PanelLayout;

impl Workspace {
    pub(super) fn header(&mut self, ui: &mut Ui) {
        let compact = ui.available_width() < 680.0;
        let density = if compact {
            Density::Touch
        } else {
            Density::Regular
        };
        density.scope(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if compact { "EVM" } else { "EVM LAB" })
                        .strong()
                        .color(palette::ACCENT),
                );
                let views = [
                    (View::Graph, "CFG"),
                    (View::Disassembly, "Code"),
                    (View::Ssa, "SSA"),
                    (View::Split, "Compare"),
                ];
                if compact {
                    let title = views
                        .iter()
                        .find(|(view, _)| *view == self.view)
                        .map_or("CFG", |(_, title)| *title);
                    let route = egui::ComboBox::from_id_salt("workspace_view")
                        .width(82.0)
                        .selected_text(title)
                        .show_ui(ui, |ui| {
                            for (view, title) in views {
                                controls::tab_value(ui, &mut self.view, view, title, density);
                            }
                        });
                    controls::observe(ui, &route.response, "Workspace view", "menu", None);
                } else {
                    for (view, title) in views {
                        let index = match view {
                            View::Split => 0,
                            View::Disassembly => 1,
                            View::Graph => 2,
                            View::Ssa => 3,
                        };
                        controls::tab_value(ui, &mut self.view, view, title, density)
                            .on_hover_text(format!("Shortcut {index}"));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if controls::action(ui, if compact { "Input" } else { "New analysis" }, density)
                        .clicked()
                    {
                        self.form.open = true;
                    }
                    if controls::toggle(ui, &mut self.inspector.expanded, "Inspect", density)
                        .on_hover_text(self.inspector.visibility_hint(ui.ctx()))
                        .changed()
                        && self.inspector.expanded
                        && self.panels.compact()
                    {
                        self.directory.open = false;
                    }
                    if controls::toggle(ui, &mut self.directory.open, "Sources", density)
                        .on_hover_text(
                            "Captured code, compatible contexts and original account observations",
                        )
                        .changed()
                        && self.directory.open
                        && self.panels.compact()
                    {
                        self.inspector.expanded = false;
                    }
                });
            });
        });
    }
}

/// A narrow source drawer consumes space below navigation instead of covering
/// mode controls. Both representations use this geometry and the same IDs.
pub(super) fn source_panel(ui: &Ui, viewport: egui::Vec2) -> egui::Panel {
    if viewport.x >= 900.0 {
        egui::Panel::left("world_directory")
            .resizable(true)
            .default_size(208.0)
            .min_size(160.0)
            .max_size(320.0)
            .frame(palette::chrome_frame())
    } else {
        let maximum = (ui.available_height() * 0.42).clamp(0.0, 240.0);
        egui::Panel::top("sources_drawer")
            .resizable(true)
            .default_size(180.0_f32.min(maximum))
            .min_size(72.0_f32.min(maximum))
            .max_size(maximum)
            .frame(palette::chrome_frame())
    }
}

pub(super) fn source_heading(ui: &mut Ui, open: &mut bool) {
    Density::Compact.scope(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("SOURCES")
                    .small()
                    .strong()
                    .color(palette::MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if controls::action(ui, "Close", Density::Compact).clicked() {
                    *open = false;
                }
            });
        });
    });
}

#[cfg(test)]
mod tests;

pub(super) fn result_context(
    ui: &mut Ui,
    report: &crate::report::ReportView<'_>,
    program: Option<usize>,
    previous: bool,
    display: &mut crate::ssa_display::SsaDisplay,
) {
    let source = program.and_then(|id| report.programs.iter().find(|program| program.id == id));
    let program_label =
        source.map_or_else(
            || "No captured program".into(),
            |program| {
                format!(
                    "{} · {:?} · {}",
                    Symbol::Program(program.id),
                    program.kind,
                    directory::short(program.code_address.as_deref().unwrap_or(
                        match program.kind {
                            evm_abstract_protocol::CodeKind::Runtime => "Supplied input program",
                            evm_abstract_protocol::CodeKind::Initcode => "Constructor source",
                            _ => "No physical code account",
                        }
                    ))
                )
            },
        );
    let snapshot = report.metadata.snapshot.as_ref().map_or_else(
        || "Standalone bytecode".into(),
        |snapshot| {
            format!(
                "chain {} · block {} · {}",
                snapshot.chain_id,
                snapshot.number.as_deref().unwrap_or("unreported"),
                directory::short(&snapshot.block_hash)
            )
        },
    );
    let label = format!(
        "{}{} · {:?} · {snapshot}",
        if previous { "Previous result · " } else { "" },
        program_label,
        report.fork
    );
    // Metadata retains the original small-button height rather than reserving
    // a toolbar row above the analysis panes.
    Density::Inline.scope(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut full = *display == crate::ssa_display::SsaDisplay::Full;
                if controls::toggle(ui, &mut full, "SSA details", Density::Inline)
                    .on_hover_text("Show completed stack operations, effect chains and completion annotations in SSA views")
                    .changed()
                {
                    *display = if full {
                        crate::ssa_display::SsaDisplay::Full
                    } else {
                        crate::ssa_display::SsaDisplay::Compact
                    };
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(notation::widget(
                            ui,
                            RichText::new(label).small().color(palette::MUTED),
                        ))
                        .truncate(),
                    )
                    .on_hover_ui(|ui| {
                        if let Some(program) = source {
                            ui.monospace(program.code_address.as_deref().unwrap_or(match program.kind {
                                evm_abstract_protocol::CodeKind::Runtime => "Supplied input program",
                                evm_abstract_protocol::CodeKind::Initcode => "Constructor source",
                                _ => "No physical code account",
                            }));
                            ui.monospace(&program.code_hash);
                        }
                        if let Some(snapshot) = &report.metadata.snapshot {
                            ui.monospace(&snapshot.block_hash);
                            if controls::action(ui, "Copy block hash", Density::Compact).clicked() {
                                ui.ctx().copy_text(snapshot.block_hash.clone());
                            }
                        }
                        ui.monospace(&report.metadata.fingerprint);
                    });
                });
            });
        });
    });
}
