//! One qualified location and its history. Renderers propose locations; this
//! module validates them against the immutable presentation before changing state.

use super::{ReportGeneration, ReportViewId, presentation::Presentation};
use evm_abstract_protocol::{ProcessLocation, ProcessSource};

/// A catalog row retains its physical/source alias, even when bytes are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceId {
    pub(crate) report: ReportGeneration,
    pub(crate) program: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SourceFocus {
    Captured(SourceId),
    Uncaptured {
        report: ReportGeneration,
        source: ProcessSource,
    },
}

impl SourceFocus {
    pub(crate) fn program(&self) -> Option<usize> {
        match self {
            Self::Captured(id) => Some(id.program),
            Self::Uncaptured { .. } => None,
        }
    }

    pub(crate) fn generation(&self) -> ReportGeneration {
        match self {
            Self::Captured(id) => id.report,
            Self::Uncaptured { report, .. } => *report,
        }
    }
}

/// These are occurrence references, never a join of owner/caller/static facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextFocus {
    Minimum { summary: usize, procedure: usize },
    Graph { state: usize },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MinimumEvidence {
    pub(crate) node: Option<usize>,
    pub(crate) control: Option<ProcessLocation>,
    pub(crate) output: Option<usize>,
    pub(crate) account: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceFocus {
    /// Decoded source is independent of reachability or instruction evidence.
    Source {
        block: Option<usize>,
        pc: Option<usize>,
    },
    Minimum(MinimumEvidence),
    Graph(crate::app::Selection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Focus {
    pub(crate) view: ReportViewId,
    pub(crate) source: Option<SourceFocus>,
    pub(crate) context: Option<ContextFocus>,
    pub(crate) evidence: EvidenceFocus,
}

impl Focus {
    pub(crate) fn source(view: ReportViewId, source: SourceFocus) -> Self {
        Self {
            view,
            source: Some(source),
            context: None,
            evidence: EvidenceFocus::Source {
                block: None,
                pc: None,
            },
        }
    }

    pub(crate) fn graph_selection(&self) -> Option<crate::app::Selection> {
        match self.evidence {
            EvidenceFocus::Graph(selection) => Some(selection),
            _ => None,
        }
    }

    pub(crate) fn minimum_selection(&self) -> Option<crate::app::minimum::Selection> {
        let ContextFocus::Minimum { summary, procedure } = self.context? else {
            return None;
        };
        let EvidenceFocus::Minimum(evidence) = self.evidence else {
            return None;
        };
        Some(crate::app::minimum::Selection {
            summary,
            procedure,
            node: evidence.node,
            control: evidence.control,
            output: evidence.output,
            account: evidence.account,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CameraBookmark {
    pub(crate) zoom: f32,
    pub(crate) pan: [f32; 2],
}

impl CameraBookmark {
    pub(crate) fn new(zoom: f32, pan: [f32; 2]) -> Option<Self> {
        (zoom.is_finite() && zoom > 0.0 && pan.iter().all(|value| value.is_finite()))
            .then_some(Self { zoom, pan })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Location {
    pub(crate) focus: Focus,
    pub(crate) camera: Option<CameraBookmark>,
    /// Snapshot of the shell's existing route, restored by Back/Forward.
    pub(crate) route: crate::app::View,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Open(Focus),
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NavigationError {
    #[error("This location belongs to an earlier result")]
    StaleReport,
    #[error("This evidence belongs to a different model publication")]
    StaleView,
    #[error("Source is unavailable in this result")]
    MissingSource,
    #[error("The source does not match this execution context")]
    SourceMismatch,
    #[error("This context is unavailable in the selected model")]
    MissingContext,
    #[error("The evidence does not belong to this context")]
    ContextMismatch,
    #[error("This instruction, value or outcome has no matching evidence")]
    MissingEvidence,
}

#[derive(Default)]
pub(crate) struct Navigator {
    current: Option<Location>,
    back: Vec<Location>,
    forward: Vec<Location>,
}

impl Navigator {
    pub(crate) fn current(&self) -> Option<&Location> {
        self.current.as_ref()
    }

    pub(crate) fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub(crate) fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub(crate) fn remember_route(&mut self, route: crate::app::View) {
        if let Some(current) = &mut self.current {
            current.route = route;
        }
    }

    /// Camera updates do not add history; a call/source navigation snapshots it.
    pub(crate) fn remember_camera(&mut self, camera: CameraBookmark) {
        if CameraBookmark::new(camera.zoom, camera.pan).is_some()
            && let Some(current) = &mut self.current
        {
            current.camera = Some(camera);
        }
    }

    pub(crate) fn apply(
        &mut self,
        presentation: &Presentation<'_>,
        intent: Intent,
    ) -> Result<Option<&Location>, NavigationError> {
        match intent {
            Intent::Open(focus) => {
                presentation.validate(&focus)?;
                if self
                    .current
                    .as_ref()
                    .is_some_and(|location| location.focus == focus)
                {
                    return Ok(self.current.as_ref());
                }
                let route = self
                    .current
                    .as_ref()
                    .map_or(crate::app::View::Graph, |current| current.route);
                if let Some(current) = self.current.take() {
                    self.back.push(current);
                }
                self.current = Some(Location {
                    focus,
                    camera: None,
                    route,
                });
                self.forward.clear();
            }
            Intent::Back => {
                if let Some(previous) = pop_valid(&mut self.back, presentation)
                    && let Some(current) = self.current.replace(previous)
                {
                    self.forward.push(current);
                }
            }
            Intent::Forward => {
                if let Some(next) = pop_valid(&mut self.forward, presentation)
                    && let Some(current) = self.current.replace(next)
                {
                    self.back.push(current);
                }
            }
        }
        Ok(self.current.as_ref())
    }

    /// Legacy widget adapters update a location without creating a breadcrumb
    /// for every instruction/value click. Source/context changes use Open.
    pub(crate) fn synchronize(
        &mut self,
        presentation: &Presentation<'_>,
        focus: Focus,
    ) -> Result<(), NavigationError> {
        presentation.validate(&focus)?;
        let camera = self
            .current
            .as_ref()
            .filter(|old| {
                old.focus.view == focus.view
                    && old.focus.context == focus.context
                    && old.focus.source == focus.source
            })
            .and_then(|old| old.camera);
        let route = self
            .current
            .as_ref()
            .map_or(crate::app::View::Graph, |current| current.route);
        self.current = Some(Location {
            focus,
            camera,
            route,
        });
        Ok(())
    }

    /// An optional richer publication can add sources/Native data without
    /// changing the selected Global graph. Keep all still-qualified locations.
    pub(crate) fn reconcile(&mut self, presentation: &Presentation<'_>, fallback: Option<Focus>) {
        self.back
            .retain(|location| presentation.validate(&location.focus).is_ok());
        self.forward
            .retain(|location| presentation.validate(&location.focus).is_ok());
        if self
            .current
            .as_ref()
            .is_some_and(|location| presentation.validate(&location.focus).is_ok())
        {
            return;
        }
        self.current = fallback
            .filter(|focus| presentation.validate(focus).is_ok())
            .map(|focus| Location {
                focus,
                camera: None,
                route: crate::app::View::Graph,
            });
    }
}

fn pop_valid(history: &mut Vec<Location>, presentation: &Presentation<'_>) -> Option<Location> {
    while let Some(location) = history.pop() {
        if presentation.validate(&location.focus).is_ok() {
            return Some(location);
        }
    }
    None
}

#[cfg(test)]
#[path = "navigation/tests.rs"]
mod tests;
