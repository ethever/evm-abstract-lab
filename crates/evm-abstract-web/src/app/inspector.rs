//! Resizable world inspector. Entry, historical exit and finalized outcomes are
//! explicit distinct observations, never an invented per-instruction trace.
use crate::report::ReportView;

mod buffers;
mod context;
mod errors;
mod evidence;
mod hybrid;
mod minimum;
mod native;
mod navigation;
mod observation;
mod original;
mod outcomes;
mod panel;
mod process;
mod selection;
pub(crate) mod shortcut;
mod source;
mod ssa;
mod stores;
mod summaries;
mod symbolic;
mod terminal;
mod value;

use super::Selection;
use crate::controls::{self, Density};
use egui::Ui;
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{
    ApiError, BlockCoverage, FrameSnapshot, MachineSnapshot, StoreSnapshot,
};
pub(in crate::app) use selection::{
    InspectionAction, InspectionSource, InspectionTarget, OriginalAccount,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Frame,
    Stack,
    SsaValue,
    Memory,
    Storage,
    Transient,
    Calldata,
    Returndata,
    Outcomes,
    Acquisition,
    Environment,
    Timing,
    Diagnostics,
    Coverage,
}

impl Tab {
    const ALL: [(Self, &'static str); 14] = [
        (Self::Coverage, "Coverage"),
        (Self::Frame, "Frame"),
        (Self::Stack, "Stack"),
        (Self::SsaValue, "SSA value"),
        (Self::Memory, "Memory"),
        (Self::Storage, "Storage"),
        (Self::Transient, "Transient"),
        (Self::Calldata, "Calldata"),
        (Self::Returndata, "Returndata"),
        (Self::Outcomes, "Outcomes"),
        (Self::Acquisition, "Acquisition"),
        (Self::Environment, "Environment"),
        (Self::Timing, "Timing"),
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
    // Visibility is orthogonal to docked/maximized presentation so reopening
    // restores the last mode. Chrome may toggle it without resetting content.
    pub(super) expanded: bool,
    pub(super) tab: Tab,
    point: Point,
    frame: Option<usize>,
    last_state: Option<usize>,
    last_value: Option<usize>,
    last_minimum: Option<super::minimum::Selection>,
    last_minimum_tab: Option<Tab>,
    outcome: usize,
    rollback: bool,
    scope: minimum::Scope,
    panel: panel::Panel,
    pub(super) timings: super::timing::Details,
    #[cfg(not(target_arch = "wasm32"))]
    shortcut: shortcut::Shortcut,
}
impl Default for Inspector {
    fn default() -> Self {
        Self {
            expanded: true,
            tab: Tab::Frame,
            point: Point::Entry,
            frame: None,
            last_state: None,
            last_value: None,
            last_minimum: None,
            last_minimum_tab: None,
            outcome: 0,
            rollback: false,
            scope: minimum::Scope::default(),
            panel: panel::Panel::default(),
            timings: super::timing::Details::default(),
            #[cfg(not(target_arch = "wasm32"))]
            shortcut: shortcut::Shortcut::default(),
        }
    }
}

impl Inspector {
    pub(super) fn maximized(&self) -> bool {
        self.expanded && self.panel.maximized()
    }

    pub(super) fn docked_panel(&self, ui: &Ui, workspace_height: f32) -> egui::Panel {
        self.panel.docked(ui, workspace_height)
    }

    pub(super) fn remember_resize(&mut self, ui: &Ui, rect: egui::Rect) {
        self.panel.remember_resize(ui, rect);
    }

    pub(super) fn visibility_hint(&self, ctx: &egui::Context) -> &'static str {
        shortcut::hint(ctx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn handle_shortcut(&mut self, ctx: &egui::Context, available: bool) {
        if self.shortcut.consume(ctx, available) % 2 == 1 {
            self.expanded = !self.expanded;
        }
    }

    pub(super) fn accessible_summary(
        &self,
        report: &ReportView<'_>,
        selection: Selection,
    ) -> String {
        if !self.expanded {
            return format!("Inspector hidden · {}", self.tab.label());
        }
        let mode = if self.panel.maximized() {
            " · Maximized"
        } else {
            ""
        };
        if self.tab == Tab::SsaValue {
            return format!("{}{mode}", ssa::summary(report, selection));
        }
        if self.tab == Tab::Timing {
            return format!("Inspector: Timing{mode}");
        }
        if self.tab == Tab::Coverage {
            return format!(
                "Inspector: Coverage{mode} · {}",
                super::hybrid::summary(report.model).unwrap_or_else(|| "Native analysis".into())
            );
        }
        if self.tab == Tab::Acquisition {
            return format!(
                "Inspector: Acquisition{mode} · {}",
                evidence::acquisition_summary(report)
            );
        }
        if self.tab == Tab::Diagnostics {
            return format!(
                "Inspector: Diagnostics{mode} · {}",
                evidence::diagnostic_summary(report)
            );
        }
        if self.tab == Tab::Outcomes {
            let coverage = super::terminal_coverage::status(report.model)
                .map_or(String::new(), |status| format!(" · {status}"));
            let alternatives = if report.root_terminal_coverage.is_some() {
                " · Return alternative · Revert alternative · Failure alternative"
            } else {
                ""
            };
            return format!(
                "Inspector: Outcomes{mode}{coverage}{alternatives} · {} observed outcomes",
                report.outcomes.len()
            );
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
        let mut interactions = String::new();
        let input = if self.tab == Tab::Frame
            && self.point == Point::Entry
            && summaries::original_input(report, selection.state)
        {
            " · Original input entry"
        } else {
            ""
        };
        if self.tab == Tab::Frame
            && let Some(summary) = report
                .abstract_summaries
                .iter()
                .rev()
                .find(|summary| Some(summary.source_state) == selection.state)
        {
            interactions = format!(
                " · {} · {}",
                summaries::label(summary.kind),
                summaries::coverage_label(summary.closed, summary.source_status(false))
            );
        }
        format!(
            "Inspector: {} · {boundary} · {frame}{store}{input}{interactions}{mode}",
            self.tab.label()
        )
    }

    /// Report-local IDs can repeat; reset observation coordinates, not UI preferences.
    pub(super) fn reset_report(&mut self) {
        self.point = Point::Entry;
        self.frame = None;
        self.last_state = None;
        self.last_value = None;
        self.last_minimum = None;
        self.last_minimum_tab = None;
        self.outcome = 0;
        self.rollback = false;
    }

    pub(super) fn select_value(&mut self, value: Option<usize>) {
        if self.last_value != value {
            self.last_value = value;
            if value.is_some() {
                self.expanded = true;
                self.tab = Tab::SsaValue;
            }
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
        account: Option<&str>,
    ) {
        if selection.state.is_none()
            && selection.value.is_none()
            && let Some(address) = account
        {
            self.inspect(
                ui,
                InspectionTarget::Original {
                    report: report.model,
                    account: OriginalAccount::Address(address),
                },
            );
            return;
        }
        // The legacy shell has no typed source focus. With no observation at
        // all, its root source is still inspectable without choosing a state.
        if selection.state.is_none()
            && selection.value.is_none()
            && account.is_none()
            && self.tab == Tab::Frame
        {
            let source = report.metadata.root_program.map_or(
                InspectionSource::Process(&report.metadata.declared_root_source),
                InspectionSource::Program,
            );
            self.inspect(
                ui,
                InspectionTarget::Source {
                    report: report.model,
                    source,
                },
            );
            return;
        }
        if let Some(InspectionAction::Native(next)) = self.inspect(
            ui,
            InspectionTarget::Native {
                report,
                selection: *selection,
                account,
            },
        ) {
            *selection = next;
        }
    }

    fn window_controls(&mut self, ui: &mut Ui) {
        if controls::action(ui, "Hide", Density::Compact)
            .on_hover_text(self.visibility_hint(ui.ctx()))
            .clicked()
        {
            self.expanded = false;
        }
        if controls::action(
            ui,
            if self.panel.maximized() {
                "Restore"
            } else {
                "Maximize"
            },
            Density::Compact,
        )
        .on_hover_text(if self.panel.maximized() {
            "Restore the previous Inspect height"
        } else {
            "Use the result workspace for Inspect"
        })
        .clicked()
        {
            self.panel.toggle_maximized();
            ui.ctx().request_repaint();
        }
    }
}

fn scroll(ui: &mut Ui, id: &str, contents: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::both()
        .id_salt(id)
        .min_scrolled_height(0.0)
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
