//! Native boundary snapshots use one selected state and frame; instruction
//! highlighting never changes the observation into an instruction trace.

use super::{
    Inspector, Point, Selection, Tab, buffers, context, evidence, hybrid, observation, process,
    scroll, selected_frame, selected_store, ssa, stores, summaries, symbolic,
};
use crate::{
    controls::{self, Density},
    palette,
    report::ReportView,
};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{BlockCoverage, MachineSnapshot};

impl Inspector {
    pub(super) fn machine<'a>(
        &self,
        report: &'a ReportView<'_>,
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

    pub(super) fn contents(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
        account: Option<&str>,
    ) {
        let model = report.model;
        if self.last_state != selection.state {
            self.frame = None;
            self.point = Point::Entry;
            self.last_state = selection.state;
            self.rollback = false;
        }
        self.navigation(ui);
        let boundary = matches!(
            self.tab,
            Tab::Frame
                | Tab::Stack
                | Tab::Memory
                | Tab::Calldata
                | Tab::Returndata
                | Tab::Storage
                | Tab::Transient
                | Tab::SsaValue
        );
        if boundary {
            let machine = self.machine(report, *selection);
            observation::show(
                ui,
                report,
                machine.and_then(|machine| selected_frame(machine, self.frame)),
                *selection,
            );
        } else {
            observation::show(ui, report, None, *selection);
        }
        if self.tab == Tab::Outcomes {
            ui.label("Root outcomes · returned data and finalized transaction store");
        } else if self.tab == Tab::Environment {
            ui.label("Original observations · declared input and captured environment");
        }
        if self.tab == Tab::SsaValue {
            ssa::show(ui, report, selection);
            return;
        }
        if self.tab == Tab::Timing {
            scroll(ui, "timing_details", |ui| {
                super::super::timing::show(ui, model, &self.timings);
            });
            return;
        }
        if self.tab == Tab::Diagnostics {
            scroll(ui, "diagnostic_details", |ui| {
                evidence::diagnostics(ui, report, selection)
            });
            return;
        }
        if self.tab == Tab::Coverage {
            scroll(ui, "coverage_details", |ui| {
                hybrid::show(ui, report, *selection)
            });
            return;
        }
        let has_exit = report
            .states
            .iter()
            .find(|state| Some(state.state) == selection.state)
            .is_some_and(|state| state.exit.is_some());
        let coverage = selection
            .state
            .map(|state| crate::widgets::coverage(report, state));
        if boundary {
            Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("inspector_boundary")
                    .width(144.0)
                    .selected_text(match self.point {
                            Point::Entry if report.identity().namespace() == evm_abstract_protocol::AnalysisNamespace::Baseline => "Global · entry observation",
                        Point::Entry => "Native · before block",
                        Point::Exit if !has_exit => "Native · after unavailable",
                            Point::Exit if report.identity().namespace() == evm_abstract_protocol::AnalysisNamespace::Baseline => "Global · retained after",
                        Point::Exit if coverage == Some(BlockCoverage::Current) => "Native · after block",
                        Point::Exit => "Native · historical after",
                    })
                    .show_ui(ui, |ui| {
                        controls::tab_value(ui, &mut self.point, Point::Entry, "Before block · entry snapshot", Density::Compact);
                        ui.add_enabled_ui(has_exit, |ui| {
                            controls::tab_value(ui, &mut self.point, Point::Exit, if coverage == Some(BlockCoverage::Current) { "After block · observed exit" } else { "After block · historical exit" }, Density::Compact);
                        });
                        if !has_exit {
                            ui.label("No after-block snapshot is attached.");
                        }
                    });
                if let Some(machine) = self.machine(report, *selection) {
                    let selected = selected_frame(machine, self.frame).map_or(0, |frame| frame.index);
                    egui::ComboBox::from_id_salt("inspector_frame")
                        .width(98.0)
                        .selected_text(if machine.frames.len() == 1 { "Active frame".into() } else { format!("Frame {}{}", selected + 1, if machine.frames.last().is_some_and(|frame| frame.index == selected) { " · active" } else { " · caller" }) })
                        .show_ui(ui, |ui| {
                            for frame in &machine.frames {
                                controls::tab_value(ui, &mut self.frame, Some(frame.index), format!("Frame {} · {}", frame.index + 1, if machine.frames.last().is_some_and(|active| active.index == frame.index) { "active" } else { "suspended caller" }), Density::Compact);
                            }
                        });
                }
                if let Some(pc) = selection.pc {
                    ui.label(RichText::new(format!("PC 0x{pc:x}")).small().color(palette::MUTED))
                        .on_hover_text("Instruction selection links the views. Inspection shows a block boundary, not a per-instruction memory trace.");
                }
            });
        });
        }
        if boundary && self.point == Point::Exit && coverage != Some(BlockCoverage::Current) {
            ui.colored_label(palette::WARNING,"Historical receipt: current entry facts have changed. This is not a current exit snapshot.");
        }
        ui.separator();
        let machine = self.machine(report, *selection);
        let frame = machine.and_then(|machine| selected_frame(machine, self.frame));
        match self.tab {
            Tab::Frame => {
                if let Some(frame) = frame {
                    scroll(ui, "frame_details", |ui| {
                        if self.point == Point::Entry
                            && summaries::original_input(report, selection.state)
                        {
                            ui.label("Original input entry. Possible execution is described by the process model below.");
                            ui.collapsing("Original input snapshot", |ui| {
                                if let Some(machine) = machine {
                                    symbolic::show(
                                        ui,
                                        report,
                                        machine.path.node,
                                        "Paired path conditions",
                                    );
                                }
                                context::frame(ui, report, frame);
                            });
                        } else {
                            if let Some(machine) = machine {
                                symbolic::show(
                                    ui,
                                    report,
                                    machine.path.node,
                                    "Paired path conditions",
                                );
                            }
                            context::frame(ui, report, frame);
                        }
                        ui.collapsing("Advanced observation identity", |ui| {
                            if let Some(state) = selection.state {
                                ui.monospace(Symbol::State(state).to_string());
                            }
                            ui.monospace(format!("Frame {}", Symbol::Frame(frame.index)));
                            ui.monospace(format!("Publication: {:?}", report.identity()));
                        });
                        summaries::show(ui, report, selection.state);
                        if controls::action(ui, "Inspect rollback store", Density::Regular)
                            .clicked()
                        {
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
                    if frame.process.is_some() {
                        ui.label("Process summaries do not record an instruction stack.");
                    } else {
                        buffers::stack(ui, &frame.stack);
                    }
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
                    if let Some(id) = id {
                        buffers::show(ui, report, id, self.tab.label());
                    } else {
                        ui.label("This process has no instruction byte-array snapshot.");
                    }
                } else {
                    ui.label("No frame byte-array snapshot available.");
                }
            }
            Tab::Storage | Tab::Transient => {
                if frame.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        controls::tab_value(
                            ui,
                            &mut self.rollback,
                            false,
                            "Current store",
                            Density::Compact,
                        );
                        controls::tab_value(
                            ui,
                            &mut self.rollback,
                            true,
                            "Frame rollback store",
                            Density::Compact,
                        );
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
                } else if let Some(address) = account.or_else(|| {
                    frame.and_then(|frame| {
                        let input_owner = frame.index == 0
                            && report.input_namespace().is_some()
                            && frame.storage_address.as_deref() == report.input_namespace();
                        (!input_owner)
                            .then_some(frame.storage_address.as_deref())
                            .flatten()
                    })
                }) {
                    stores::storage(ui, report, store, address, self.tab == Tab::Transient);
                } else {
                    if report.input_namespace().is_some()
                        && frame.is_some_and(|frame| frame.index == 0)
                    {
                        ui.label("Root ADDRESS is symbolic. Inspect the supplied root state under Environment, or select a physical account for its transaction state.");
                        return;
                    }
                    if let Some(key) = frame.and_then(|frame| frame.process.as_ref()) {
                        ui.label(format!(
                            "Physical state class: {}",
                            process::owner(&key.owner)
                        ));
                        ui.label("Inspect the transaction store; this class does not supply a concrete storage address.");
                    } else {
                        ui.label("Select a state or account to inspect its storage.");
                    }
                }
            }
            Tab::Environment => scroll(ui, "environment_details", |ui| {
                context::environment(ui, report)
            }),
            Tab::Acquisition => scroll(ui, "acquisition_details", |ui| {
                evidence::acquisition(ui, report)
            }),
            Tab::Outcomes => self.outcomes(ui, report, selection),
            Tab::SsaValue | Tab::Timing | Tab::Diagnostics | Tab::Coverage => {
                unreachable!("definition, timing and evidence inspection use their own coordinates")
            }
        }
    }
}
