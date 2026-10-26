//! The shell owns navigation. Inspecting borrows a qualified publication and a
//! selection snapshot; only an explicit pick returns a selection change.

use super::{Inspector, Selection};
use crate::{app::minimum, report::ReportView};
use egui::Ui;
use evm_abstract_protocol::{AnalysisReport, MinimumModel, ProcessSource};

pub(in crate::app) enum InspectionSource<'a> {
    Program(usize),
    Process(&'a ProcessSource),
}

pub(in crate::app) enum InspectionTarget<'a, 'report> {
    Source {
        report: &'a AnalysisReport,
        source: InspectionSource<'a>,
    },
    Native {
        report: &'a ReportView<'report>,
        selection: Selection,
        account: Option<&'a str>,
    },
    Global {
        report: &'a AnalysisReport,
        model: &'a MinimumModel,
        selection: minimum::Selection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum InspectionAction {
    Native(Selection),
    Global(minimum::Selection),
}

impl Inspector {
    /// The caller resolves the selection in the target's exact report namespace.
    /// The inspector never mutates a report or creates a Native coordinate from
    /// a conservative source location.
    pub(in crate::app) fn inspect(
        &mut self,
        ui: &mut Ui,
        target: InspectionTarget<'_, '_>,
    ) -> Option<InspectionAction> {
        let rect = ui.available_rect_before_wrap();
        let mut contents = ui.new_child(
            egui::UiBuilder::new()
                .id(egui::Id::new("inspector_contents"))
                .max_rect(rect),
        );
        contents.set_clip_rect(contents.clip_rect().intersect(rect));
        let action = match target {
            InspectionTarget::Source { report, source } => {
                self.source_contents(&mut contents, report, source);
                None
            }
            InspectionTarget::Native {
                report,
                selection,
                account,
            } => {
                let mut next = selection;
                self.contents(&mut contents, report, &mut next, account);
                (next != selection).then_some(InspectionAction::Native(next))
            }
            InspectionTarget::Global {
                report,
                model,
                selection,
            } => {
                let mut next = selection;
                self.minimum_contents(&mut contents, report, model, &mut next);
                (next != selection).then_some(InspectionAction::Global(next))
            }
        };
        // Content minimums cannot enlarge the user's selected panel rectangle.
        ui.advance_cursor_after_rect(rect);
        action
    }
}
