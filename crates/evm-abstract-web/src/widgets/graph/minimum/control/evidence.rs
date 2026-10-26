//! Resolve display provenance against the same immutable observation. In
//! particular, a retained return delivery is not automatically a current one.

use super::FlowOrigin;
use evm_abstract_protocol::{AnalysisReport, SummarySourceStatus};

impl FlowOrigin {
    pub(super) fn qualified_current(self, report: &AnalysisReport, summary: usize) -> bool {
        let Some(model) = report.minimum.as_ref() else {
            return false;
        };
        let Some(observation) = model.summaries.get(summary) else {
            return false;
        };
        if crate::app::minimum::source_status(model, summary) != SummarySourceStatus::Current {
            return false;
        }
        let cfg = &observation.relation.regional_cfg;
        let current_node = |id| {
            cfg.nodes
                .iter()
                .find(|node| node.id == id)
                .is_some_and(|node| {
                    !node.excluded
                        && node.transfer.as_ref().is_some_and(|transfer| {
                            transfer.current && transfer.input_revision == node.input_revision
                        })
                })
        };
        match self {
            Self::Admission { .. } | Self::DecodedJump { .. } => false,
            Self::Source { node, .. } | Self::Jump { node, .. } => current_node(node),
            Self::Regional { from, to } => {
                current_node(from)
                    && cfg
                        .nodes
                        .iter()
                        .find(|node| node.id == from)
                        .and_then(|node| node.transfer.as_ref())
                        .is_some_and(|transfer| {
                            transfer.successors.iter().any(|edge| edge.to == to)
                        })
            }
            Self::CallSite { call } | Self::Call { call, .. } => {
                cfg.calls
                    .iter()
                    .find(|row| row.id == call)
                    .is_some_and(|call| {
                        current_node(call.from)
                            && cfg
                                .nodes
                                .iter()
                                .find(|node| node.id == call.from)
                                .is_some_and(|node| {
                                    node.input_revision == call.input_revision
                                        && node.transfer.as_ref().is_some_and(|transfer| {
                                            transfer.calls.contains(&call.id)
                                        })
                                })
                    })
            }
            Self::Delivery { call, delivery, .. } => cfg
                .calls
                .iter()
                .find(|row| row.id == call)
                .is_some_and(|call| {
                    current_node(call.from)
                        && call.deliveries.get(delivery).is_some_and(|receipt| {
                            receipt.current
                                && receipt.input_revision == call.input_revision
                                && cfg
                                    .nodes
                                    .iter()
                                    .find(|node| node.id == call.from)
                                    .is_some_and(|node| node.input_revision == call.input_revision)
                                && observation.bounds.get(call.callee).is_some_and(|bounds| {
                                    bounds.outputs.iter().any(|output| {
                                        output.kind == receipt.kind
                                            && output.generation == receipt.output_generation
                                    })
                                })
                        })
                }),
        }
    }

    pub(crate) fn detail(self, report: &AnalysisReport, summary: usize) -> String {
        let Some(model) = report.minimum.as_ref() else {
            return "Canonical evidence unavailable".into();
        };
        let Some(observation) = model.summaries.get(summary) else {
            return "Observation unavailable".into();
        };
        let cfg = &observation.relation.regional_cfg;
        let source = crate::app::minimum::source_status(model, summary);
        let qualified = self.qualified_current(report, summary);
        let receipt = match self {
            Self::Delivery { call, delivery, successor } => cfg.calls.iter().find(|row| row.id == call)
                .and_then(|call| call.deliveries.get(delivery).map(|receipt| format!(
                    "{:?} delivery · call {} · successor r{}\nCaller input {} · delivered input {} · output generation {}\nWithin-observation current {} · {} creation alternatives",
                    receipt.kind, call.id, successor, call.input_revision, receipt.input_revision, receipt.output_generation, receipt.current, receipt.creation.len()
                ))),
            Self::CallSite { call } | Self::Call { call, .. } => cfg.calls.iter().find(|row| row.id == call).map(|call| format!(
                "{:?} · {:?} scope · target {:?}\nCall {} · caller r{} · callee procedure {} · input {}",
                call.action, call.scope, call.target, call.id, call.from, call.callee, call.input_revision
            )),
            Self::Admission { node } => Some(format!("Admitted regional r{node}; admission alone supplies no transfer")),
            Self::DecodedJump { relation, target } => Some(format!("Decoded legal JUMPDEST index {target} · directory {relation}\nUnknown-jump receipt is unexpanded; source candidate has no regional or instruction evidence")),
            Self::Source { node, relation, from, to } => Some(format!("MAY source {from:?} → {to:?} · regional r{node} · directory {relation}")),
            Self::Regional { from, to } => Some(format!("Retained regional topology r{from} → r{to}")),
            Self::Jump { node, relation, target } => Some(format!("MAY legal JUMPDEST index {target} · hub receipt r{node} · directory {relation}\nTarget-set membership does not supply the destination's effects")),
        };
        format!(
            "{}\nObservation {source:?} · qualified current {qualified}",
            receipt.unwrap_or_else(|| "Receipt unavailable".into())
        )
    }
}
