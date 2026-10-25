//! Display namespaces are independent of the engine's global relation selection.
use super::{Selection, Workspace};
use crate::controls::{self, Density};
use egui::Ui;
use evm_abstract_protocol::{AnalysisNamespace, AnalysisReport, HybridNativeBinding};

/// Local report lineage, independent of HTTP task and input revisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReportGeneration(u64);

/// Numeric state/program/value IDs belong to one graph in one report lineage.
/// A detail-only publication retains the identity of its unchanged global graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReportViewId {
    generation: ReportGeneration,
    namespace: AnalysisNamespace,
    version: Option<evm_abstract_protocol::PublicationVersion>,
}

impl ReportViewId {
    #[cfg(test)]
    pub(crate) fn fixture(generation: u64, namespace: AnalysisNamespace) -> Self {
        Self {
            generation: ReportGeneration(generation),
            namespace,
            version: None,
        }
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
        }
    }
}

#[derive(Default)]
pub(super) struct ReportViewState {
    identity: ReportViewId,
    publication: Option<evm_abstract_protocol::JobPublication>,
}

impl ReportViewState {
    pub(super) fn publication(&self) -> Option<&evm_abstract_protocol::JobPublication> {
        self.publication.as_ref()
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
    },
    Namespace(AnalysisNamespace),
}

impl Workspace {
    pub(super) fn model_selector(&mut self, ui: &mut Ui) {
        let Some(report) = &self.report else { return };
        let Some(hybrid) = &report.hybrid else { return };
        let mut next = self.report_view.namespace();
        ui.horizontal_wrapped(|ui| {
            controls::tab_value(ui, &mut next, AnalysisNamespace::Baseline, "Global model", Density::Compact);
            ui.add_enabled_ui(report.graph(AnalysisNamespace::Native).is_some(), |ui| {
                controls::tab_value(ui, &mut next, AnalysisNamespace::Native, "Detailed paths", Density::Compact);
            });
            if next == AnalysisNamespace::Native {
                if let HybridNativeBinding::SymbolicOwnerNamespace(binding) = &hybrid.native_coverage().detail_binding {
                    ui.label(format!("Root ADDRESS = {} for these paths; the global model retains other addresses.", binding.namespace));
                }
                if hybrid.rpc.as_ref().is_some_and(|rpc| !rpc.detail_at_current_revision) {
                    ui.label("Detailed paths retain earlier input facts.");
                }
            }
        });
        if next != self.report_view.namespace() {
            self.select_model(next);
        }
    }

    fn select_model(&mut self, namespace: AnalysisNamespace) {
        self.activate_view(Activation::Namespace(namespace));
    }

    /// Report arrivals and namespace changes share the same selection transition.
    /// Unchanged graph versions keep selection, caches, camera and gestures.
    pub(super) fn activate_view(&mut self, activation: Activation) {
        let Some(report) = &self.report else { return };
        let (namespace, replacement) = match activation {
            Activation::Report {
                replacement,
                publication,
            } => {
                self.report_view.publication = publication;
                (
                    if replacement || report.graph(self.report_view.namespace()).is_none() {
                        report.selected_view
                    } else {
                        self.report_view.namespace()
                    },
                    replacement,
                )
            }
            Activation::Namespace(namespace) => (namespace, false),
        };
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
        self.disasm_focus = Selection::default();
        self.ssa_focus = Selection::default();
        self.inspector.reset_report();
        if replacement {
            self.directory.open = visible.metadata.snapshot.is_some() || report.programs.len() > 1;
        }
    }
}

#[cfg(test)]
mod tests;
