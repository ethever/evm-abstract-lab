//! Occurrence-level read/definition flow. Raw block identity is not an AST
//! execution occurrence: Block, Transfer and Exit have distinct source roles.
use super::{Error, K, SharedInterface, SharedSourceIndex, V, charge, value_sort};
use crate::{Edge, EntityId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
type LiveSets = BTreeMap<usize, BTreeSet<EntityId>>;
const MAX_FLOW_SLOTS: usize = crate::MAX_LIFTED_RECORDS * 8;
#[derive(Default)]
struct Event {
    reads: BTreeSet<EntityId>,
    defines: BTreeSet<EntityId>,
    next: Vec<usize>,
}
pub(super) struct Flow {
    events: Vec<Event>,
    predecessors: Vec<Vec<usize>>,
    entry: usize,
    entries: BTreeMap<EntityId, usize>,
    members: BTreeMap<EntityId, usize>,
    loop_exits: BTreeMap<EntityId, usize>,
    live_out: BTreeMap<usize, BTreeSet<EntityId>>,
}
fn op_event(
    index: &SharedSourceIndex<'_>,
    block: EntityId,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Event, Error> {
    let high = index.high_blocks.get(&block).ok_or(Error::Missing(block))?;
    let mut e = Event::default();
    for statement in &high.statements {
        charge(
            pay,
            statement
                .outputs
                .len()
                .saturating_add(statement.inputs.len())
                .saturating_add(8),
        )?;
        let op = index
            .operations
            .get(&statement.operation)
            .ok_or(Error::Missing(statement.operation))?;
        for expr in &statement.inputs {
            let reads = index
                .expression_reads
                .get(expr)
                .ok_or(Error::Missing(*expr))?;
            charge(pay, reads.len())?;
            e.reads.extend(reads.difference(&e.defines).copied());
        }
        for v in op.memory_input.into_iter().chain(op.effect_input) {
            if !e.defines.contains(&v) {
                e.reads.insert(v);
            }
        }
        e.defines.extend(statement.outputs.iter().copied());
        e.defines.extend(op.memory_output);
        e.defines.extend(op.effect_output);
    }
    Ok(e)
}
fn edge_event(
    index: &SharedSourceIndex<'_>,
    edge: &Edge,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Event, Error> {
    let high = index
        .high_transfers
        .get(&edge.id)
        .ok_or(Error::Missing(edge.id))?;
    let target = index
        .high_blocks
        .get(&edge.to)
        .ok_or(Error::Missing(edge.to))?;
    charge(
        pay,
        high.arguments
            .len()
            .saturating_add(target.parameters.len())
            .saturating_add(8),
    )?;
    let mut e = Event::default();
    for expr in &high.arguments {
        let reads = index
            .expression_reads
            .get(expr)
            .ok_or(Error::Missing(*expr))?;
        charge(pay, reads.len())?;
        e.reads.extend(reads.iter().copied());
    }
    e.reads.extend([edge.memory, edge.effect]);
    let raw = index.blocks.get(&edge.to).ok_or(Error::Missing(edge.to))?;
    e.defines.extend(target.parameters.iter().copied());
    e.defines
        .extend([raw.memory_parameter, raw.effect_parameter]);
    Ok(e)
}
fn edge_footprint(index: &SharedSourceIndex<'_>, id: EntityId) -> Result<usize, Error> {
    let edge = index.edges.get(&id).ok_or(Error::Missing(id))?;
    let high = index.high_transfers.get(&id).ok_or(Error::Missing(id))?;
    let target = index
        .high_blocks
        .get(&edge.to)
        .ok_or(Error::Missing(edge.to))?;
    index.blocks.get(&edge.to).ok_or(Error::Missing(edge.to))?;
    Ok(high
        .arguments
        .iter()
        .fold(8_usize.saturating_add(target.parameters.len()), |n, e| {
            n.saturating_add(index.expression_reads.get(e).map_or(0, BTreeSet::len))
        }))
}
impl Flow {
    pub(super) fn new(
        index: &SharedSourceIndex<'_>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, Error> {
        Self::new_with_slots(index, pay, MAX_FLOW_SLOTS)
    }
    pub(super) fn new_with_slots(
        index: &SharedSourceIndex<'_>,
        pay: &mut impl FnMut(usize) -> bool,
        max_slots: usize,
    ) -> Result<Self, Error> {
        charge(pay, index.nodes.len().saturating_mul(16))?;
        let mut flow = Self {
            events: Vec::new(),
            predecessors: Vec::new(),
            entry: 0,
            entries: BTreeMap::new(),
            members: BTreeMap::new(),
            loop_exits: BTreeMap::new(),
            live_out: BTreeMap::new(),
        };
        for id in index.nodes.keys() {
            flow.members.insert(*id, flow.events.len());
            flow.events.push(Event::default());
        }
        // Node.entry is computed bottom-up without recursive descent.
        let mut ordered: Vec<_> = index.order.iter().map(|(id, n)| (*n, *id)).collect();
        ordered.sort_unstable();
        for (_, id) in ordered.iter().rev() {
            let entry = match &index.nodes[id].kind {
                K::Sequence(v) => v
                    .first()
                    .map(|n| flow.entries[&n.id])
                    .unwrap_or(flow.members[id]),
                K::While { header, .. } => flow.entries[&header.id],
                _ => flow.members[id],
            };
            flow.entries.insert(*id, entry);
        }
        let root = index
            .ir
            .structured
            .as_ref()
            .ok_or(Error::Region(EntityId(0)))?
            .root
            .id;
        flow.entry = flow.entries[&root];
        let mut todo = vec![(root, None, None::<(usize, Option<usize>)>)];
        let mut retained_slots = 0_usize;
        while let Some((id, next, loop_context)) = todo.pop() {
            charge(pay, 8)?;
            let event = flow.members[&id];
            let n = index.nodes[&id];
            let footprint = match &n.kind {
                K::Block { block } => {
                    let high = index.high_blocks.get(block).ok_or(Error::Missing(*block))?;
                    high.statements.iter().fold(8_usize, |n, s| {
                        s.inputs.iter().fold(
                            n.saturating_add(s.outputs.len()).saturating_add(8),
                            |n, e| {
                                n.saturating_add(
                                    index.expression_reads.get(e).map_or(0, BTreeSet::len),
                                )
                            },
                        )
                    })
                }
                K::Transfer { edge } => edge_footprint(index, *edge)?,
                K::While {
                    condition,
                    exit_edge,
                    ..
                } => {
                    let control = 16_usize.saturating_add(
                        index
                            .controls
                            .get(&id)
                            .and_then(|c| index.expression_reads.get(&c.condition))
                            .map_or(0, BTreeSet::len),
                    );
                    if condition.is_some() {
                        control.saturating_add(
                            exit_edge
                                .map(|e| edge_footprint(index, e))
                                .transpose()?
                                .unwrap_or(8),
                        )
                    } else {
                        control
                    }
                }
                K::Dispatcher { .. } => return Err(Error::Region(id)),
                K::Exit { block } | K::PartialExit { block } | K::Indirect { block, .. } => {
                    index.high_blocks.get(block).map_or(16_usize, |b| {
                        b.exit_values
                            .iter()
                            .copied()
                            .chain(b.target)
                            .chain(b.condition)
                            .fold(16_usize, |n, e| {
                                n.saturating_add(
                                    index.expression_reads.get(&e).map_or(0, BTreeSet::len),
                                )
                            })
                    })
                }
                _ => 16_usize.saturating_add(
                    index
                        .controls
                        .get(&id)
                        .and_then(|c| index.expression_reads.get(&c.condition))
                        .map_or(0, BTreeSet::len),
                ),
            };
            charge(pay, footprint.saturating_mul(4))?;
            let required = retained_slots.saturating_add(footprint.saturating_mul(2));
            if required > max_slots {
                return Err(Error::Capacity {
                    resource: crate::SharedCapacity::FlowEventSlots,
                    required,
                    limit: max_slots,
                });
            }

            let mut e = Event::default();
            match &n.kind {
                K::Sequence(v) => {
                    e.next
                        .extend(v.first().map(|n| flow.entries[&n.id]).or(next));
                    for (i, n) in v.iter().enumerate().rev() {
                        todo.push((
                            n.id,
                            v.get(i + 1).map(|n| flow.entries[&n.id]).or(next),
                            loop_context,
                        ));
                    }
                }
                K::Block { block } => {
                    e = op_event(index, *block, pay)?;
                    e.next.extend(next);
                }
                K::Transfer { edge } => {
                    let edge = index.edges.get(edge).ok_or(Error::Missing(*edge))?;
                    e = edge_event(index, edge, pay)?;
                    e.next.extend(next);
                }
                K::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    e.next
                        .extend([flow.entries[&then_branch.id], flow.entries[&else_branch.id]]);
                    todo.push((else_branch.id, next, loop_context));
                    todo.push((then_branch.id, next, loop_context));
                }
                K::While {
                    header,
                    condition,
                    body,
                    exit_edge,
                    ..
                } => {
                    let head = flow.entries[&header.id];
                    e.next.push(flow.entries[&body.id]);
                    if condition.is_some() {
                        let extra = exit_edge
                            .map(|e| edge_footprint(index, e))
                            .transpose()?
                            .unwrap_or(8);
                        charge(pay, extra.saturating_mul(4))?;
                        let required = retained_slots.saturating_add(extra.saturating_mul(2));
                        if required > max_slots {
                            return Err(Error::Capacity {
                                resource: crate::SharedCapacity::FlowEventSlots,
                                required,
                                limit: max_slots,
                            });
                        }
                        let exit = flow.events.len();
                        let mut end = if let Some(edge) = exit_edge {
                            let edge = index.edges.get(edge).ok_or(Error::Missing(*edge))?;
                            edge_event(index, edge, pay)?
                        } else {
                            Event::default()
                        };
                        end.next.extend(next);
                        retained_slots = retained_slots
                            .saturating_add(end.reads.len())
                            .saturating_add(end.defines.len())
                            .saturating_add(end.next.len());
                        if retained_slots > max_slots {
                            return Err(Error::Capacity {
                                resource: crate::SharedCapacity::FlowEventSlots,
                                required: retained_slots,
                                limit: max_slots,
                            });
                        }
                        flow.events.push(end);
                        flow.loop_exits.insert(id, exit);
                        e.next.push(exit);
                    }
                    todo.push((body.id, Some(head), Some((head, next))));
                    todo.push((header.id, Some(event), Some((head, next))));
                }
                K::Break => e.next.extend(loop_context.ok_or(Error::Region(id))?.1),
                K::Continue => e.next.push(loop_context.ok_or(Error::Region(id))?.0),
                K::Exit { .. } | K::PartialExit { .. } => {}
                K::Indirect { arms, .. } => {
                    for arm in arms {
                        e.next.push(flow.entries[&arm.body.id]);
                        todo.push((arm.body.id, next, loop_context));
                    }
                }
                K::Dispatcher { .. } => return Err(Error::Region(id)),
            }
            let high: Vec<EntityId> = match &n.kind {
                K::Block { block } => index
                    .high_blocks
                    .get(block)
                    .into_iter()
                    .flat_map(|b| b.statements.iter().flat_map(|s| s.inputs.iter().copied()))
                    .collect(),
                K::Transfer { edge } => index
                    .high_transfers
                    .get(edge)
                    .map(|t| t.arguments.clone())
                    .unwrap_or_default(),
                K::If { .. } | K::While { .. } => index
                    .controls
                    .get(&id)
                    .map(|c| vec![c.condition])
                    .unwrap_or_default(),
                K::Exit { block } | K::PartialExit { block } | K::Indirect { block, .. } => index
                    .high_blocks
                    .get(block)
                    .map(|b| {
                        b.exit_values
                            .iter()
                            .copied()
                            .chain(b.target)
                            .chain(b.condition)
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            for expr in high {
                let reads = index
                    .expression_reads
                    .get(&expr)
                    .ok_or(Error::Missing(expr))?;
                charge(pay, reads.len())?;
                e.reads.extend(reads.difference(&e.defines).copied());
            }
            charge(
                pay,
                e.reads
                    .len()
                    .saturating_add(e.defines.len())
                    .saturating_add(e.next.len()),
            )?;
            retained_slots = retained_slots
                .saturating_add(e.reads.len())
                .saturating_add(e.defines.len())
                .saturating_add(e.next.len());
            if retained_slots > max_slots {
                return Err(Error::Capacity {
                    resource: crate::SharedCapacity::FlowEventSlots,
                    required: retained_slots,
                    limit: max_slots,
                });
            }
            flow.events[event] = e;
        }
        flow.predecessors.resize_with(flow.events.len(), Vec::new);
        for (id, e) in flow.events.iter().enumerate() {
            for next in &e.next {
                flow.predecessors[*next].push(id);
            }
        }
        let all: BTreeSet<_> = (0..flow.events.len()).collect();
        let (_, out) = flow.liveness(&all, &BTreeSet::new(), pay)?;
        flow.live_out = out;
        Ok(flow)
    }
    fn liveness(
        &self,
        selected: &BTreeSet<usize>,
        boundary: &BTreeSet<EntityId>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(LiveSets, LiveSets), Error> {
        charge(pay, selected.len().saturating_mul(4))?;
        let mut incoming: BTreeMap<_, BTreeSet<EntityId>> =
            selected.iter().map(|id| (*id, BTreeSet::new())).collect();
        let mut outgoing = incoming.clone();
        let mut pending: VecDeque<_> = selected.iter().copied().collect();
        let mut queued = selected.clone();
        let mut slots = 0_usize;
        while let Some(id) = pending.pop_front() {
            queued.remove(&id);
            let event = &self.events[id];
            let capacity = event
                .next
                .iter()
                .filter(|n| selected.contains(n))
                .fold(0_usize, |n, s| n.saturating_add(incoming[s].len()))
                .saturating_add(boundary.len());
            charge(
                pay,
                capacity
                    .saturating_mul(4)
                    .saturating_add(event.reads.len())
                    .saturating_add(event.defines.len())
                    .saturating_add(1),
            )?;
            let required = slots
                .saturating_add(capacity.saturating_mul(2))
                .saturating_add(event.reads.len());
            if required > MAX_FLOW_SLOTS {
                return Err(Error::Capacity {
                    resource: crate::SharedCapacity::FlowLivenessSlots,
                    required,
                    limit: MAX_FLOW_SLOTS,
                });
            }
            let mut out = BTreeSet::new();
            for next in &event.next {
                if selected.contains(next) {
                    out.extend(incoming[next].iter().copied());
                } else {
                    out.extend(boundary.iter().copied());
                }
            }
            // No successors is terminal, except an actual lexical fallthrough
            // past the root, represented by the absence of a source successor.
            if event.next.is_empty() && !boundary.is_empty() { /* terminal exits propagate, with no internal return */
            }
            let new_in: BTreeSet<_> = event
                .reads
                .iter()
                .copied()
                .chain(out.difference(&event.defines).copied())
                .collect();
            let changed = new_in != incoming[&id];
            slots = slots
                .saturating_sub(incoming[&id].len())
                .saturating_sub(outgoing[&id].len())
                .saturating_add(new_in.len())
                .saturating_add(out.len());
            incoming.insert(id, new_in);
            outgoing.insert(id, out);
            if changed {
                for pred in &self.predecessors[id] {
                    if selected.contains(pred) && queued.insert(*pred) {
                        pending.push_back(*pred);
                    }
                }
            }
        }
        Ok((incoming, outgoing))
    }
    pub(super) fn interface(
        &self,
        index: &SharedSourceIndex<'_>,
        root: EntityId,
        nodes: &[EntityId],
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(SharedInterface, BTreeSet<EntityId>), Error> {
        let selected: BTreeSet<_> = nodes
            .iter()
            .flat_map(|n| std::iter::once(self.members[n]).chain(self.loop_exits.get(n).copied()))
            .collect();
        let mut definitions = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        for id in &selected {
            charge(
                pay,
                self.events[*id]
                    .defines
                    .len()
                    .saturating_add(self.live_out[id].len())
                    .saturating_add(1),
            )?;
            definitions.extend(self.events[*id].defines.iter().copied());
        }
        for id in &selected {
            if self.events[*id].next.iter().any(|n| !selected.contains(n)) {
                outputs.extend(self.live_out[id].intersection(&definitions).copied());
            }
        }
        let (inputs, _) = self.liveness(&selected, &outputs, pay)?;
        let inputs = &inputs[&self.entries[&root]];
        for input in inputs {
            self.available(index, *input, self.entries[&root], pay)?;
        }
        let mut result = SharedInterface::default();
        for id in inputs {
            match value_sort(index.values.get(id).ok_or(Error::Missing(*id))?) {
                0 => result.inputs.push(*id),
                1 => result.memory_inputs.push(*id),
                _ => result.effect_inputs.push(*id),
            }
        }
        for id in outputs {
            match value_sort(index.values.get(&id).ok_or(Error::Missing(id))?) {
                0 => result.outputs.push(id),
                1 => result.memory_outputs.push(id),
                _ => result.effect_outputs.push(id),
            }
        }
        Ok((result, definitions))
    }
    fn available(
        &self,
        index: &SharedSourceIndex<'_>,
        value: EntityId,
        entry: usize,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), Error> {
        let v = index.values.get(&value).ok_or(Error::Missing(value))?;
        if matches!(
            v.kind,
            V::Constant(_) | V::Memory { operation: None } | V::Effect { operation: None }
        ) || index
            .ir
            .entry
            .and_then(|id| index.blocks.get(&id))
            .is_some_and(|b| {
                b.memory_parameter == value
                    || b.effect_parameter == value
                    || b.parameters.contains(&value)
            })
        {
            return Ok(());
        }
        // Search for an entry path on which the captured value was never
        // defined. Cutting at actual definition occurrences handles loops and
        // repeated source blocks without confusing raw and syntax identity.
        charge(pay, self.predecessors[entry].len())?;
        let mut todo = self.predecessors[entry].clone();
        let mut seen = BTreeSet::new();
        if entry == self.entry {
            return Err(Error::Interface(value));
        }
        while let Some(id) = todo.pop() {
            charge(pay, self.predecessors[id].len().saturating_add(2))?;
            if !seen.insert(id) || self.events[id].defines.contains(&value) {
                continue;
            }
            if id == self.entry {
                return Err(Error::Interface(value));
            }
            todo.extend(self.predecessors[id].iter().copied());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
