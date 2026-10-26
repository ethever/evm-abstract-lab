//! Display namespaces are independent of the engine's global relation selection.
pub(crate) mod navigation;
pub(crate) mod presentation;

use super::{Selection, Workspace};
use evm_abstract_protocol::{AnalysisNamespace, AnalysisReport};

/// Local report lineage, independent of HTTP task and input revisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReportGeneration(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Representation {
    RichGraph,
    Minimum,
}

/// Numeric state/program/value IDs belong to one graph in one report lineage.
/// A detail-only publication retains the identity of its unchanged global graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReportViewId {
    generation: ReportGeneration,
    namespace: AnalysisNamespace,
    version: Option<evm_abstract_protocol::PublicationVersion>,
    representation: Representation,
}

impl ReportViewId {
    pub(crate) fn generation(self) -> ReportGeneration {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn fixture(generation: u64, namespace: AnalysisNamespace) -> Self {
        Self {
            generation: ReportGeneration(generation),
            namespace,
            version: None,
            representation: Representation::RichGraph,
        }
    }

    pub(crate) fn minimum(
        self,
        version: Option<evm_abstract_protocol::PublicationVersion>,
    ) -> Self {
        Self {
            namespace: AnalysisNamespace::Baseline,
            representation: Representation::Minimum,
            version,
            ..self
        }
    }

    pub(crate) fn is_minimum(self) -> bool {
        self.representation == Representation::Minimum
    }

    pub(crate) fn namespace(self) -> AnalysisNamespace {
        self.namespace
    }

    pub(crate) fn same_report(self, other: Self) -> bool {
        self.generation == other.generation
    }

    pub(crate) fn for_namespace(
        self,
        namespace: AnalysisNamespace,
        publication: Option<&evm_abstract_protocol::JobPublication>,
    ) -> Self {
        Self {
            namespace,
            representation: Representation::RichGraph,
            version: publication.and_then(|publication| match namespace {
                AnalysisNamespace::Baseline => publication.global_version,
                AnalysisNamespace::Native => publication.native_version,
            }),
            ..self
        }
    }
}

impl Default for ReportViewId {
    fn default() -> Self {
        Self {
            generation: ReportGeneration::default(),
            namespace: AnalysisNamespace::Native,
            version: None,
            representation: Representation::RichGraph,
        }
    }
}

#[derive(Default)]
pub(super) struct ReportViewState {
    identity: ReportViewId,
    publication: Option<evm_abstract_protocol::JobPublication>,
    job_id: Option<evm_abstract_protocol::JobId>,
    index: presentation::ReportIndex,
    navigator: navigation::Navigator,
}

impl ReportViewState {
    pub(crate) fn presentation<'a>(
        &'a self,
        report: &'a AnalysisReport,
    ) -> presentation::Presentation<'a> {
        presentation::Presentation::new(
            report,
            &self.index,
            self.identity,
            self.publication.as_ref(),
        )
    }

    #[cfg(test)]
    pub(crate) fn index(&self) -> &presentation::ReportIndex {
        &self.index
    }

    pub(crate) fn navigator(&self) -> &navigation::Navigator {
        &self.navigator
    }

    pub(crate) fn navigator_mut(&mut self) -> &mut navigation::Navigator {
        &mut self.navigator
    }

    pub(crate) fn navigate(
        &mut self,
        report: &AnalysisReport,
        intent: navigation::Intent,
    ) -> Result<Option<navigation::Location>, navigation::NavigationError> {
        let presentation = presentation::Presentation::new(
            report,
            &self.index,
            self.identity,
            self.publication.as_ref(),
        );
        let location = self.navigator.apply(&presentation, intent)?.cloned();
        if let Some(location) = &location {
            self.identity = location.focus.view;
        }
        Ok(location)
    }

    pub(crate) fn synchronize_focus(
        &mut self,
        report: &AnalysisReport,
        focus: navigation::Focus,
    ) -> Result<(), navigation::NavigationError> {
        let presentation = presentation::Presentation::new(
            report,
            &self.index,
            self.identity,
            self.publication.as_ref(),
        );
        let view = focus.view;
        self.navigator.synchronize(&presentation, focus)?;
        self.identity = view;
        Ok(())
    }

    fn refresh_index(&mut self, report: &AnalysisReport) {
        self.index.update(
            report,
            self.identity,
            self.publication.as_ref(),
            self.job_id,
        );
        let presentation = presentation::Presentation::new(
            report,
            &self.index,
            self.identity,
            self.publication.as_ref(),
        );
        self.navigator.reconcile(&presentation, None);
    }

    pub(super) fn publication(&self) -> Option<&evm_abstract_protocol::JobPublication> {
        self.publication.as_ref()
    }

    pub(super) fn identity(&self) -> ReportViewId {
        self.identity
    }

    pub(super) fn minimum<'a>(
        &self,
        report: &'a AnalysisReport,
    ) -> Option<&'a evm_abstract_protocol::MinimumModel> {
        (self.identity.is_minimum())
            .then_some(report.minimum.as_deref())
            .flatten()
    }

    pub(super) fn namespace(&self) -> AnalysisNamespace {
        self.identity.namespace
    }

    pub(super) fn shown<'a>(&'a self, report: &'a AnalysisReport) -> crate::report::ReportView<'a> {
        crate::report::ReportView::new(report, self.identity, self.publication.as_ref())
            .expect("validated active report namespace")
    }
}

#[derive(Clone)]
pub(super) enum Activation {
    Report {
        replacement: bool,
        publication: Option<evm_abstract_protocol::JobPublication>,
        job_id: Option<evm_abstract_protocol::JobId>,
    },
    // Historical namespace-entry oracles use this adapter; user navigation is Focus-based.
    #[cfg(test)]
    Namespace(AnalysisNamespace),
}

impl Workspace {
    /// Typography fixtures mutate source catalogs directly; production accepts
    /// immutable publications through install_report instead.
    #[cfg(test)]
    pub(crate) fn refresh_fixture_index(&mut self) {
        self.report_view.index = presentation::ReportIndex::default();
        if let Some(report) = &self.report {
            self.report_view.refresh_index(report);
        }
    }
}

impl Workspace {
    /// Report arrivals and namespace changes share the same selection transition.
    /// Unchanged graph versions keep selection, caches, camera and gestures.
    pub(super) fn activate_view(&mut self, activation: Activation) {
        let Some(report) = &self.report else { return };
        #[cfg(test)]
        let navigation = matches!(activation, Activation::Namespace(_));
        #[cfg(not(test))]
        let navigation = false;
        if navigation {
            self.report_view.navigator.remember_route(self.view);
        }
        let (namespace, replacement) = match activation {
            Activation::Report {
                replacement,
                publication,
                job_id,
            } => {
                self.report_view.publication = publication;
                self.report_view.job_id = job_id;
                (
                    if replacement || !available(report, self.report_view.namespace()) {
                        report.selected_view
                    } else {
                        self.report_view.namespace()
                    },
                    replacement,
                )
            }
            #[cfg(test)]
            Activation::Namespace(namespace) => (namespace, false),
        };
        if namespace == AnalysisNamespace::Baseline
            && let Some(minimum) = report.minimum.as_deref()
        {
            let mut identity = self.report_view.identity;
            if replacement {
                identity.generation = ReportGeneration(
                    identity
                        .generation
                        .0
                        .checked_add(1)
                        .expect("report generation exhausted"),
                );
            }
            identity = identity.minimum(
                self.report_view
                    .publication
                    .as_ref()
                    .and_then(|publication| publication.minimum_version),
            );
            if identity == self.report_view.identity {
                self.report_view.refresh_index(report);
                return;
            }
            self.report_view.identity = identity;
            self.graph.activate(identity);
            self.ssa_cache.activate(identity);
            self.selection = Selection::default();
            self.previous_state = None;
            self.minimum_selection = super::minimum::Selection::default();
            self.minimum_selection
                .select_summary(minimum, super::minimum::initial_summary(minimum));
            self.program = super::minimum::source_program(report, &minimum.declared_root_source);
            self.account = None;
            self.ssa_focus = Selection::default();
            self.inspector.reset_report();
            self.report_view.refresh_index(report);
            if let Ok(focus) = self.report_view.presentation(report).entry_focus(identity) {
                self.program = focus
                    .source
                    .as_ref()
                    .and_then(navigation::SourceFocus::program);
                if navigation {
                    let _ = self
                        .report_view
                        .navigate(report, navigation::Intent::Open(focus));
                } else {
                    let _ = self.report_view.synchronize_focus(report, focus);
                }
                self.report_view.navigator.remember_route(self.view);
            }
            return;
        }
        let Some(visible) = report.graph(namespace) else {
            return;
        };
        let mut identity = self.report_view.identity;
        if replacement {
            identity.generation = ReportGeneration(
                identity
                    .generation
                    .0
                    .checked_add(1)
                    .expect("report generation exhausted"),
            );
        }
        identity = identity.for_namespace(namespace, self.report_view.publication.as_ref());
        if identity == self.report_view.identity {
            self.report_view.refresh_index(report);
            return;
        }
        self.report_view.identity = identity;
        self.graph.activate(identity);
        self.ssa_cache.activate(identity);
        self.selection = Selection {
            state: visible.cfg.first().map(|state| state.id),
            ..Default::default()
        };
        self.program = visible
            .cfg
            .first()
            .map_or(visible.metadata.root_program, |state| state.program);
        self.account = None;
        self.previous_state = self.selection.state;
        self.ssa_focus = Selection::default();
        self.inspector.reset_report();
        self.report_view.refresh_index(report);
        if let Ok(focus) = self.report_view.presentation(report).entry_focus(identity) {
            self.program = focus
                .source
                .as_ref()
                .and_then(navigation::SourceFocus::program);
            if navigation {
                let _ = self
                    .report_view
                    .navigate(report, navigation::Intent::Open(focus));
            } else {
                let _ = self.report_view.synchronize_focus(report, focus);
            }
            self.report_view.navigator.remember_route(self.view);
        }
    }
}

fn available(report: &AnalysisReport, namespace: AnalysisNamespace) -> bool {
    report.graph(namespace).is_some()
        || (namespace == AnalysisNamespace::Baseline && report.minimum.is_some())
}

#[cfg(test)]
mod tests;
