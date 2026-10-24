//! Display namespaces are independent of the engine's global relation selection.
use super::{Selection, Workspace};
use crate::controls::{self, Density};
use egui::Ui;
use evm_abstract_protocol::{AnalysisNamespace, AnalysisReport, HybridNativeBinding};

pub(super) fn shown(report: &AnalysisReport, namespace: AnalysisNamespace) -> &AnalysisReport {
    report.graph(namespace).unwrap_or(report)
}

pub(super) fn initial(report: &AnalysisReport) -> AnalysisNamespace {
    match &report.hybrid {
        Some(hybrid) if !hybrid.local_prefixes.is_empty() => AnalysisNamespace::Native,
        Some(hybrid) => hybrid.namespace(),
        None => AnalysisNamespace::Native,
    }
}

impl Workspace {
    pub(super) fn model_selector(&mut self, ui: &mut Ui) {
        let Some(report) = &self.report else { return };
        let Some(hybrid) = &report.hybrid else { return };
        let mut next = self.view_namespace;
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
        if next != self.view_namespace {
            self.select_model(next);
        }
    }

    fn select_model(&mut self, namespace: AnalysisNamespace) {
        let Some(report) = &self.report else { return };
        let report = shown(report, namespace);
        self.view_namespace = namespace;
        self.selection = Selection {
            state: report.cfg.first().map(|state| state.id),
            ..Default::default()
        };
        self.program = report
            .cfg
            .first()
            .and_then(|state| state.program)
            .or(report.metadata.root_program);
        self.account = None;
        self.previous_state = self.selection.state;
        self.disasm_focus = Selection::default();
        self.ssa_focus = Selection::default();
        self.ssa_cache.clear();
        self.graph.reset_report();
        self.inspector.reset_report();
    }
}
