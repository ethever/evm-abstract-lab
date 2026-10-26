use std::collections::BTreeMap;

use super::{
    Builder, CardKind, Destination, FlowOrigin, GraphLevel, TargetEntry, TargetSet, TargetSetKind,
    edge,
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
            for successor in node
                .transfer
                .iter()
                .flat_map(|transfer| &transfer.successors)
            {
                let Some(target) = nodes.get(&successor.to) else {
                    continue;
                };
                let origin = FlowOrigin::Regional {
                    from: node.id,
                    to: successor.to,
                };
                self.retain_relation(node, target, Some(successor.kind), origin);
                let Some(level) = self.transfer_level(node, target, successor.kind) else {
                    continue;
                };
                if let Some(to) = self.body(successor.to) {
                    edge(
                        &mut self.projection.edges,
                        from,
                        to,
                        successor.kind,
                        level,
                        origin,
                    );
                } else {
                    outside.push((
                        successor.kind,
                        level,
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
                        entries: outside.iter().map(|(_, _, entry)| *entry).collect(),
                    },
                );
                for (kind, level, entry) in outside {
                    edge(
                        &mut self.projection.edges,
                        from,
                        portal,
                        kind,
                        level,
                        entry.via,
                    );
                }
            }
            // Membership is not a category. Delivered continuations and older
            // admissions remain accessible without inventing Fallthrough.
            for &target in &node.successors {
                if node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.successors.iter().any(|edge| edge.to == target)
                }) {
                    continue;
                }
                if let Some(target) = nodes.get(&target) {
                    self.retain_relation(
                        node,
                        target,
                        None,
                        FlowOrigin::Topology {
                            from: node.id,
                            to: target.id,
                        },
                    );
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
            for &entry in &call.entries {
                if let Some(target) = nodes.get(&entry) {
                    self.retain_relation(
                        caller,
                        target,
                        Some(EdgeKind::Call),
                        FlowOrigin::Call {
                            call: call.id,
                            entry,
                        },
                    );
                }
            }
            for (delivery, receipt) in call.deliveries.iter().enumerate() {
                for successor in &receipt.successors {
                    if let Some(target) = nodes.get(&successor.to) {
                        self.retain_relation(
                            caller,
                            target,
                            Some(successor.kind),
                            FlowOrigin::Delivery {
                                call: call.id,
                                delivery,
                                successor: successor.to,
                            },
                        );
                    }
                }
            }
            let Some(level) = self.call_level(caller, call.action) else {
                continue;
            };
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
                level,
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
                    if self.projection.level == GraphLevel::Model
                        && let Some(to) = self.body(entry)
                    {
                        edge(
                            &mut self.projection.edges,
                            portal,
                            to,
                            EdgeKind::Call,
                            level,
                            FlowOrigin::Call {
                                call: call.id,
                                entry,
                            },
                        );
                    }
                }
            }
            for (delivery, receipt) in call.deliveries.iter().enumerate() {
                for successor in &receipt.successors {
                    let origin = FlowOrigin::Delivery {
                        call: call.id,
                        delivery,
                        successor: successor.to,
                    };
                    if let Some(to) = self.body(successor.to)
                        && let Some(target) = nodes.get(&successor.to)
                        && let Some(level) = self.continuation_level(caller, target, call.action)
                    {
                        edge(
                            &mut self.projection.edges,
                            portal,
                            to,
                            successor.kind,
                            level,
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
