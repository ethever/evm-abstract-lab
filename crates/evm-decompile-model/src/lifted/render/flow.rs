//! Closed raw control flow and paid dominance intervals for source bindings.
//! Every registered edge must have one terminator owner. Open or malformed
//! graphs keep local bindings available but cannot certify ancestor bindings.
use super::LiftedRenderError as Error;
use crate::{EntityId, Ir, Terminator};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Flow {
    blocks: BTreeMap<EntityId, usize>,
    predecessors: Vec<Vec<usize>>,
    dominance: Vec<Option<(usize, usize)>>,
    closed: bool,
}
impl Flow {
    pub(super) fn new(ir: &Ir, pay: &mut impl FnMut(usize) -> bool) -> Result<Self, Error> {
        paid(pay, ir.blocks.len().saturating_mul(2).saturating_add(1))?;
        let blocks: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let (mut closed, successors, predecessors) = cfg(ir, &blocks, pay)?;
        closed &= blocks.len() == ir.blocks.len();
        let dominance = if closed {
            dominance(ir, &blocks, &successors, &predecessors, pay)?
        } else {
            Vec::new()
        };
        Ok(Self {
            blocks,
            predecessors,
            dominance,
            closed,
        })
    }
    pub(super) fn closed(&self) -> bool {
        self.closed
    }
    pub(super) fn predecessors(&self, block: usize) -> &[usize] {
        &self.predecessors[block]
    }
    pub(super) fn strictly_dominates(&self, definition: EntityId, consumer: EntityId) -> bool {
        let (Some(definition), Some(consumer)) =
            (self.blocks.get(&definition), self.blocks.get(&consumer))
        else {
            return false;
        };
        match (
            self.dominance.get(*definition),
            self.dominance.get(*consumer),
        ) {
            (Some(Some((start, end))), Some(Some((point, _)))) => start < point && point < end,
            _ => false,
        }
    }
}
fn paid(pay: &mut impl FnMut(usize) -> bool, units: usize) -> Result<(), Error> {
    if pay(units) {
        Ok(())
    } else {
        Err(Error::Limit)
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
    let mut closed = ir.fact_scope == crate::FactScope::CompleteLocalCfg
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
