use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{
    super::scope::{
        CardKind, Destination, FlowOrigin, GraphProjection, ProjectionCard, TargetEntry, TargetSet,
        TargetSetKind,
    },
    edge,
};

/// Windowing changes display ownership only. Every removed crossing retains its
/// original flow and exact destination in an inspectable portal.
pub(super) fn project(
    report: &evm_abstract_protocol::AnalysisReport,
    full: GraphProjection,
    limit: usize,
) -> GraphProjection {
    let source = |id: usize| {
        matches!(
            full.cards[id].kind,
            CardKind::Source { .. } | CardKind::Receipt
        )
    };
    let center = full
        .anchor
        .filter(|&id| source(id))
        .or_else(|| (0..full.cards.len()).find(|&id| source(id)));
    let mut outgoing: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut incoming: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for edge in &full.edges {
        outgoing.entry(edge.from).or_default().push(edge.to);
        incoming.entry(edge.to).or_default().push(edge.from);
    }
    let mut selected = BTreeSet::new();
    let mut queue: VecDeque<_> = center.into_iter().collect();
    while let Some(id) = queue.pop_front() {
        if selected.contains(&id) {
            continue;
        }
        // Calls and boundary portals do not consume source-point capacity, but
        // traversal through a portal must never pull an entire callee into scope.
        if source(id) && selected.iter().filter(|&&id| source(id)).count() >= limit {
            continue;
        }
        selected.insert(id);
        if !source(id) {
            continue;
        }
        queue.extend(outgoing.get(&id).into_iter().flatten().copied());
        queue.extend(
            incoming
                .get(&id)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&id| source(id)),
        );
    }
    // Include every boundary attached to a displayed source, even when the
    // capacity was reached before the boundary's queue position.
    let boundary: Vec<_> = full
        .edges
        .iter()
        .filter(|edge| selected.contains(&edge.from) && !source(edge.to))
        .map(|edge| edge.to)
        .collect();
    selected.extend(boundary);
    let shown_points = selected.iter().filter(|&&id| source(id)).count();
    let displayed_receipts: BTreeSet<_> = selected
        .iter()
        .filter(|&&id| source(id))
        .flat_map(|&id| {
            full.cards[id]
                .origins
                .iter()
                .map(|origin| origin.target.origin.node)
        })
        .collect();
    let remap: BTreeMap<_, _> = selected
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect();
    let mut result = GraphProjection {
        scope: full.scope,
        source_status: full.source_status,
        cards: selected.iter().map(|&id| full.cards[id].clone()).collect(),
        edges: Vec::new(),
        target_sets: Vec::new(),
        counts: full.counts,
        anchor: center.and_then(|id| remap.get(&id).copied()),
        hidden_set: None,
    };
    let mut retained_sets = BTreeMap::new();
    for card in &mut result.cards {
        if let Some(old) = card.target_set {
            let index = *retained_sets.entry(old).or_insert_with(|| {
                let index = result.target_sets.len();
                result.target_sets.push(full.target_sets[old].clone());
                index
            });
            card.target_set = Some(index);
        }
    }
    let mut folded: BTreeMap<(usize, bool), usize> = BTreeMap::new();
    let calls: BTreeMap<_, _> = report.minimum.as_ref().unwrap().summaries[full.scope.summary()]
        .relation
        .regional_cfg
        .calls
        .iter()
        .map(|call| (call.id, call))
        .collect();
    for flow in &full.edges {
        match (remap.get(&flow.from).copied(), remap.get(&flow.to).copied()) {
            (Some(from), Some(to)) => {
                for &origin in &flow.origins {
                    edge(&mut result.edges, from, to, flow.kind, origin);
                }
            }
            (None, None) => result.counts.hidden_flows += flow.origins.len(),
            (from, to) => {
                result.counts.hidden_flows += flow.origins.len();
                let reverse = from.is_none();
                let visible = from.or(to).unwrap();
                let hidden = if reverse { flow.from } else { flow.to };
                let portal = *folded.entry((visible, reverse)).or_insert_with(|| {
                    let id = result.cards.len();
                    let index = result.target_sets.len();
                    result.cards.push(ProjectionCard {
                        kind: CardKind::Folded,
                        origins: Vec::new(),
                        target_set: Some(index),
                    });
                    result.target_sets.push(TargetSet {
                        kind: TargetSetKind::Folded,
                        entries: Vec::new(),
                    });
                    id
                });
                let index = result.cards[portal].target_set.unwrap();
                for &via in &flow.origins {
                    let node = match via {
                        FlowOrigin::Admission { node } => Some(node),
                        FlowOrigin::DecodedJump { .. } => None,
                        FlowOrigin::Source { node, .. } | FlowOrigin::Jump { node, .. } => {
                            Some(node)
                        }
                        FlowOrigin::Regional { from, to } => Some(if reverse { from } else { to }),
                        FlowOrigin::CallSite { call } | FlowOrigin::Call { call, .. } => {
                            calls.get(&call).map(|call| call.from)
                        }
                        FlowOrigin::Delivery {
                            call, successor, ..
                        } => {
                            if reverse {
                                calls.get(&call).map(|call| call.from)
                            } else {
                                Some(successor)
                            }
                        }
                    };
                    for origin in full.cards[hidden]
                        .origins
                        .iter()
                        .filter(|origin| Some(origin.target.origin.node) == node)
                    {
                        let entry = TargetEntry {
                            destination: Destination::Control(origin.target),
                            via,
                        };
                        if !result.target_sets[index].entries.contains(&entry) {
                            result.target_sets[index].entries.push(entry);
                        }
                    }
                }
                let (from, to) = if reverse {
                    (portal, visible)
                } else {
                    (visible, portal)
                };
                for &origin in &flow.origins {
                    edge(&mut result.edges, from, to, flow.kind, origin);
                }
            }
        }
    }
    result.counts.displayed_points = shown_points;
    result.counts.hidden_points = result.counts.total_points.saturating_sub(shown_points);
    result.counts.displayed_unexpanded_points = result
        .cards
        .iter()
        .filter(|card| {
            matches!(card.kind, CardKind::Source { .. } | CardKind::Receipt)
                && !card
                    .origins
                    .iter()
                    .any(|origin| origin.through || origin.transferred)
        })
        .count();
    result.counts.displayed_receipts = displayed_receipts.len();
    result.counts.hidden_receipts = result
        .counts
        .context_receipts
        .saturating_sub(displayed_receipts.len());
    result.counts.displayed_calls = result
        .target_sets
        .iter()
        .filter_map(|set| match &set.kind {
            TargetSetKind::Calls { calls } => Some(calls.len()),
            _ => None,
        })
        .sum();
    let hidden_entries: Vec<_> = full
        .cards
        .iter()
        .enumerate()
        .filter(|(id, _)| source(*id) && !selected.contains(id))
        .flat_map(|(_, card)| {
            card.origins.iter().map(|origin| TargetEntry {
                destination: Destination::Control(origin.target),
                via: FlowOrigin::Admission {
                    node: origin.target.origin.node,
                },
            })
        })
        .collect();
    if !hidden_entries.is_empty() {
        result.hidden_set = Some(result.target_sets.len());
        result.target_sets.push(TargetSet {
            kind: TargetSetKind::Folded,
            entries: hidden_entries,
        });
    }
    result
}
