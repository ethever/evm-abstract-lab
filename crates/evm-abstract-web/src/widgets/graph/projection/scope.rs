//! A bounded Native display scope. Missing frame evidence never certifies two
//! states as the same invocation. Portals retain original target state IDs.
use super::{DisplayGraph, DisplayNode, GraphMode, Portal, ReportIndex, edge_kind_key};
use crate::report::ReportView;
use evm_abstract_protocol::{CfgBlock, FrameSnapshot};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const SOURCE_CARD_LIMIT: usize = 12;

pub(in crate::widgets::graph) fn same_context(
    report: &ReportView<'_>,
    index: &ReportIndex,
    left: usize,
    right: usize,
) -> bool {
    if left == right {
        return index.cfg(report, left).is_some();
    }
    let (Some(a), Some(b)) = (index.cfg(report, left), index.cfg(report, right)) else {
        return false;
    };
    if !same_source(a, b) {
        return false;
    }
    let (Some(a), Some(b)) = (index.details(report, left), index.details(report, right)) else {
        return false;
    };
    if a.entry.frames.len() != b.entry.frames.len() || a.entry.frames.is_empty() {
        return false;
    }
    let last = a.entry.frames.len() - 1;
    a.entry.frames[..last] == b.entry.frames[..last]
        && same_active(&a.entry.frames[last], &b.entry.frames[last])
}

fn same_source(a: &CfgBlock, b: &CfgBlock) -> bool {
    a.program == b.program
        && a.code_address == b.code_address
        && a.storage_address == b.storage_address
        && a.frame_depth == b.frame_depth
        && match (&a.process, &b.process) {
            (Some(a), Some(b)) => a.same_context(b),
            (None, None) => true,
            _ => false,
        }
}

fn same_active(a: &FrameSnapshot, b: &FrameSnapshot) -> bool {
    a.index == b.index
        && a.program == b.program
        && a.code_address == b.code_address
        && a.code_hash == b.code_hash
        && a.storage_address == b.storage_address
        && a.address_value == b.address_value
        && a.caller == b.caller
        && a.address_operand == b.address_operand
        && a.caller_operand == b.caller_operand
        && a.call_value == b.call_value
        && a.kind == b.kind
        && a.is_static == b.is_static
        && a.continuation == b.continuation
        && a.rollback_store == b.rollback_store
        && a.calldata == b.calldata
        && a.process_operands == b.process_operands
        && a.process_creation == b.process_creation
        && match (&a.process, &b.process) {
            (Some(a), Some(b)) => a.same_context(b),
            (None, None) => true,
            _ => false,
        }
}

impl DisplayGraph {
    pub(in crate::widgets::graph) fn scoped(
        report: &ReportView<'_>,
        index: &ReportIndex,
        mode: GraphMode,
        program: Option<usize>,
        context: Option<usize>,
        center: Option<usize>,
        hops: usize,
    ) -> Self {
        let context = context
            .filter(|state| index.cfg(report, *state).is_some())
            .or_else(|| {
                report
                    .cfg
                    .iter()
                    .find(|state| program.is_none() || state.program == program)
                    .map(|state| state.id)
            });
        let allowed = report
            .cfg
            .iter()
            .filter(|state| {
                context.is_some_and(|context| same_context(report, index, context, state.id))
            })
            .map(|state| state.id)
            .collect();
        let mut display =
            Self::build_filtered(report, index, mode, program, center, hops, Some(&allowed));
        let center = center
            .or(context)
            .and_then(|state| display.state_to_node.get(&state))
            .copied()
            .or_else(|| display.nodes.first().map(|node| node.id));
        let mut neighbors: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for edge in &display.edges {
            neighbors.entry(edge.from).or_default().push(edge.to);
            neighbors.entry(edge.to).or_default().push(edge.from);
        }
        let mut shown = BTreeSet::new();
        let mut queue = VecDeque::new();
        if let Some(center) = center {
            shown.insert(center);
            queue.push_back(center);
        }
        while let Some(node) = queue.pop_front() {
            for &neighbor in neighbors.get(&node).into_iter().flatten() {
                if shown.len() == SOURCE_CARD_LIMIT {
                    break;
                }
                if shown.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        display.nodes.retain(|node| shown.contains(&node.id));
        display.state_to_node.retain(|_, node| shown.contains(node));
        display
            .edges
            .retain(|edge| shown.contains(&edge.from) && shown.contains(&edge.to));
        let kept_edges: BTreeSet<_> = display.edges.iter().map(|edge| edge.id).collect();
        display.edge_members.retain(|id, _| kept_edges.contains(id));
        display.hidden_states = report.cfg.len().saturating_sub(display.state_to_node.len());
        display.boundary_edges = 0;
        let mut portals = BTreeMap::new();
        let mut portal_targets: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        let mut occupied: BTreeSet<_> = report.cfg.iter().map(|state| state.id).collect();
        let mut next_id = 0;
        for edge in report.edges.iter() {
            if index.cfg(report, edge.from).is_none() || index.cfg(report, edge.to).is_none() {
                continue;
            }
            let from = display.state_to_node.get(&edge.from).copied();
            let to = display.state_to_node.get(&edge.to).copied();
            let (anchor, target, outgoing) = match (from, to) {
                (Some(from), None) => (from, edge.to, true),
                (None, Some(to)) => (to, edge.from, false),
                _ => continue,
            };
            display.boundary_edges += 1;
            let key = (anchor, outgoing, edge_kind_key(edge.kind));
            let (id, display_edge) = *portals.entry(key).or_insert_with(|| {
                while occupied.contains(&next_id) {
                    next_id += 1;
                }
                let id = next_id;
                occupied.insert(id);
                display.nodes.push(DisplayNode {
                    id,
                    members: Vec::new(),
                    program: None,
                    basic_block: None,
                    start_pc: None,
                });
                display.portals.insert(
                    id,
                    Portal {
                        targets: Vec::new(),
                        outgoing,
                        kind: edge.kind,
                    },
                );
                display.edges.push(evm_abstract_protocol::CfgEdge {
                    id: edge.id,
                    from: if outgoing { anchor } else { id },
                    to: if outgoing { id } else { anchor },
                    kind: edge.kind,
                });
                display.edge_members.insert(edge.id, Vec::new());
                (id, edge.id)
            });
            portal_targets.entry(id).or_default().insert(target);
            display
                .edge_members
                .get_mut(&display_edge)
                .unwrap()
                .push(edge.id);
        }
        for (id, targets) in portal_targets {
            display.portals.get_mut(&id).unwrap().targets = targets.into_iter().collect();
        }
        display
    }
}

#[cfg(test)]
mod tests;
