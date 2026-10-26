//! Derived scene indices belong to the explicit report view bound by Graph::activate.

use super::{Graph, GraphMode, NodeView, ReportIndex, Visibility, edge_label};
use crate::report::ReportView;
use egui::Ui;
use evm_abstract_notation::Symbol;
use std::collections::BTreeMap;

impl Graph {
    pub(super) fn ensure_scene(&mut self, report: &ReportView<'_>, selected: Option<usize>) {
        if self.scene.index.is_none() {
            self.scene.index = Some(ReportIndex::new(report));
        }
        let index = self.scene.index.as_ref().unwrap();
        if !self.scene.scope_initialized {
            self.scene.program_scope = selected
                .and_then(|state| index.cfg(report, state))
                .or_else(|| report.cfg.first())
                .and_then(|state| state.program);
            self.scene.scope_initialized = true;
        }
        let selected_context = selected.filter(|state| {
            index
                .cfg(report, *state)
                .is_some_and(|state| state.program == self.scene.program_scope)
        });
        if self.scene.native_context.is_none() {
            self.scene.native_context = selected_context.or_else(|| {
                report
                    .cfg
                    .iter()
                    .find(|state| state.program == self.scene.program_scope)
                    .map(|state| state.id)
            });
        }
        if !self.whole_scope
            && let (Some(context), Some(selected)) = (self.scene.native_context, selected_context)
            && (!super::projection::same_context(report, index, context, selected)
                || (!self.scene.display_dirty
                    && !self.scene.display.state_to_node.contains_key(&selected)))
        {
            self.scene.native_context = Some(selected);
            self.invalidate_scene();
        }
        if self.mode == GraphMode::Local
            && self.scene.local_center != selected
            && !self.scene.display_dirty
        {
            self.invalidate_scene();
        }
        if !self.scene.display_dirty {
            return;
        }
        let Some(index) = &self.scene.index else {
            return;
        };
        self.scene.display = if self.whole_scope {
            super::projection::DisplayGraph::build(
                report,
                index,
                self.mode,
                self.scene.program_scope,
                selected,
                self.neighborhood_hops,
            )
        } else {
            super::projection::DisplayGraph::scoped(
                report,
                index,
                self.mode,
                self.scene.program_scope,
                self.scene.native_context,
                selected,
                self.neighborhood_hops,
            )
        };
        self.scene.ending_summary = index.termination_summary(
            &self
                .scene
                .display
                .state_to_node
                .keys()
                .copied()
                .collect::<Vec<_>>(),
        );
        self.scene.shown_states = self.scene.display.state_to_node.len();
        self.scene.native_edges = self.scene.display.edge_members.values().map(Vec::len).sum();
        self.scene.node_positions = self
            .scene
            .display
            .nodes
            .iter()
            .enumerate()
            .map(|(pos, node)| (node.id, pos))
            .collect();
        self.scene.edge_positions = self
            .scene
            .display
            .edges
            .iter()
            .enumerate()
            .map(|(pos, edge)| (edge.id, pos))
            .collect();
        self.scene.native_edge_to_display = self
            .scene
            .display
            .edge_members
            .iter()
            .flat_map(|(display, members)| members.iter().map(move |native| (*native, *display)))
            .collect();
        self.scene.edge_labels = self
            .scene
            .display
            .edges
            .iter()
            .map(|edge| {
                let count = self
                    .scene
                    .display
                    .edge_members
                    .get(&edge.id)
                    .map_or(0, Vec::len);
                let label = edge_label(edge.id, edge.kind);
                let label = if count > 1 {
                    format!("{label} ({count})")
                } else {
                    label
                };
                (edge.id, label)
            })
            .collect();
        self.scene.nodes.clear();
        self.scene.highlight_dirty = true;
        self.scene.local_center = selected;
        self.scene.display_dirty = false;
        self.scene.layout_pending = true;
    }

    pub(super) fn rebuild_visibility(&mut self) {
        self.scene.node_visibility = Visibility::new(&self.scene.placement.nodes);
        let bounds: BTreeMap<_, _> = self
            .scene
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
        self.scene.edge_visibility = Visibility::new(&bounds);
    }

    pub(super) fn update_highlight(&mut self, report: &ReportView<'_>, selected: Option<usize>) {
        if !self.scene.highlight_dirty && self.scene.highlighted_state == selected {
            return;
        }
        self.scene.highlighted_edges.clear();
        if let (Some(state), Some(index)) = (selected, self.scene.index.as_ref()) {
            for position in index.outgoing(state).iter().chain(index.incoming(state)) {
                if let Some(id) = self
                    .scene
                    .native_edge_to_display
                    .get(&report.edges[*position].id)
                {
                    self.scene.highlighted_edges.insert(*id);
                }
            }
        }
        self.scene.highlighted_state = selected;
        self.scene.highlight_dirty = false;
    }

    pub(super) fn edge_detail(&self, ui: &mut Ui, report: &ReportView<'_>, edge: usize) {
        let Some(members) = self.scene.display.edge_members.get(&edge) else {
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
        if let Some(index) = &self.scene.index {
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
        if self.scene.identity.is_minimum() {
            return self.minimum_summary();
        }
        if self.scene.index.is_none() || self.scene.display_dirty {
            return format!("CFG view: {:?}; awaiting display", self.mode);
        }
        let total = self.scene.shown_states + self.scene.display.hidden_states;
        let scope = if self.mode == GraphMode::Local {
            "Cross-program neighborhood".into()
        } else {
            self.scene
                .program_scope
                .map_or("All programs".into(), |id| Symbol::Program(id).to_string())
        };
        let detail = match (self.scene.detail_level, self.content) {
            (super::paint::DetailLevel::Silhouette, _) => "overview shapes",
            (super::paint::DetailLevel::Identity, _) => "identities",
            (super::paint::DetailLevel::Preview, NodeView::Ssa) => "SSA preview",
            (super::paint::DetailLevel::Preview, NodeView::Disassembly) => "disassembly preview",
        };
        let layout = self.scene.layout_error.as_ref().map_or_else(
            || {
                format!(
                    "{} groups; CFG detail: {detail}; CFG zoom: {:.3}%",
                    self.scene.placement.groups.len(),
                    self.zoom * 100.0,
                )
            },
            |error| format!("Layout unavailable: {error}"),
        );
        let selected_ending = self.scene.highlighted_state
            .and_then(|state| self.scene.display.state_to_node.get(&state))
            .and_then(|id| self.scene.nodes.get(id))
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
            self.scene.display.nodes.len(),
            self.scene.shown_states,
            self.scene.display.edges.len(),
            self.scene.native_edges,
            self.scene.display.hidden_states,
            self.scene.display.boundary_edges,
            self.scene.ending_summary.root.returns,
            self.scene.ending_summary.root.reverts,
            self.scene.ending_summary.root.failures,
            self.scene.ending_summary.frame.returns,
            self.scene.ending_summary.frame.reverts,
            self.scene.ending_summary.frame.failures,
            self.scene.ending_summary.continuing,
            self.scene.ending_summary.unresolved,
            self.scene.ending_summary.unobserved,
        )
    }
}
