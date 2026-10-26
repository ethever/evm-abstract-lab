//! Display policy over an unchanged D5 source projection. Omitted crossings are
//! typed navigation metadata; they never manufacture rank-distorting graph nodes.

mod provenance;
mod select;

use std::collections::{BTreeMap, BTreeSet};

use super::{
    super::scope::{
        Destination, FlowOrigin, FrontierDirection, FrontierFlow, GraphLevel, GraphProjection,
        GraphWindow, ProjectionError, TargetEntry, TargetSet, TargetSetKind, WindowFrontier,
    },
    edge,
};
use provenance::Provenance;

pub(super) fn project(
    report: &evm_abstract_protocol::AnalysisReport,
    full: GraphProjection,
    choice: GraphWindow,
) -> Result<GraphProjection, ProjectionError> {
    let choice = if full.level == GraphLevel::Model {
        GraphWindow::FullSource
    } else {
        choice
    };
    let core = select::core(&full);
    let selected = select::cards(&full, choice, &core);
    let remap: BTreeMap<_, _> = selected
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect();
    let mut result = GraphProjection {
        scope: full.scope,
        window: choice,
        level: full.level,
        source_status: full.source_status,
        cards: selected.iter().map(|&id| full.cards[id].clone()).collect(),
        edges: Vec::new(),
        // Preserve every original target set, including targets attached to an
        // omitted source/call boundary. Existing card/set identities stay exact.
        target_sets: full.target_sets.clone(),
        counts: full.counts,
        anchor: full.anchor.and_then(|id| remap.get(&id).copied()),
        core: core
            .iter()
            .filter_map(|id| remap.get(id).copied())
            .collect(),
        frontiers: Vec::new(),
        hidden_set: None,
        model_relations: full.model_relations.clone(),
        model_set: full.model_set,
    };
    let cfg = &report
        .minimum
        .as_ref()
        .ok_or(ProjectionError::MissingModel)?
        .summaries[full.scope.summary()]
    .relation
    .regional_cfg;
    let provenance = Provenance::new(full.scope.summary(), cfg);
    let mut frontier_indices: BTreeMap<(usize, bool), usize> = BTreeMap::new();
    for flow in &full.edges {
        match (remap.get(&flow.from).copied(), remap.get(&flow.to).copied()) {
            (Some(from), Some(to)) => {
                for &origin in &flow.origins {
                    edge(&mut result.edges, from, to, flow.kind, flow.level, origin);
                }
            }
            (None, None) => result.counts.hidden_flows += flow.origins.len(),
            (from, to) => {
                result.counts.hidden_flows += flow.origins.len();
                let direction = if from.is_none() {
                    FrontierDirection::Incoming
                } else {
                    FrontierDirection::Outgoing
                };
                let card = from.or(to).ok_or(ProjectionError::MissingFlowOrigin)?;
                let frontier = *frontier_indices
                    .entry((card, direction == FrontierDirection::Incoming))
                    .or_insert_with(|| {
                        let index = result.frontiers.len();
                        let target_set = result.target_sets.len();
                        result.target_sets.push(TargetSet {
                            kind: TargetSetKind::Folded,
                            entries: Vec::new(),
                        });
                        result.frontiers.push(WindowFrontier {
                            card,
                            direction,
                            target_set,
                            flows: Vec::new(),
                        });
                        index
                    });
                for &origin in &flow.origins {
                    let (source, target) = provenance.endpoints(origin)?;
                    let frontier = &mut result.frontiers[frontier];
                    frontier.flows.push(FrontierFlow {
                        from: source,
                        to: target,
                        kind: flow.kind,
                        level: flow.level,
                        origin,
                    });
                    let destination = if direction == FrontierDirection::Incoming {
                        source
                    } else {
                        target
                    };
                    let entry = TargetEntry {
                        destination: Destination::Control(destination),
                        via: origin,
                    };
                    let entries = &mut result.target_sets[frontier.target_set].entries;
                    if !entries.contains(&entry) {
                        entries.push(entry);
                    }
                }
            }
        }
    }
    result.counts.incoming_flows = result
        .frontiers
        .iter()
        .filter(|frontier| frontier.direction == FrontierDirection::Incoming)
        .map(|frontier| frontier.flows.len())
        .sum();
    result.counts.outgoing_flows = result
        .frontiers
        .iter()
        .filter(|frontier| frontier.direction == FrontierDirection::Outgoing)
        .map(|frontier| frontier.flows.len())
        .sum();
    let shown_points = selected
        .iter()
        .filter(|&&id| select::point(&full, id))
        .count();
    let receipts: BTreeSet<_> = selected
        .iter()
        .filter(|&&id| select::point(&full, id))
        .flat_map(|&id| {
            full.cards[id]
                .origins
                .iter()
                .map(|origin| origin.target.origin.node)
        })
        .collect();
    result.counts.displayed_points = shown_points;
    result.counts.hidden_points = result.counts.total_points.saturating_sub(shown_points);
    result.counts.displayed_receipts = receipts.len();
    result.counts.hidden_receipts = result
        .counts
        .context_receipts
        .saturating_sub(receipts.len());
    result.counts.displayed_unexpanded_points = result
        .cards
        .iter()
        .filter(|card| {
            matches!(
                card.kind,
                super::super::scope::CardKind::Source { .. }
                    | super::super::scope::CardKind::Receipt
            ) && !card
                .origins
                .iter()
                .any(|origin| origin.through || origin.transferred)
        })
        .count();
    result.counts.displayed_calls = result
        .cards
        .iter()
        .filter_map(|card| card.target_set)
        .filter_map(|id| match &result.target_sets[id].kind {
            TargetSetKind::Calls { calls } => Some(calls.len()),
            _ => None,
        })
        .sum();
    result.counts.summarized_model_relations = result
        .model_relations
        .iter()
        .filter(|relation| {
            !result.edges.iter().any(|edge| {
                relation.kind == Some(edge.kind) && edge.origins.contains(&relation.origin)
            })
        })
        .count();
    let hidden = full
        .cards
        .iter()
        .enumerate()
        .filter(|(id, _)| select::point(&full, *id) && !selected.contains(id))
        .flat_map(|(_, card)| {
            card.origins.iter().map(|origin| TargetEntry {
                destination: Destination::Control(origin.target),
                via: FlowOrigin::Admission {
                    node: origin.target.origin.node,
                },
            })
        })
        .collect();
    result.hidden_set = append_set(&mut result.target_sets, hidden);
    Ok(result)
}

fn append_set(sets: &mut Vec<TargetSet>, entries: Vec<TargetEntry>) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }
    let index = sets.len();
    sets.push(TargetSet {
        kind: TargetSetKind::Folded,
        entries,
    });
    Some(index)
}
