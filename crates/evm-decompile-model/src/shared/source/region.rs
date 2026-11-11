//! Bounded semantic/source domain collection and occurrence interfaces.
use super::{Error, SharedRegion, SharedSourceIndex, charge};
use super::{MAX_SHARED_REGION_ENTITIES, MAX_SHARED_REGION_NODES};
use crate::EntityId;
use crate::{SharedContinuation, SharedProcedureKind, StructuredKind as K, Terminator as T};
use std::collections::BTreeSet;
impl SharedSourceIndex<'_> {
    /// Inspect a supported fragment of the certified logical projection.
    pub fn region(
        &self,
        root: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<SharedRegion, Error> {
        if self.flow.is_none() {
            return Err(Error::Region(root));
        }
        let mut nodes = Vec::new();
        let mut blocks = Vec::new();
        let mut block_set = BTreeSet::new();
        let mut entities = BTreeSet::new();
        let mut transfers = BTreeSet::new();
        let mut exprs = Vec::new();
        let mut operations = 0_usize;
        let mut retained_operations = 0_usize;
        let mut pure_arithmetic = true;
        let mut todo = vec![(root, 0_usize)];
        while let Some((id, loops)) = todo.pop() {
            charge(pay, 8)?;
            if nodes.len() >= MAX_SHARED_REGION_NODES {
                return Err(Error::Region(root));
            }
            let node = self.nodes.get(&id).ok_or(Error::Missing(id))?;
            nodes.push(id);
            entities.insert(id);
            match &node.kind {
                K::Sequence(v) => {
                    if nodes
                        .len()
                        .saturating_add(todo.len())
                        .saturating_add(v.len())
                        > MAX_SHARED_REGION_NODES
                    {
                        return Err(Error::Region(root));
                    }
                    charge(pay, v.len())?;
                    todo.extend(v.iter().rev().map(|n| (n.id, loops)));
                }
                K::Block { block } => {
                    let raw = self.blocks.get(block).ok_or(Error::Missing(*block))?;
                    charge(pay, raw.operations.len().saturating_mul(4))?;
                    if raw.terminator.is_partial()
                        || matches!(raw.terminator, T::Exit { .. } | T::Trap { .. })
                        || matches!(&raw.terminator,T::Jump{invalid_targets,..}|T::Branch{invalid_targets,..} if !invalid_targets.is_empty())
                        || matches!(&raw.terminator, T::Jump { edges, .. } if edges.is_empty())
                        || matches!(&raw.terminator, T::Branch { then_edges, else_edge, .. } if then_edges.is_empty() || else_edge.is_none())
                        || raw.operations.iter().any(|op| {
                            matches!(op.opcode, 0x00 | 0xf0..=0xf5 | 0xfa | 0xfd..=0xff)
                                || op.effects.call
                                || op.effects.create
                                || op.effects.terminate
                        })
                    {
                        return Err(Error::Region(root));
                    }
                    pure_arithmetic &= raw.operations.iter().all(|op| {
                        op.effects.is_pure() && super::arithmetic::arithmetic_primitive(op.opcode)
                    });
                    let high = self.high_blocks.get(block).ok_or(Error::Missing(*block))?;
                    if block_set.insert(*block) {
                        blocks.push(*block);
                    }
                    if entities
                        .len()
                        .saturating_add(high.parameters.len())
                        .saturating_add(4)
                        > MAX_SHARED_REGION_ENTITIES
                    {
                        return Err(Error::Region(root));
                    }
                    entities.extend([*block, high.id, raw.memory_parameter, raw.effect_parameter]);
                    entities.extend(high.parameters.iter().copied());
                    operations = operations.saturating_add(raw.operations.len());
                    retained_operations = retained_operations.saturating_add(high.statements.len());
                    for statement in &high.statements {
                        charge(
                            pay,
                            statement
                                .inputs
                                .len()
                                .saturating_add(statement.outputs.len())
                                .saturating_add(8),
                        )?;
                        let op = self
                            .operations
                            .get(&statement.operation)
                            .ok_or(Error::Missing(statement.operation))?;
                        if entities
                            .len()
                            .saturating_add(statement.outputs.len())
                            .saturating_add(6)
                            > MAX_SHARED_REGION_ENTITIES
                            || exprs.len().saturating_add(statement.inputs.len())
                                > MAX_SHARED_REGION_ENTITIES
                        {
                            return Err(Error::Region(root));
                        }
                        if statement.inputs.len() != op.inputs.len() {
                            return Err(Error::Correspondence(statement.id));
                        }
                        entities.extend([statement.id, statement.operation]);
                        entities.extend(statement.outputs.iter().copied());
                        entities.extend(op.memory_input);
                        entities.extend(op.memory_output);
                        entities.extend(op.effect_input);
                        entities.extend(op.effect_output);
                        exprs.extend(statement.inputs.iter().copied());
                    }
                    if let Some(c) = self.constructions.get(block) {
                        // Occurrence-local symbolic data has no invocation
                        // binding contract yet. Retain that source inline.
                        let r = self.reverts.get(&c.revert).ok_or(Error::Region(root))?;
                        if entities
                            .len()
                            .saturating_add(r.writes.len())
                            .saturating_add(8)
                            > MAX_SHARED_REGION_ENTITIES
                        {
                            return Err(Error::Region(root));
                        }
                        charge(pay, r.writes.len().saturating_add(8))?;
                        entities.extend([
                            c.id,
                            r.id,
                            r.payload,
                            r.operation,
                            r.memory,
                            r.offset,
                            r.size,
                        ]);
                        entities.extend(r.writes.iter().copied());
                    }
                }
                K::Exit { .. } => return Err(Error::Region(root)),
                K::Transfer { edge } => {
                    transfers.insert(*edge);
                }
                K::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    todo.push((else_branch.id, loops));
                    todo.push((then_branch.id, loops));
                }
                K::While {
                    header,
                    body,
                    condition,
                    exit_edge,
                    ..
                } => {
                    if condition.is_none() && exit_edge.is_some() {
                        return Err(Error::Region(root));
                    }
                    transfers.extend(*exit_edge);
                    todo.push((body.id, loops + 1));
                    todo.push((header.id, loops + 1));
                }
                K::Break | K::Continue if loops > 0 => {}
                _ => return Err(Error::Region(root)),
            }
            if matches!(
                node.kind,
                K::If { .. }
                    | K::While {
                        condition: Some(_),
                        ..
                    }
            ) {
                let c = self.controls.get(&id).ok_or(Error::Missing(id))?;
                entities.insert(c.id);
                exprs.push(c.condition);
                if let Some(g) = &c.guard {
                    entities.extend(g.source_references());
                    entities.extend(match g.payload {
                        crate::LiftedGuardPayload::ErrorString(id) => Some(id),
                        _ => None,
                    });
                }
            }
        }
        let selected: BTreeSet<_> = nodes.iter().copied().collect();
        for id in &nodes {
            if self
                .controls
                .get(id)
                .and_then(|c| c.guard.as_ref())
                .and_then(|g| g.continuation.as_ref())
                .is_some_and(|c| !selected.contains(&c.sequence))
            {
                return Err(Error::Region(root));
            }
        }
        for id in &transfers {
            let e = self.edges.get(id).ok_or(Error::Missing(*id))?;
            let h = self.high_transfers.get(id).ok_or(Error::Missing(*id))?;
            let to = self.blocks.get(&e.to).ok_or(Error::Missing(e.to))?;
            let high = self.high_blocks.get(&e.to).ok_or(Error::Missing(e.to))?;
            charge(
                pay,
                h.arguments
                    .len()
                    .saturating_add(high.parameters.len())
                    .saturating_add(8),
            )?;
            if entities
                .len()
                .saturating_add(high.parameters.len())
                .saturating_add(8)
                > MAX_SHARED_REGION_ENTITIES
                || exprs.len().saturating_add(h.arguments.len()) > MAX_SHARED_REGION_ENTITIES
            {
                return Err(Error::Region(root));
            }
            entities.extend([
                *id,
                h.id,
                e.from,
                e.to,
                e.memory,
                e.effect,
                to.memory_parameter,
                to.effect_parameter,
            ]);
            entities.extend(high.parameters.iter().copied());
            exprs.extend(h.arguments.iter().copied());
        }
        let mut visited = BTreeSet::new();
        while let Some(id) = exprs.pop() {
            charge(pay, 4)?;
            if !visited.insert(id) {
                continue;
            }
            let e = self.expressions.get(&id).ok_or(Error::Missing(id))?;
            if let crate::LiftedExpressionKind::WordOp { opcode, .. } = e.kind {
                pure_arithmetic &= super::arithmetic::arithmetic_opcode(opcode);
            }
            if entities.len().saturating_add(3) > MAX_SHARED_REGION_ENTITIES {
                return Err(Error::Region(root));
            }
            entities.extend([id, e.block]);
            if let crate::LiftedExpressionKind::Value(value) = e.kind {
                entities.insert(value);
            }
            let args = e.kind.operands();
            charge(pay, args.len())?;
            exprs.extend(args);
            if entities.len() > MAX_SHARED_REGION_ENTITIES {
                return Err(Error::Region(root));
            }
        }
        let flow = self.flow.as_ref().ok_or(Error::Region(root))?;
        flow.check_boundary(self, root, &nodes, pay)?;
        let (mut interface, _locals) = flow.interface(self, root, &nodes, pay)?;
        for edge in &transfers {
            let e = self.edges[edge];
            if !block_set.contains(&e.to) {
                interface.continuations.push(SharedContinuation {
                    edge: *edge,
                    target: e.to,
                });
            }
        }
        if interface.references().any(|id| !entities.contains(&id)) {
            return Err(Error::Interface(root));
        }
        Ok(SharedRegion {
            kind: if pure_arithmetic {
                SharedProcedureKind::PureArithmetic
            } else {
                SharedProcedureKind::Normal
            },
            root,
            nodes,
            blocks,
            entities: entities.into_iter().collect(),
            interface,
            operations,
            retained_operations,
            terminal_closed: true,
        })
    }
}
