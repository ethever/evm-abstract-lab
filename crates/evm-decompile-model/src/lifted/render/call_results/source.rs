//! Independent source facts for the display's immutable return-buffer names.
//! A loop may keep an anchored epoch or execute a fresh CALL before a read.
//! Unanchored cycles, open joins and conflicting epochs never produce a name.
use super::super::LiftedRenderError as Error;
use crate::{Effects, EntityId, FactScope, Ir, Operation, Terminator, ValueKind};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[cfg(test)]
pub(super) mod tests;

pub(super) struct CallSources<'a> {
    operations: BTreeMap<EntityId, &'a Operation>,
    positions: BTreeMap<EntityId, (EntityId, usize)>,
    blocks: BTreeMap<EntityId, usize>,
    calls: BTreeSet<EntityId>,
    origins: BTreeMap<EntityId, EntityId>,
    words: BTreeMap<EntityId, EntityId>,
    dominance: Vec<Option<(usize, usize)>>,
}
impl CallSources<'_> {
    pub(super) fn origin(&self, operation: EntityId) -> Option<EntityId> {
        self.origins.get(&operation).copied()
    }
    pub(super) fn available(&self, call: EntityId, use_block: EntityId, before: usize) -> bool {
        if !self.calls.contains(&call) {
            return false;
        }
        let Some((owner, position)) = self.position(call) else {
            return false;
        };
        if owner == use_block {
            return position < before;
        }
        let (Some(owner), Some(consumer)) = (self.blocks.get(&owner), self.blocks.get(&use_block))
        else {
            return false;
        };
        let (Some((start, end)), Some((point, _))) =
            (self.dominance[*owner], self.dominance[*consumer])
        else {
            return false;
        };
        start <= point && point < end
    }
    pub(super) fn position(&self, operation: EntityId) -> Option<(EntityId, usize)> {
        self.positions.get(&operation).copied()
    }
    pub(super) fn operation(&self, operation: EntityId) -> Option<&Operation> {
        self.operations.get(&operation).copied()
    }
    pub(super) fn word_origin(&self, word: EntityId) -> Option<EntityId> {
        self.words.get(&word).copied()
    }
}

fn paid(pay: &mut impl FnMut(usize) -> bool, units: usize) -> Result<(), Error> {
    if pay(units) {
        Ok(())
    } else {
        Err(Error::Limit)
    }
}

/// The four CALL-family primitives return a success word. CREATE's address is
/// deliberately excluded, although CREATE still resets the buffer epoch.
pub(super) fn call_shape(op: &Operation) -> bool {
    let count = match op.opcode {
        0xf1 | 0xf2 => 7,
        0xf4 | 0xfa => 6,
        _ => return false,
    };
    let mutable = op.opcode != 0xfa;
    op.inputs.len() == count
        && op.outputs.len() == 1
        && op.memory_input.is_some()
        && op.memory_output.is_some()
        && op.effect_input.is_some()
        && op.effect_output.is_some()
        && op.effects
            == Effects {
                memory_read: true,
                memory_write: true,
                storage_read: true,
                storage_write: mutable,
                transient_read: true,
                transient_write: mutable,
                world_read: true,
                world_write: mutable,
                call: true,
                return_data_write: true,
                ..Effects::default()
            }
}

fn create_shape(op: &Operation) -> bool {
    let count = match op.opcode {
        0xf0 => 3,
        0xf5 => 4,
        _ => return false,
    };
    op.inputs.len() == count
        && op.outputs.len() == 1
        && op.memory_input.is_some()
        && op.memory_output.is_none()
        && op.effect_input.is_some()
        && op.effect_output.is_some()
        && op.effects
            == Effects {
                memory_read: true,
                storage_read: true,
                storage_write: true,
                transient_read: true,
                transient_write: true,
                world_read: true,
                world_write: true,
                create: true,
                return_data_write: true,
                ..Effects::default()
            }
}

fn read_shape(op: &Operation) -> bool {
    (match op.opcode {
        0x3d => {
            op.inputs.is_empty()
                && op.outputs.len() == 1
                && op.memory_input.is_none()
                && op.memory_output.is_none()
        }
        0x3e => {
            op.inputs.len() == 3
                && op.outputs.is_empty()
                && op.memory_input.is_some()
                && op.memory_output.is_some()
        }
        _ => return false,
    }) && op.effect_input.is_some()
        && op.effect_output.is_some()
        && op.effects
            == Effects {
                memory_write: op.opcode == 0x3e,
                return_data_read: true,
                ..Effects::default()
            }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Epoch {
    Pending,
    Call(EntityId),
    Unknown,
}
enum Equation {
    Anchor(EntityId),
    Join(Vec<usize>),
    Unknown,
}

pub(super) fn analyze<'a>(
    ir: &'a Ir,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<CallSources<'a>, Error> {
    let units = ir.blocks.iter().fold(
        ir.values
            .len()
            .saturating_add(ir.edges.len())
            .saturating_add(ir.blocks.len()),
        |n, block| {
            block
                .operations
                .iter()
                .fold(n.saturating_add(block.parameters.len()), |n, op| {
                    n.saturating_add(op.inputs.len())
                        .saturating_add(op.outputs.len())
                        .saturating_add(8)
                })
        },
    );
    paid(pay, units.saturating_mul(6).saturating_add(1))?;
    let mut result = CallSources {
        operations: BTreeMap::new(),
        positions: BTreeMap::new(),
        blocks: BTreeMap::new(),
        calls: BTreeSet::new(),
        origins: BTreeMap::new(),
        words: BTreeMap::new(),
        dominance: vec![None; ir.blocks.len()],
    };
    let mut unique = true;
    for (index, block) in ir.blocks.iter().enumerate() {
        unique &= result.blocks.insert(block.id, index).is_none();
        for (position, op) in block.operations.iter().enumerate() {
            unique &= result.operations.insert(op.id, op).is_none();
            result.positions.insert(op.id, (block.id, position));
            if call_shape(op) || create_shape(op) {
                result.calls.insert(op.id);
            }
        }
    }
    let values: BTreeMap<_, _> = ir
        .values
        .iter()
        .map(|value| (value.id, &value.kind))
        .collect();
    if !unique || values.len() != ir.values.len() {
        result.operations.clear();
        result.positions.clear();
        result.blocks.clear();
        result.calls.clear();
        return Ok(result);
    }
    let (closed, successors, predecessors) = cfg(ir, &result.blocks, pay)?;
    if closed {
        result.dominance = dominance(ir, &result.blocks, &successors, &predecessors, pay)?;
    }
    let mut equations: Vec<_> = (0..ir.blocks.len()).map(|_| Equation::Unknown).collect();
    let mut outgoing = vec![0; ir.blocks.len()];
    let mut readers = Vec::new();
    for (index, block) in ir.blocks.iter().enumerate() {
        let mut current = index;
        for op in &block.operations {
            paid(pay, 2)?;
            if read_shape(op) {
                readers.push((op.id, current));
            }
            // Opcodes reset the epoch even if forged effect metadata claims
            // otherwise. A malformed CALL becomes Unknown, never transparent.
            if matches!(op.opcode, 0xf0..=0xf2 | 0xf4 | 0xf5 | 0xfa) || op.effects.return_data_write
            {
                paid(pay, 2)?;
                current = equations.len();
                equations.push(if result.calls.contains(&op.id) {
                    Equation::Anchor(op.id)
                } else {
                    Equation::Unknown
                });
            }
        }
        outgoing[index] = current;
    }
    if closed {
        for (index, block) in ir.blocks.iter().enumerate() {
            let inputs = &predecessors[index];
            if Some(block.id) != ir.entry && !inputs.is_empty() {
                paid(pay, inputs.len().saturating_mul(2))?;
                equations[index] = Equation::Join(inputs.iter().map(|p| outgoing[*p]).collect());
            }
        }
    }
    let epochs = solve(&equations, pay)?;
    for (operation, state) in readers {
        paid(pay, 4)?;
        let op = result.operations[&operation];
        let Epoch::Call(call) = epochs[state] else {
            continue;
        };
        let (block, position) = result.positions[&op.id];
        if !result.available(call, block, position) {
            continue;
        }
        result.origins.insert(op.id, call);
        if op.opcode == 0x3d && values.get(&op.outputs[0]).is_some_and(|kind|
            matches!(kind, ValueKind::Operation {operation,index:0} if *operation == op.id)) {
            result.words.insert(op.outputs[0], call);
        }
    }
    Ok(result)
}

fn solve(equations: &[Equation], pay: &mut impl FnMut(usize) -> bool) -> Result<Vec<Epoch>, Error> {
    let links = equations.iter().fold(0usize, |n, equation| {
        n.saturating_add(match equation {
            Equation::Join(children) => children.len(),
            _ => 0,
        })
    });
    paid(
        pay,
        equations
            .len()
            .saturating_mul(6)
            .saturating_add(links.saturating_mul(3)),
    )?;
    let mut epochs: Vec<_> = equations
        .iter()
        .map(|equation| match equation {
            Equation::Anchor(call) => Epoch::Call(*call),
            Equation::Unknown => Epoch::Unknown,
            Equation::Join(_) => Epoch::Pending,
        })
        .collect();
    let mut users = vec![Vec::new(); equations.len()];
    for (parent, equation) in equations.iter().enumerate() {
        if let Equation::Join(children) = equation {
            for child in children {
                users[*child].push(parent);
            }
        }
    }
    let mut queued = vec![true; equations.len()];
    let mut pending: VecDeque<_> = (0..equations.len()).collect();
    for phase in 0..2 {
        if phase == 1 {
            // Bottom is useful while an anchored loop converges, but cannot be
            // ignored forever. Unknown/no-anchor SCCs poison their consumers.
            for index in 0..epochs.len() {
                paid(pay, 1)?;
                if epochs[index] != Epoch::Pending {
                    continue;
                }
                epochs[index] = Epoch::Unknown;
                for user in &users[index] {
                    if !queued[*user] {
                        queued[*user] = true;
                        pending.push_back(*user);
                    }
                }
            }
        }
        while let Some(index) = pending.pop_front() {
            queued[index] = false;
            let Equation::Join(children) = &equations[index] else {
                continue;
            };
            paid(
                pay,
                children
                    .len()
                    .saturating_add(users[index].len())
                    .saturating_add(2),
            )?;
            let mut next = Epoch::Pending;
            for child in children {
                next = merge(next, epochs[*child]);
                if next == Epoch::Unknown {
                    break;
                }
            }
            next = merge(epochs[index], next);
            if next == epochs[index] {
                continue;
            }
            epochs[index] = next;
            for user in &users[index] {
                if !queued[*user] {
                    queued[*user] = true;
                    pending.push_back(*user);
                }
            }
        }
    }
    Ok(epochs)
}
fn merge(a: Epoch, b: Epoch) -> Epoch {
    match (a, b) {
        (Epoch::Unknown, _) | (_, Epoch::Unknown) => Epoch::Unknown,
        (Epoch::Pending, x) | (x, Epoch::Pending) => x,
        (Epoch::Call(a), Epoch::Call(b)) if a == b => Epoch::Call(a),
        _ => Epoch::Unknown,
    }
}

type Adjacency = Vec<Vec<usize>>;
fn cfg(
    ir: &Ir,
    blocks: &BTreeMap<EntityId, usize>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<(bool, Adjacency, Adjacency), Error> {
    paid(
        pay,
        ir.blocks
            .len()
            .saturating_mul(3)
            .saturating_add(ir.edges.len().saturating_mul(5)),
    )?;
    let mut closed = ir.fact_scope == FactScope::CompleteLocalCfg
        && ir.entry.is_some_and(|entry| blocks.contains_key(&entry));
    let edges: BTreeMap<_, _> = ir.edges.iter().map(|edge| (edge.id, edge)).collect();
    closed &= edges.len() == ir.edges.len();
    let mut used = BTreeSet::new();
    let mut successors = vec![Vec::new(); ir.blocks.len()];
    let mut predecessors = vec![Vec::new(); ir.blocks.len()];
    for (index, block) in ir.blocks.iter().enumerate() {
        let known = match &block.terminator {
            Terminator::Fallthrough { .. } | Terminator::Exit { .. } | Terminator::Trap { .. } => {
                true
            }
            Terminator::Jump {
                edges,
                unresolved: false,
                invalid_targets,
                ..
            } => !edges.is_empty() && invalid_targets.is_empty(),
            Terminator::Branch {
                then_edges,
                else_edge: Some(_),
                unresolved: false,
                invalid_targets,
                ..
            } => !then_edges.is_empty() && invalid_targets.is_empty(),
            _ => false,
        };
        closed &= known;
        let count = match &block.terminator {
            Terminator::Fallthrough { .. } => 1,
            Terminator::Jump { edges, .. } => edges.len(),
            Terminator::Branch {
                then_edges,
                else_edge,
                ..
            } => then_edges
                .len()
                .saturating_add(usize::from(else_edge.is_some())),
            _ => 0,
        };
        paid(pay, count.saturating_mul(4).saturating_add(1))?;
        for id in block.terminator.edges() {
            let Some(edge) = edges.get(&id) else {
                closed = false;
                continue;
            };
            let Some(to) = blocks.get(&edge.to).copied() else {
                closed = false;
                continue;
            };
            if edge.from != block.id || !used.insert(id) {
                closed = false;
                continue;
            }
            successors[index].push(to);
            predecessors[to].push(index);
        }
    }
    closed &= used.len() == ir.edges.len();
    Ok((closed, successors, predecessors))
}

fn dominance(
    ir: &Ir,
    blocks: &BTreeMap<EntityId, usize>,
    successors: &Adjacency,
    predecessors: &Adjacency,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Vec<Option<(usize, usize)>>, Error> {
    let n = ir.blocks.len();
    paid(pay, n.saturating_mul(12).saturating_add(1))?;
    let entry = blocks[&ir.entry.expect("closed CFG has an entry")];
    let mut seen = vec![false; n];
    let mut postorder = Vec::new();
    let mut stack = vec![(entry, 0)];
    seen[entry] = true;
    while let Some((node, next)) = stack.last_mut() {
        paid(pay, 1)?;
        if let Some(child) = successors[*node].get(*next).copied() {
            *next += 1;
            if !seen[child] {
                seen[child] = true;
                stack.push((child, 0));
            }
        } else {
            postorder.push(*node);
            stack.pop();
        }
    }
    postorder.reverse();
    let mut rank = vec![usize::MAX; n];
    for (index, node) in postorder.iter().enumerate() {
        rank[*node] = index;
    }
    let mut parent: Vec<Option<usize>> = vec![None; n];
    parent[entry] = Some(entry);
    let mut changed = true;
    while changed {
        changed = false;
        for node in postorder.iter().copied().skip(1) {
            paid(pay, predecessors[node].len().saturating_add(1))?;
            let mut next: Option<usize> = None;
            for predecessor in predecessors[node]
                .iter()
                .copied()
                .filter(|p| parent[*p].is_some())
            {
                next = Some(match next {
                    None => predecessor,
                    Some(mut common) => {
                        let mut other = predecessor;
                        while common != other {
                            paid(pay, 1)?;
                            if rank[common] > rank[other] {
                                common = parent[common].unwrap();
                            } else {
                                other = parent[other].unwrap();
                            }
                        }
                        common
                    }
                });
            }
            if parent[node] != next {
                parent[node] = next;
                changed = true;
            }
        }
    }
    let mut children = vec![Vec::new(); n];
    for node in postorder.iter().copied().skip(1) {
        children[parent[node].expect("reachable node has a dominator")].push(node);
    }
    let mut spans: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut pending = vec![(entry, false)];
    let mut clock = 0;
    while let Some((node, exit)) = pending.pop() {
        paid(pay, children[node].len().saturating_add(2))?;
        if exit {
            spans[node].as_mut().expect("entered tree node").1 = clock;
        } else {
            spans[node] = Some((clock, clock));
            clock += 1;
            pending.push((node, true));
            pending.extend(children[node].iter().rev().map(|child| (*child, false)));
        }
    }
    Ok(spans)
}
