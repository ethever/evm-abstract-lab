//! Responsive defaults apply once to a fresh entry CFG. Resizing preserves
//! explicit inspector preferences; compact panels share one content lane.

use super::super::model::ReportViewId;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(in super::super) struct PanelLayout {
    report: Option<ReportViewId>,
    compact: bool,
}

impl PanelLayout {
    pub(in super::super) fn compact(&self) -> bool {
        self.compact
    }

    pub(in super::super) fn prepare(
        &mut self,
        viewport: egui::Vec2,
        report: ReportViewId,
        entry_cfg: bool,
        sources: &mut bool,
        inspector: &mut bool,
    ) {
        let compact = viewport.x < 680.0 || viewport.y < 560.0;
        let replacement = self
            .report
            .is_none_or(|previous| !previous.same_report(report));
        if compact && replacement && entry_cfg {
            // Opening a result must leave a usable graph on a phone. Sources
            // and Inspect remain explicit, anchored choices in the header.
            *sources = false;
            *inspector = false;
        } else if compact && *sources && *inspector {
            *sources = false;
        }
        self.compact = compact;
        self.report = Some(report);
    }
}
