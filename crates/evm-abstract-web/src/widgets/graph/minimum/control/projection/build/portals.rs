use std::collections::BTreeMap;

use super::{
    Builder, CardKind, Destination, FlowOrigin, TargetEntry, TargetSet, TargetSetKind, edge,
};
use evm_abstract_protocol::{EdgeKind, ProcessAction};

impl Builder<'_> {
    pub(super) fn regional_edges(&mut self) {
        let nodes: BTreeMap<_, _> = self
            .relation
            .relation
            .regional_cfg
            .nodes
            .iter()
            .map(|node| (node.id, node))
            .collect();
        for node in self.members.clone() {
            let Some(from) = self.body(node.id) else {
                continue;
            };
            let mut outside = Vec::new();
            for &successor in &node.successors {
                let Some(target) = nodes.get(&successor) else {
                    continue;
                };
                let kind = node
                    .transfer
                    .as_ref()
                    .and_then(|transfer| {
                        transfer.successors.iter().find(|edge| edge.to == successor)
                    })
                    .map_or(EdgeKind::Fallthrough, |edge| edge.kind);
                let origin = FlowOrigin::Regional {
                    from: node.id,
                    to: successor,
                };
                if let Some(to) = self.body(successor) {
                    edge(&mut self.projection.edges, from, to, kind, origin);
                } else {
                    outside.push((
                        kind,
                        TargetEntry {
                            destination: Destination::Control(self.target(target, None)),
                            via: origin,
                        },
                    ));
                }
            }
            if !outside.is_empty() {
                let portal = self.portal(
                    CardKind::Folded,
                    vec![self.evidence(node, None, false, false)],
                    TargetSet {
                        kind: TargetSetKind::ContextBoundary,
                        entries: outside.iter().map(|(_, entry)| *entry).collect(),
                    },
                );
                for (kind, entry) in outside {
                    edge(&mut self.projection.edges, from, portal, kind, entry.via);
                }
            }
        }
    }

    pub(super) fn calls(&mut self) {
        let cfg = &self.relation.relation.regional_cfg;
        let nodes: BTreeMap<_, _> = cfg.nodes.iter().map(|node| (node.id, node)).collect();
        let mut portals = BTreeMap::new();
        for call in &cfg.calls {
            let Some(from) = self.body(call.from) else {
                continue;
            };
            let Some(caller) = nodes.get(&call.from).copied() else {
                continue;
            };
            self.projection.counts.calls += 1;
            self.projection.counts.deliveries += call.deliveries.len();
            let portal = *portals
                .entry((from, action_key(call.action)))
                .or_insert_with(|| {
                    self.portal(
                        CardKind::Calls {
                            action: call.action,
                        },
                        Vec::new(),
                        TargetSet {
                            kind: TargetSetKind::Calls { calls: Vec::new() },
                            entries: Vec::new(),
                        },
                    )
                });
            let evidence = self.evidence(caller, None, false, false);
            let card = &mut self.projection.cards[portal];
            if !card.origins.iter().any(|row| row.target == evidence.target) {
                card.origins.push(evidence);
            }
            let index = card.target_set.unwrap();
            if let TargetSetKind::Calls { calls } = &mut self.projection.target_sets[index].kind {
                calls.push(call.id);
            }
            edge(
                &mut self.projection.edges,
                from,
                portal,
                EdgeKind::Call,
                FlowOrigin::CallSite { call: call.id },
            );
            for &entry in &call.entries {
                if let Some(target) = nodes.get(&entry) {
                    let target = self.target(target, None);
                    self.projection.target_sets[index]
                        .entries
                        .push(TargetEntry {
                            destination: Destination::Control(target),
                            via: FlowOrigin::Call {
                                call: call.id,
                                entry,
                            },
                        });
                }
            }
            for (delivery, receipt) in call.deliveries.iter().enumerate() {
                for successor in &receipt.successors {
                    let origin = FlowOrigin::Delivery {
                        call: call.id,
                        delivery,
                        successor: successor.to,
                    };
                    if let Some(to) = self.body(successor.to) {
                        edge(
                            &mut self.projection.edges,
                            portal,
                            to,
                            successor.kind,
                            origin,
                        );
                    }
                    if let Some(target) = nodes.get(&successor.to) {
                        let target = self.target(target, None);
                        self.projection.target_sets[index]
                            .entries
                            .push(TargetEntry {
                                destination: Destination::Control(target),
                                via: origin,
                            });
                    }
                }
            }
        }
    }
}

fn action_key(action: Option<ProcessAction>) -> u8 {
    match action {
        None => 0,
        Some(ProcessAction::Call) => 1,
        Some(ProcessAction::StaticCall) => 2,
        Some(ProcessAction::DelegateCall) => 3,
        Some(ProcessAction::CallCode) => 4,
        Some(ProcessAction::Create) => 5,
        Some(ProcessAction::Create2) => 6,
    }
}
