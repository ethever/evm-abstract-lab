//! Inspect either graph without mixing its state IDs or dropping coarse routes.
use super::{buffers, context, symbolic};
use crate::{
    app::{Selection, hybrid},
    controls::{self, Density},
    palette, widgets,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::{
    AnalysisNamespace, AnalysisReport, HybridLocalScope, InstructionProgress,
};

pub(super) struct Detail {
    graph: widgets::Graph,
    namespace: AnalysisNamespace,
    selection: Selection,
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            graph: widgets::Graph::default(),
            namespace: AnalysisNamespace::Native,
            selection: Selection::default(),
        }
    }
}

impl Detail {
    pub(super) fn reset(&mut self) {
        self.graph.reset_report();
        self.selection = Selection::default();
    }

    pub(super) fn show(&mut self, ui: &mut Ui, report: &AnalysisReport) {
        let Some(evidence) = &report.hybrid else {
            ui.label("This report contains a single native analysis.");
            return;
        };
        let precision = evidence.native_coverage();
        ui.label(RichText::new(hybrid::summary(report).unwrap_or_default()).strong());
        ui.label("The global model keeps all covered call, callback and creation routes. Detailed paths describe their recorded inputs; pending paths remain covered by the global model.");
        ui.label(format!("{} code sources retain jump contexts; other code uses joined contexts. More analysis work can extend the detailed observations.", precision.detail_regions.len()));
        if let Some(rpc) = &evidence.rpc {
            ui.label(format!(
                "Original facts {} · current facts {} · optional precision {:?}",
                rpc.baseline_revision, rpc.current_revision, rpc.precision
            ));
            if !rpc.detail_at_current_revision && rpc.detail_revision.is_some() {
                ui.colored_label(palette::WARNING, "Detailed paths retain earlier input facts. New observations have not refreshed their execution receipts.");
            }
            if rpc.baseline_relation
                == evm_abstract_protocol::RpcBaselineRelation::InitialFactRestriction
            {
                ui.label("New observations restrict the original input set. Its closed global model covers that subset; its source receipts keep their original fact revision.");
            }
            for failure in &rpc.failures {
                ui.colored_label(palette::WARNING, &failure.message);
            }
            if let Some(failure) = &rpc.preparation_failure {
                super::errors::api(ui, failure);
            }
        }
        ui.collapsing("Context sources and work", |ui| {
            for region in &precision.detail_regions {
                ui.monospace(format!("{} · {}", region.code_address.as_deref().unwrap_or("Supplied input program"), region.code_hash));
            }
            let work = &precision.work;
            ui.label(format!("Work: {} total · {} earlier · {} global · {} preparation · {} detail / {} allowance",
                work.total, work.prior, work.baseline, work.preparation, work.detail, work.detail_allowance));
            if let Some(rpc) = &evidence.rpc {
                ui.label(format!("Session work {} · optional work {} / {}", rpc.work, rpc.precision_work, rpc.max_precision_work));
            }
        });
        let before = self.namespace;
        ui.horizontal_wrapped(|ui| {
            controls::tab_value(
                ui,
                &mut self.namespace,
                AnalysisNamespace::Baseline,
                "Global model",
                Density::Compact,
            );
            ui.add_enabled_ui(report.graph(AnalysisNamespace::Native).is_some(), |ui| {
                controls::tab_value(
                    ui,
                    &mut self.namespace,
                    AnalysisNamespace::Native,
                    "Detailed paths",
                    Density::Compact,
                );
            });
        });
        if self.namespace != before {
            self.reset();
        }
        let Some(graph) = report.graph(self.namespace) else {
            ui.label("Detailed paths are not available yet. Global coverage is retained.");
            return;
        };
        if self.namespace == AnalysisNamespace::Native && !graph.frontiers.is_empty() {
            egui::CollapsingHeader::new(format!(
                "Pending detailed paths: {}",
                graph.frontiers.len()
            ))
            .default_open(graph.cfg.is_empty())
            .show(ui, |ui| {
                for frontier in &graph.frontiers {
                    ui.colored_label(
                        palette::WARNING,
                        format!(
                            "State {:?} · {:?}: {}",
                            frontier.from, frontier.kind, frontier.detail
                        ),
                    );
                }
            });
        }
        if self.selection.state.is_none() {
            self.selection.state = graph.cfg.first().map(|state| state.id);
        }
        ui.push_id("hybrid_graph_namespace", |ui| {
            ui.allocate_ui(egui::vec2(ui.available_width(), 320.0), |ui| {
                self.graph.show(ui, graph, &mut self.selection);
            });
        });
        let Some(id) = self.selection.state else {
            return;
        };
        ui.label(
            RichText::new(format!(
                "{} state {id}",
                match self.namespace {
                    AnalysisNamespace::Baseline => "Global",
                    AnalysisNamespace::Native => "Detailed",
                }
            ))
            .strong(),
        );
        if self.namespace == AnalysisNamespace::Baseline {
            super::summaries::show(ui, graph, Some(id));
            return;
        }
        let prefix = evidence
            .local_prefixes
            .iter()
            .find(|prefix| prefix.state == id);
        if let Some(prefix) = prefix {
            ui.label(match prefix.scope {
                HybridLocalScope::RetainedInputPrefix => "Current prefix for this recorded abstract input. Other incoming paths can still grow the input.",
                HybridLocalScope::NativeOwnerNamespacePrefix => "Prefix under the additional root address condition below. The global model also covers other possible root addresses.",
            });
            if let evm_abstract_protocol::HybridNativeBinding::SymbolicOwnerNamespace(binding) =
                &prefix.binding
            {
                ui.label(format!(
                    "Additional condition: root ADDRESS = {}",
                    binding.namespace
                ));
            }
            ui.label(if prefix.context_selected {
                "Selected source · jump contexts retained"
            } else {
                "Unselected source · jump contexts joined"
            });
            ui.collapsing("Source and input identity", |ui| {
                ui.monospace(format!(
                    "{} · facts {}",
                    snapshot(&prefix.snapshot),
                    prefix.revision
                ));
                ui.monospace(format!(
                    "Code: {} · hash {}",
                    prefix
                        .frame
                        .code_address
                        .as_deref()
                        .unwrap_or("not recorded"),
                    prefix.frame.code_hash.as_deref().unwrap_or("not recorded")
                ));
                ui.monospace(format!(
                    "Storage: {} · jump context {:?}",
                    prefix
                        .frame
                        .storage_address
                        .as_deref()
                        .unwrap_or("not recorded"),
                    prefix.frame.context
                ));
            });
            for instruction in &prefix.instructions {
                ui.label(format!(
                    "PC 0x{:x} · {}",
                    instruction.pc,
                    phase(instruction.phase)
                ));
            }
            ui.label(format!("{} installed successors", prefix.successors.len()));
            for successor in &prefix.successors {
                ui.label(format!(
                    "Detailed state {} · {:?}",
                    successor.state, successor.kind
                ));
            }
            for index in &prefix.frontiers {
                if let Some(frontier) = graph.frontiers.get(*index) {
                    ui.colored_label(
                        palette::WARNING,
                        format!("Pending {:?}: {}", frontier.kind, frontier.detail),
                    );
                }
            }
        } else {
            ui.label("No current instruction prefix is certified for this detailed input. Its entry or historical exit does not establish completed execution.");
            for frontier in graph
                .frontiers
                .iter()
                .filter(|frontier| frontier.from == Some(id))
            {
                ui.colored_label(
                    palette::WARNING,
                    format!("Pending {:?}: {}", frontier.kind, frontier.detail),
                );
            }
        }
        if let Some(state) = graph.states.iter().find(|state| state.state == id) {
            ui.collapsing("Recorded entry", |ui| {
                symbolic::show(ui, graph, state.entry.path.node, "Input path conditions");
                if let Some(frame) = state.entry.frames.last() {
                    context::frame(ui, graph, frame);
                    buffers::stack(ui, &frame.stack);
                }
            });
        }
        if graph.root_terminal_coverage.is_some() {
            ui.collapsing("Native terminal auxiliary", |ui| {
                super::terminal::show(ui, graph)
            });
        }
    }
}

fn snapshot(snapshot: &evm_abstract_protocol::RootTerminalSnapshot) -> String {
    match snapshot {
        evm_abstract_protocol::RootTerminalSnapshot::Offline(snapshot) => format!(
            "Offline {} · namespace {}",
            snapshot.label, snapshot.namespace
        ),
        evm_abstract_protocol::RootTerminalSnapshot::Chain(snapshot) => format!(
            "Chain {} · block {}",
            snapshot.chain_id, snapshot.block_hash
        ),
    }
}

fn phase(phase: InstructionProgress) -> &'static str {
    match phase {
        InstructionProgress::Started => "reached · operands pending",
        InstructionProgress::OperandsConsumed => "operands consumed · result pending",
        InstructionProgress::Completed => "local computation completed",
        InstructionProgress::Dispatched => "dispatch entered · inspect installed and pending paths",
        InstructionProgress::Faulted => "instruction fault",
    }
}
