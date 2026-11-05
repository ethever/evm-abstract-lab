//! Identity-only phi equations and their raw definition availability proof.
//!
//! An SCC is a copy only when all external inputs have one canonical identity,
//! with an actual anchor and a definition strictly dominating every entry. No
//! abstract value facts or individual assignment unions establish equivalence.
use super::availability::Availability;
use crate::{EntityId, FactScope, Ir, Value, ValueKind};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod components;
mod discover;
mod validate;

#[derive(Clone, Copy)]
pub(super) struct Definition {
    pub(super) block: usize,
    /// None denotes a block-entry binding; Some is the primitive order index.
    pub(super) position: Option<usize>,
}

pub(super) struct Copies<'a> {
    pub(super) values: BTreeMap<EntityId, &'a Value>,
    pub(super) definitions: BTreeMap<EntityId, Definition>,
    operations: BTreeMap<EntityId, Definition>,
    pub(super) blocks: BTreeMap<EntityId, usize>,
    pub(super) parameters: BTreeMap<EntityId, (usize, usize)>,
    pub(super) incoming: Vec<Vec<usize>>,
    pub(super) aliases: BTreeMap<EntityId, EntityId>,
    pub(super) materialize: BTreeSet<EntityId>,
    availability: Availability,
    complete: bool,
}

impl<'a> Copies<'a> {
    pub(super) fn build(ir: &'a Ir, pay: &mut impl FnMut(usize) -> bool) -> Option<Self> {
        // Reserve the linear indexes before collecting. Edge/operation payloads
        // are separately charged before their adjacency/definition allocations.
        if !pay(ir
            .values
            .len()
            .saturating_add(ir.blocks.len())
            .saturating_add(ir.edges.len())
            .saturating_mul(12)
            .saturating_add(1))
        {
            return None;
        }
        let values = ir.values.iter().map(|v| (v.id, v)).collect();
        let blocks: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let mut definitions = BTreeMap::new();
        let mut operations = BTreeMap::new();
        let mut parameters = BTreeMap::new();
        for (block, b) in ir.blocks.iter().enumerate() {
            if !pay(b.parameters.len().saturating_mul(4).saturating_add(1)) {
                return None;
            }
            for (slot, value) in b.parameters.iter().copied().enumerate() {
                parameters.insert(value, (block, slot));
                definitions.insert(
                    value,
                    Definition {
                        block,
                        position: None,
                    },
                );
            }
            for (position, op) in b.operations.iter().enumerate() {
                if !pay(op.outputs.len().saturating_mul(2).saturating_add(2)) {
                    return None;
                }
                operations.insert(
                    op.id,
                    Definition {
                        block,
                        position: Some(position),
                    },
                );
                for output in &op.outputs {
                    definitions.insert(
                        *output,
                        Definition {
                            block,
                            position: Some(position),
                        },
                    );
                }
            }
        }
        let mut incoming = vec![Vec::new(); ir.blocks.len()];
        let mut outgoing = vec![Vec::new(); ir.blocks.len()];
        for (i, edge) in ir.edges.iter().enumerate() {
            if !pay(2) {
                return None;
            }
            incoming[*blocks.get(&edge.to)?].push(i);
            outgoing[*blocks.get(&edge.from)?].push(i);
        }
        let complete = ir.fact_scope == FactScope::CompleteLocalCfg
            && ir.blocks.iter().all(|b| !b.terminator.is_partial());
        let availability = Availability::build(ir, &blocks, &incoming, &outgoing, complete, pay)?;
        let mut copies = Self {
            values,
            definitions,
            operations,
            blocks,
            parameters,
            incoming,
            availability,
            complete,
            aliases: BTreeMap::new(),
            materialize: BTreeSet::new(),
        };
        if complete {
            copies.discover(ir, pay)?;
        }
        if !copies.verify(ir, pay)? {
            return None;
        }
        for root in copies.aliases.values() {
            if !pay(1) {
                return None;
            }
            if matches!(copies.values.get(root)?.kind, ValueKind::Operation { .. }) {
                copies.materialize.insert(*root);
            }
        }
        Some(copies)
    }

    pub(super) fn canonical(&self, value: EntityId) -> EntityId {
        self.aliases.get(&value).copied().unwrap_or(value)
    }

    pub(super) fn available_at_entry(&self, root: EntityId, block: usize) -> bool {
        self.definitions.get(&root).is_some_and(|definition| {
            self.availability
                .strictly_dominates(definition.block, block)
        })
    }

    fn eligible(&self, ir: &Ir, block: usize) -> bool {
        self.complete
            && Some(ir.blocks[block].id) != ir.entry
            && self.availability.reachable(block)
            && !self.incoming[block].is_empty()
    }

    /// Check the resulting equations independently of SCC selection: every RHS
    /// matches the same available root, and every alias reaches a real external
    /// anchor. The latter is a single reverse traversal, not one BFS per phi.
    pub(super) fn verify(&self, ir: &Ir, pay: &mut impl FnMut(usize) -> bool) -> Option<bool> {
        if !pay(self.aliases.len().saturating_mul(5).saturating_add(1)) {
            return None;
        }
        let mut dependents = BTreeMap::<EntityId, Vec<EntityId>>::new();
        let mut anchored = BTreeSet::new();
        let mut todo = VecDeque::new();
        for (parameter, root) in &self.aliases {
            let (block, slot) = *self.parameters.get(parameter)?;
            if !self.eligible(ir, block)
                || self.aliases.contains_key(root)
                || !self.available_at_entry(*root, block)
            {
                return Some(false);
            }
            if !pay(self.incoming[block].len().saturating_mul(2)) {
                return None;
            }
            for edge in &self.incoming[block] {
                let argument = *ir.edges[*edge].arguments.get(slot)?;
                if self.canonical(argument) != *root {
                    return Some(false);
                }
                if argument == *root {
                    if anchored.insert(*parameter) {
                        todo.push_back(*parameter);
                    }
                } else {
                    dependents.entry(argument).or_default().push(*parameter);
                }
            }
        }
        while let Some(parameter) = todo.pop_front() {
            let next = dependents
                .get(&parameter)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if !pay(next.len().saturating_add(1)) {
                return None;
            }
            for dependent in next {
                if anchored.insert(*dependent) {
                    todo.push_back(*dependent);
                }
            }
        }
        Some(anchored.len() == self.aliases.len())
    }
}
