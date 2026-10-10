//! Bounded world inspector. Entry, historical exit and finalized outcomes are
//! explicit distinct observations, never an invented per-instruction trace.

mod buffers;
mod context;
mod errors;
mod evidence;
mod stores;
mod value;

use super::Selection;
use crate::{notation, palette};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{
    AnalysisReport, ApiError, BlockCoverage, FrameSnapshot, MachineSnapshot, StoreSnapshot,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Frame,
    Stack,
    Memory,
    Storage,
    Transient,
    Calldata,
    Returndata,
    Outcomes,
    Acquisition,
    Environment,
    Diagnostics,
}

impl Tab {
    const ALL: [(Self, &'static str); 11] = [
        (Self::Frame, "Frame"),
        (Self::Stack, "Stack"),
        (Self::Memory, "Memory"),
        (Self::Storage, "Storage"),
        (Self::Transient, "Transient"),
        (Self::Calldata, "Calldata"),
        (Self::Returndata, "Returndata"),
        (Self::Outcomes, "Outcomes"),
        (Self::Acquisition, "Acquisition"),
        (Self::Environment, "Environment"),
        (Self::Diagnostics, "Diagnostics"),
    ];
    fn label(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(tab, _)| *tab == self)
            .map_or("Frame", |(_, label)| *label)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Point {
    #[default]
    Entry,
    Exit,
}

pub(super) struct Inspector {
    pub(super) expanded: bool,
    pub(super) tab: Tab,
    point: Point,
    frame: Option<usize>,
    last_state: Option<usize>,
    outcome: usize,
    rollback: bool,
}
impl Default for Inspector {
    fn default() -> Self {
        Self {
            expanded: true,
            tab: Tab::Frame,
            point: Point::Entry,
            frame: None,
            last_state: None,
            outcome: 0,
            rollback: false,
        }
    }
}

impl Inspector {
    pub(super) fn accessible_summary(
        &self,
        report: &AnalysisReport,
        selection: Selection,
    ) -> String {
        if !self.expanded {
            return format!("Inspector hidden · {}", self.tab.label());
        }
        let machine = self.machine(report, selection);
        let boundary = match self.point {
            Point::Entry => "Block entry",
            Point::Exit if machine.is_none() => "Exit unavailable",
            Point::Exit
                if selection.state.is_some_and(|state| {
                    crate::widgets::coverage(report, state) == BlockCoverage::Current
                }) =>
            {
                "Observed exit"
            }
            Point::Exit => "Historical exit",
        };
        let frame = machine
            .and_then(|machine| selected_frame(machine, self.frame))
            .map_or("No frame".into(), |frame| {
                format!("Frame {}", Symbol::Frame(frame.index))
            });
        let store = if self.rollback && matches!(self.tab, Tab::Storage | Tab::Transient) {
            " · Rollback store"
        } else {
            ""
        };
        format!(
            "Inspector: {} · {boundary} · {frame}{store}",
            self.tab.label()
        )
    }

    /// Report-local IDs can repeat; reset observation coordinates, not UI preferences.
    pub(super) fn reset_report(&mut self) {
        self.point = Point::Entry;
        self.frame = None;
        self.last_state = None;
        self.outcome = 0;
        self.rollback = false;
    }

    fn machine<'a>(
        &self,
        report: &'a AnalysisReport,
        selection: Selection,
    ) -> Option<&'a MachineSnapshot> {
        let details = report
            .states
            .iter()
            .find(|state| Some(state.state) == selection.state)?;
        match self.point {
            Point::Entry => Some(&details.entry),
            Point::Exit => details.exit.as_ref(),
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        selection: &mut Selection,
        account: Option<&str>,
    ) {
        if self.last_state != selection.state {
            self.frame = None;
            self.point = Point::Entry;
            self.last_state = selection.state;
            self.rollback = false;
        }
        let narrow = ui.available_width() < 1120.0;
        ui.horizontal(|ui| {
            if narrow {
                egui::ComboBox::from_id_salt("inspector_tab")
                    .width(122.0)
                    .selected_text(self.tab.label())
                    .show_ui(ui, |ui| {
                        for (tab, label) in Tab::ALL {
                            ui.selectable_value(&mut self.tab, tab, label);
                        }
                    });
            } else {
                for (tab, label) in Tab::ALL {
                    ui.selectable_value(&mut self.tab, tab, label);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Hide").clicked() {
                    self.expanded = false;
                }
            });
        });
        let has_exit = report
            .states
            .iter()
            .find(|state| Some(state.state) == selection.state)
            .is_some_and(|state| state.exit.is_some());
        let coverage = selection
            .state
            .map(|state| crate::widgets::coverage(report, state));
        ui.horizontal_wrapped(|ui| {
            ui.label(notation::widget(ui, RichText::new(selection.state.map_or("No state selected".into(), |state| Symbol::State(state).to_string())).strong()));
            ui.selectable_value(&mut self.point,Point::Entry,"Block entry");
            ui.add_enabled_ui(has_exit, |ui| {
                ui.selectable_value(&mut self.point,Point::Exit,if coverage==Some(BlockCoverage::Current) {"Observed exit"} else {"Historical exit"});
            });
            if let Some(machine)=self.machine(report,*selection) {
                let selected=selected_frame(machine,self.frame).map_or(0,|frame|frame.index);
                egui::ComboBox::from_id_salt("inspector_frame").width(85.0).selected_text(notation::widget(ui, format!("Frame {}", Symbol::Frame(selected)))).show_ui(ui,|ui| {
                    for frame in &machine.frames {
                        ui.selectable_value(&mut self.frame,Some(frame.index),notation::widget(ui, format!("Frame {}{}", Symbol::Frame(frame.index), if frame.index+1==machine.frames.len(){" · active"}else{" · suspended"})));
                    }
                });
            }
            if let Some(pc)=selection.pc {
                ui.label(RichText::new(format!("PC 0x{pc:x}")).small().color(palette::MUTED))
                    .on_hover_text("Instruction selection links the views. Inspection shows a block boundary, not a per-instruction memory trace.");
            }
        });
        if self.point == Point::Exit && coverage != Some(BlockCoverage::Current) {
            ui.colored_label(palette::WARNING,"Historical receipt: current entry facts have changed. This is not a current exit snapshot.");
        }
        ui.separator();
        let machine = self.machine(report, *selection);
        let frame = machine.and_then(|machine| selected_frame(machine, self.frame));
        match self.tab {
            Tab::Frame => {
                if let Some(frame) = frame {
                    scroll(ui, "frame_details", |ui| {
                        context::frame(ui, frame);
                        if ui.button("Inspect rollback store").clicked() {
                            self.rollback = true;
                            self.tab = Tab::Storage;
                        }
                    });
                } else {
                    ui.label("No frame snapshot for this selection.");
                }
            }
            Tab::Stack => {
                if let Some(frame) = frame {
                    buffers::stack(ui, &frame.stack);
                } else {
                    ui.label("No frame stack available.");
                }
            }
            Tab::Memory | Tab::Calldata | Tab::Returndata => {
                if let Some(frame) = frame {
                    let id = match self.tab {
                        Tab::Memory => frame.memory,
                        Tab::Calldata => frame.calldata,
                        _ => frame.returndata,
                    };
                    buffers::show(ui, report, id, self.tab.label());
                } else {
                    ui.label("No frame byte-array snapshot available.");
                }
            }
            Tab::Storage | Tab::Transient => {
                if frame.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.rollback, false, "Current store");
                        ui.selectable_value(&mut self.rollback, true, "Frame rollback store");
                    });
                    if self.rollback {
                        ui.label(
                            "State saved before this frame entered; a revert restores this store.",
                        );
                    }
                }
                let store = machine.and_then(|machine| {
                    selected_store(&report.stores, machine, frame, self.rollback)
                });
                if machine.is_some() && store.is_none() {
                    ui.colored_label(
                        palette::ERROR,
                        "The report references an unavailable store snapshot",
                    );
                } else if let Some(address) =
                    account.or_else(|| frame.map(|frame| frame.storage_address.as_str()))
                {
                    stores::storage(ui, report, store, address, self.tab == Tab::Transient);
                } else {
                    ui.label("Select a state or account to inspect its storage.");
                }
            }
            Tab::Environment => scroll(ui, "environment_details", |ui| {
                context::environment(ui, report)
            }),
            Tab::Acquisition => scroll(ui, "acquisition_details", |ui| {
                evidence::acquisition(ui, report)
            }),
            Tab::Diagnostics => scroll(ui, "diagnostic_details", |ui| {
                evidence::diagnostics(ui, report, selection)
            }),
            Tab::Outcomes => self.outcomes(ui, report, selection),
        }
    }

    fn outcomes(&mut self, ui: &mut Ui, report: &AnalysisReport, selection: &mut Selection) {
        if report.outcomes.is_empty() {
            ui.label("No terminal root outcome recorded. Check coverage frontiers.");
            return;
        }
        self.outcome = self.outcome.min(report.outcomes.len() - 1);
        let selected = &report.outcomes[self.outcome];
        egui::ComboBox::from_id_salt("outcome_selection")
            .width(ui.available_width().min(340.0))
            .selected_text(notation::widget(
                ui,
                format!(
                    "Outcome {} / {} · {} · {:?}",
                    Symbol::Outcome(self.outcome),
                    report.outcomes.len(),
                    Symbol::State(selected.state),
                    selected.kind
                ),
            ))
            .show_ui(ui, |ui| {
                for (index, outcome) in report.outcomes.iter().enumerate() {
                    if ui
                        .selectable_value(
                            &mut self.outcome,
                            index,
                            notation::widget(
                                ui,
                                format!(
                                    "Outcome {} · {} · {:?}",
                                    Symbol::Outcome(index),
                                    Symbol::State(outcome.state),
                                    outcome.kind
                                ),
                            ),
                        )
                        .clicked()
                    {
                        *selection = Selection {
                            state: Some(outcome.state),
                            pc: None,
                        };
                    }
                }
            });
        let outcome = &report.outcomes[self.outcome];
        scroll(ui, "outcome_details", |ui| {
            if let Some(data) = report.byte_arrays.get(outcome.data) {
                ui.horizontal(|ui| {
                    ui.label("Return/revert length");
                    value::cell(ui, &data.length);
                });
                ui.collapsing("Return/revert bytes", |ui| {
                    ui.allocate_ui(egui::vec2(ui.available_width(), 120.0), |ui| {
                        buffers::show(ui, report, outcome.data, "Outcome bytes")
                    });
                });
            }
            if let Some(store) = report.stores.get(outcome.store) {
                ui.label(RichText::new("Finalized store versus initial observations").strong());
                ui.label(
                    RichText::new(
                        "Abstract differences can include joins and refinements as well as writes.",
                    )
                    .small()
                    .color(palette::MUTED),
                );
                stores::changes(ui, report, store);
            }
        });
    }
}

fn scroll(ui: &mut Ui, id: &str, contents: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::both()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, contents);
}

/// Shared by the status error display and acquisition inspector.
pub(super) fn api_error_details(ui: &mut Ui, error: &ApiError) {
    errors::api(ui, error);
}

fn selected_frame(machine: &MachineSnapshot, index: Option<usize>) -> Option<&FrameSnapshot> {
    index
        .and_then(|index| machine.frames.iter().find(|frame| frame.index == index))
        .or_else(|| machine.frames.last())
}
fn selected_store<'a>(
    stores: &'a [StoreSnapshot],
    machine: &MachineSnapshot,
    frame: Option<&FrameSnapshot>,
    rollback: bool,
) -> Option<&'a StoreSnapshot> {
    let id = if rollback {
        frame?.rollback_store
    } else {
        machine.store
    };
    stores.get(id)
}

#[cfg(test)]
mod tests;
