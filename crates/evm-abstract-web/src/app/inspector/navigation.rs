//! Shared compact content navigation, independent of observation coordinates.

use super::{Inspector, Tab};
use crate::controls::{self, Density};
use egui::{RichText, Ui};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    State,
    Data,
    Effects,
    Evidence,
    Timing,
}

impl Group {
    const ALL: [(Self, &'static str); 5] = [
        (Self::State, "State"),
        (Self::Data, "Data"),
        (Self::Effects, "Effects"),
        (Self::Evidence, "Evidence"),
        (Self::Timing, "Timing"),
    ];

    fn of(tab: Tab) -> Self {
        match tab {
            Tab::Frame | Tab::Stack | Tab::SsaValue => Self::State,
            Tab::Memory | Tab::Calldata | Tab::Returndata => Self::Data,
            Tab::Storage | Tab::Transient | Tab::Outcomes => Self::Effects,
            Tab::Coverage | Tab::Acquisition | Tab::Environment | Tab::Diagnostics => {
                Self::Evidence
            }
            Tab::Timing => Self::Timing,
        }
    }

    fn initial(self) -> Tab {
        match self {
            Self::State => Tab::Frame,
            Self::Data => Tab::Memory,
            Self::Effects => Tab::Storage,
            Self::Evidence => Tab::Coverage,
            Self::Timing => Tab::Timing,
        }
    }
}

impl Inspector {
    pub(super) fn navigation(&mut self, ui: &mut Ui) {
        let narrow = ui.available_width() < 660.0;
        Density::Compact.scope(ui, |ui| {
            ui.horizontal(|ui| {
                let mut group = Group::of(self.tab);
                let previous = group;
                if narrow {
                    egui::ComboBox::from_id_salt("inspection_group")
                        .width(72.0)
                        .selected_text(Group::ALL.iter().find(|(id, _)| *id == group).unwrap().1)
                        .show_ui(ui, |ui| {
                            for (choice, label) in Group::ALL {
                                controls::tab_value(
                                    ui,
                                    &mut group,
                                    choice,
                                    label,
                                    Density::Compact,
                                );
                            }
                        });
                } else {
                    ui.label(
                        RichText::new("INSPECT")
                            .small()
                            .color(crate::palette::MUTED),
                    );
                    for (choice, label) in Group::ALL {
                        controls::tab_value(ui, &mut group, choice, label, Density::Compact);
                    }
                }
                if previous != group {
                    self.tab = group.initial();
                }
                egui::ComboBox::from_id_salt("inspector_tab")
                    .width(if narrow { 88.0 } else { 116.0 })
                    .selected_text(self.tab.label())
                    .show_ui(ui, |ui| {
                        for (tab, label) in Tab::ALL {
                            if Group::of(tab) == group {
                                controls::tab_value(
                                    ui,
                                    &mut self.tab,
                                    tab,
                                    label,
                                    Density::Compact,
                                );
                            }
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.window_controls(ui);
                });
            });
        });
    }
}
