//! Cached projection and scene indices. Workspace resets these for a new
//! immutable report; cheap shape checks also handle standalone graph fixtures.

use super::{Graph, GraphMode, NodeView, ReportIndex, Visibility, edge_label};
use egui::Ui;
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::AnalysisReport;
use std::collections::BTreeMap;

pub(super) struct ReportStamp {
    fingerprint: String,
    shape: (usize, usize, usize, usize, usize),
    endpoints: (Option<usize>, Option<usize>),
}

impl ReportStamp {
    fn shape(report: &AnalysisReport) -> (usize, usize, usize, usize, usize) {
        (
            report.cfg.len(),
            report.edges.len(),
            report.ssa.blocks.len(),
            report.ssa.transitions.len(),
            report.states.len(),
        )
    }
    fn new(report: &AnalysisReport) -> Self {
        Self {
            fingerprint: report.metadata.fingerprint.clone(),
            shape: Self::shape(report),
            endpoints: (
                report.cfg.first().map(|node| node.id),
                report.cfg.last().map(|node| node.id),
            ),
        }
    }
    fn matches(&self, report: &AnalysisReport) -> bool {
        self.fingerprint == report.metadata.fingerprint
            && self.shape == Self::shape(report)
            && self.endpoints
                == (
                    report.cfg.first().map(|node| node.id),
                    report.cfg.last().map(|node| node.id),
                )
    }
}

impl Graph {
    pub(super) fn ensure_scene(&mut self, report: &AnalysisReport, selected: Option<usize>) {
        if !self
            .report_stamp
            .as_ref()
            .is_some_and(|stamp| stamp.matches(report))
        {
            self.report_stamp = Some(ReportStamp::new(report));
            self.index = Some(ReportIndex::new(report));
            self.invalidate_scene();
        }
        if self.mode == GraphMode::Local && self.local_center != selected {
            self.invalidate_scene();
        }
        if !self.display_dirty {
            return;
        }
        let Some(index) = &self.index else {
            return;
        };
        self.display = super::projection::DisplayGraph::build(
            report,
            index,
            self.mode,
            self.program_scope,
            selected,
            self.neighborhood_hops,
        );
        self.ending_summary = index.termination_summary(
            &self
                .display
                .state_to_node
                .keys()
                .copied()
                .collect::<Vec<_>>(),
        );
        self.shown_states = self.display.state_to_node.len();
        self.native_edges = self.display.edge_members.values().map(Vec::len).sum();
        self.node_positions = self
            .display
            .nodes
            .iter()
            .enumerate()
            .map(|(pos, node)| (node.id, pos))
            .collect();
        self.edge_positions = self
            .display
            .edges
            .iter()
            .enumerate()
            .map(|(pos, edge)| (edge.id, pos))
            .collect();
        self.native_edge_to_display = self
            .display
            .edge_members
            .iter()
            .flat_map(|(display, members)| members.iter().map(move |native| (*native, *display)))
            .collect();
        self.edge_labels = self
            .display
            .edges
            .iter()
            .map(|edge| {
                let count = self.display.edge_members.get(&edge.id).map_or(0, Vec::len);
                let label = edge_label(edge.id, edge.kind);
                let label = if count > 1 {
                    format!(
                        "{} ({count})",
                        label
                            .split_once(' ')
                            .map_or(label.as_str(), |(_, kind)| kind)
                    )
                } else {
                    label
                };
                (edge.id, label)
            })
            .collect();
        self.nodes.clear();
        self.highlight_dirty = true;
        self.local_center = selected;
        self.display_dirty = false;
        self.layout_pending = true;
    }

    pub(super) fn rebuild_visibility(&mut self) {
        self.node_visibility = Visibility::new(&self.placement.nodes);
        let bounds: BTreeMap<_, _> = self
            .placement
            .edges
            .iter()
            .map(|(id, route)| {
                let mut bounds = route.label;
                for point in &route.path {
                    bounds.extend_with(*point);
                }
                (*id, bounds)
            })
            .collect();
        self.edge_visibility = Visibility::new(&bounds);
    }

    pub(super) fn update_highlight(&mut self, report: &AnalysisReport, selected: Option<usize>) {
        if !self.highlight_dirty && self.highlighted_state == selected {
            return;
        }
        self.highlighted_edges.clear();
        if let (Some(state), Some(index)) = (selected, self.index.as_ref()) {
            for position in index.outgoing(state).iter().chain(index.incoming(state)) {
                if let Some(id) = self.native_edge_to_display.get(&report.edges[*position].id) {
                    self.highlighted_edges.insert(*id);
                }
            }
        }
        self.highlighted_state = selected;
        self.highlight_dirty = false;
    }

    pub(super) fn edge_detail(&self, ui: &mut Ui, report: &AnalysisReport, edge: usize) {
        let Some(members) = self.display.edge_members.get(&edge) else {
            return;
        };
        ui.label(crate::notation::widget(
            ui,
            format!("{} original state edge(s)", members.len()),
        ));
        if self.mode == GraphMode::Blocks {
            ui.label(crate::notation::widget(
                ui,
                "Source overview: connected summary edges may use different state instances.",
            ));
        }
        if let Some(index) = &self.index {
            for id in members.iter().take(12) {
                if let Some(edge) = index.edge(report, *id) {
                    ui.label(crate::notation::widget(
                        ui,
                        egui::RichText::new(format!(
                            "{}: {} → {} · {:?}",
                            Symbol::Edge(edge.id),
                            Symbol::State(edge.from),
                            Symbol::State(edge.to),
                            edge.kind
                        ))
                        .monospace(),
                    ));
                }
            }
            if members.len() > 12 {
                ui.label(crate::notation::widget(
                    ui,
                    format!("{} more original edges", members.len() - 12),
                ));
            }
        }
    }

    pub(crate) fn accessible_summary(&self) -> String {
        if self.report_stamp.is_none() || self.display_dirty {
            return format!("CFG view: {:?}; awaiting display", self.mode);
        }
        let total = self.shown_states + self.display.hidden_states;
        let scope = if self.mode == GraphMode::Local {
            "Cross-program neighborhood".into()
        } else {
            self.program_scope
                .map_or("All programs".into(), |id| Symbol::Program(id).to_string())
        };
        let detail = match (self.detail_level, self.content) {
            (super::paint::DetailLevel::Identity, _) => "identities",
            (super::paint::DetailLevel::Preview, NodeView::Ssa) => "SSA preview",
            (super::paint::DetailLevel::Preview, NodeView::Disassembly) => "disassembly preview",
        };
        let layout = self.layout_error.as_ref().map_or_else(
            || {
                format!(
                    "{} groups; CFG detail: {detail}; CFG zoom: {:.3}%",
                    self.placement.groups.len(),
                    self.zoom * 100.0,
                )
            },
            |error| format!("Layout unavailable: {error}"),
        );
        let selected_ending = self
            .highlighted_state
            .and_then(|state| self.display.state_to_node.get(&state))
            .and_then(|id| self.nodes.get(id))
            .map_or_else(
                || "Selected CFG ending: unavailable in this view".into(),
                |node| {
                    let summary = node.termination;
                    let kind = match summary.single_kind() {
                        Some(evm_abstract_protocol::OutcomeKind::Return) => "normal",
                        Some(evm_abstract_protocol::OutcomeKind::Revert) => "REVERT",
                        Some(evm_abstract_protocol::OutcomeKind::Failure) => "exception",
                        None if summary.has_termination() => "mixed / possible ending",
                        None => "no termination evidence",
                    };
                    format!(
                        "Selected CFG ending: {kind}; {} native members; root normal {}, REVERT {}, exception {}; frame normal {}, REVERT {}, exception {}; continuing {}, unresolved {}, non-current {}, unobserved {}",
                        summary.members,
                        summary.root.returns,
                        summary.root.reverts,
                        summary.root.failures,
                        summary.frame.returns,
                        summary.frame.reverts,
                        summary.frame.failures,
                        summary.continuing,
                        summary.unresolved,
                        summary.non_current,
                        summary.unobserved,
                    )
                },
            );
        format!(
            "CFG view: {:?}; {} shown nodes; {}/{total} states; {} shown edges; {} native edges; {} hidden states; {} boundary edges; {scope}; {layout}; CFG endings: root return {}, revert {}, failure {}; frame return {}, revert {}, failure {}; continuing {}, unresolved {}, unobserved {}; {selected_ending}",
            self.mode,
            self.display.nodes.len(),
            self.shown_states,
            self.display.edges.len(),
            self.native_edges,
            self.display.hidden_states,
            self.display.boundary_edges,
            self.ending_summary.root.returns,
            self.ending_summary.root.reverts,
            self.ending_summary.root.failures,
            self.ending_summary.frame.returns,
            self.ending_summary.frame.reverts,
            self.ending_summary.frame.failures,
            self.ending_summary.continuing,
            self.ending_summary.unresolved,
            self.ending_summary.unobserved,
        )
    }
}
