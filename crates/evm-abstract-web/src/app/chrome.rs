//! Compact workbench navigation, anchored panels and task status.

use super::{View, Workspace};
use crate::{
    controls::{self, Density},
    palette,
};
use egui::{RichText, Ui};

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
                    controls::observe(ui, &route.response, "workspace_view", "combobox", None);
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
                    let mut sources = self.sources_visible();
                    if controls::toggle(ui, &mut sources, "Sources", density)
                        .on_hover_text(
                            "Captured code, compatible contexts and original account observations",
                        )
                        .changed()
                    {
                        if sources {
                            self.open_sources();
                        } else {
                            self.directory.open = false;
                        }
                        ui.ctx().request_discard("source visibility changed");
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
