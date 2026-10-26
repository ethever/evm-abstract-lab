//! Canonical navigation is owned by ReportViewState. Legacy selections are
//! frame-local adapters for existing graph and inspector event boundaries.

use crate::app::model::navigation::{
    CameraBookmark, ContextFocus, EvidenceFocus, Focus, Intent, Location, NavigationError,
};
use crate::app::{Selection, View, Workspace, minimum};

impl Workspace {
    pub(in crate::app) fn open_native_selection(&mut self, selection: Selection, route: View) {
        let Some(report) = &self.report else { return };
        match self.report_view.presentation(report).graph_focus(
            self.report_view.identity(),
            selection,
            None,
        ) {
            Ok(focus) => self.navigate(Intent::Open(focus), Some(route)),
            Err(error) => self.navigation_error = Some(error),
        }
    }

    pub(in crate::app) fn remember_location(&mut self) {
        let (zoom, pan) = self.graph.camera();
        if let Some(camera) = CameraBookmark::new(zoom, [pan.x, pan.y]) {
            self.report_view.navigator_mut().remember_camera(camera);
        }
        self.report_view.navigator_mut().remember_route(self.view);
    }

    pub(in crate::app) fn navigate(&mut self, intent: Intent, route: Option<View>) {
        if let Intent::Open(focus) = &intent
            && let Some(report) = &self.report
            && let Err(error) = self.report_view.presentation(report).validate(focus)
        {
            self.reject_focus(error);
            return;
        }
        let previous = self.report_view.identity();
        self.remember_location();
        let result = self
            .report
            .as_ref()
            .map(|report| self.report_view.navigate(report, intent));
        match result {
            Some(Ok(Some(location))) => {
                if previous != location.focus.view {
                    self.inspector.reset_report();
                    self.ssa_focus = Selection::default();
                }
                self.view = route.unwrap_or(location.route);
                self.apply_location(location);
                self.report_view.navigator_mut().remember_route(self.view);
                self.navigation_error = None;
            }
            Some(Err(error)) => self.reject_focus(error),
            _ => {}
        }
    }

    fn apply_location(&mut self, location: Location) {
        self.apply_focus(&location.focus);
        // Opening a location follows its source. Selecting within the current
        // scene keeps the user's filter and any open Native instance picker.
        self.graph.set_program_scope(self.program);
        if let Some(camera) = location.camera {
            self.graph
                .restore_camera(camera.zoom, egui::vec2(camera.pan[0], camera.pan[1]));
        } else {
            self.graph.focus_selection();
        }
    }

    fn apply_focus(&mut self, focus: &Focus) {
        self.graph.activate(focus.view);
        self.ssa_cache.activate(focus.view);
        self.program = focus.source.as_ref().and_then(|source| source.program());
        self.account = None;
        self.original_inspection = None;
        self.restore_adapters(focus);
    }

    fn restore_adapters(&mut self, focus: &Focus) {
        self.program = focus.source.as_ref().and_then(|source| source.program());
        // A source PC never becomes a Native PC. A retained context can still
        // supply its own state/SSA view, independently of the decoded row.
        self.selection = focus
            .graph_selection()
            .unwrap_or_else(|| match focus.context {
                Some(ContextFocus::Graph { state }) => Selection {
                    state: Some(state),
                    ..Default::default()
                },
                _ => Selection::default(),
            });
        self.minimum_selection = focus
            .minimum_selection()
            .unwrap_or_else(|| match focus.context {
                Some(ContextFocus::Minimum { summary, procedure }) => minimum::Selection {
                    summary,
                    procedure,
                    ..Default::default()
                },
                _ => minimum::Selection::default(),
            });
        self.previous_state = self.selection.state;
        self.inspector.select_value(self.selection.value);
    }

    pub(in crate::app) fn accept_focus(&mut self, focus: Focus) {
        let previous = self.report_view.identity();
        let Some(report) = &self.report else { return };
        match self.report_view.synchronize_focus(report, focus.clone()) {
            Ok(()) => {
                if previous != focus.view {
                    self.inspector.reset_report();
                    self.ssa_focus = Selection::default();
                }
                self.apply_focus(&focus);
                self.navigation_error = None;
            }
            Err(error) => self.reject_focus(error),
        }
    }

    pub(in crate::app) fn accept_native_selection(&mut self, selection: Selection) {
        let Some(report) = &self.report else { return };
        let focus = self.report_view.presentation(report).graph_focus(
            self.report_view.identity(),
            selection,
            selection.state.is_none().then_some(self.program).flatten(),
        );
        self.accept_resolved_focus(focus);
    }

    pub(in crate::app) fn accept_minimum_selection(&mut self, selection: minimum::Selection) {
        let Some(report) = &self.report else { return };
        let focus = self.report_view.presentation(report).minimum_focus(
            self.report_view.identity(),
            selection,
            None,
        );
        self.accept_resolved_focus(focus);
    }

    fn accept_resolved_focus(&mut self, focus: Result<Focus, NavigationError>) {
        match focus {
            Ok(focus) => self.accept_focus(focus),
            Err(error) => self.reject_focus(error),
        }
    }

    fn reject_focus(&mut self, error: NavigationError) {
        if let Some(focus) = self
            .report_view
            .navigator()
            .current()
            .map(|location| location.focus.clone())
        {
            self.restore_adapters(&focus);
        }
        self.navigation_error = Some(error);
    }

    /// Direct writes are kept only at the legacy widget/test boundary. A
    /// decoded-only focus is never overwritten by an unrelated stale adapter.
    pub(in crate::app) fn reconcile_widget_selection(&mut self) {
        let Some(current) = self.report_view.navigator().current() else {
            return;
        };
        match current.focus.evidence {
            EvidenceFocus::Graph(selection) if selection != self.selection => {
                self.accept_native_selection(self.selection);
            }
            EvidenceFocus::Minimum(_)
                if current.focus.minimum_selection() != Some(self.minimum_selection) =>
            {
                self.accept_minimum_selection(self.minimum_selection);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
