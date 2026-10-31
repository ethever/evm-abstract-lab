//! Positions in one immutable IR snapshot. No borrowed tree or cloned subtree
//! is retained: syntax locations use parent/child coordinates in linear space.

use evm_abstract_protocol::decompile_model::{
    Block, DispatchTransition, Edge, EntityId, Ir, Operation, StructuredKind, StructuredNode, Value,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct StageIndex {
    pub blocks: BTreeMap<EntityId, usize>,
    operations: BTreeMap<EntityId, (usize, usize)>,
    values: BTreeMap<EntityId, usize>,
    edges: BTreeMap<EntityId, usize>,
    nodes: BTreeMap<EntityId, Option<(EntityId, usize)>>,
    transitions: BTreeMap<EntityId, (EntityId, usize, usize)>,
    #[cfg(test)]
    pub value_reads: std::cell::Cell<usize>,
}

impl StageIndex {
    // Entry policy is model-owned and shared with execution/export. Supply
    // this immutable stage's edge index so transfer anchors stay source-based.
    pub fn first_block(&self, ir: &Ir, node: &StructuredNode) -> Option<EntityId> {
        node.entry_block_with(|edge| self.edge(ir, edge).map(|edge| edge.from))
    }
    pub fn new(ir: &Ir) -> Self {
        let mut index = Self::default();
        for (position, block) in ir.blocks.iter().enumerate() {
            index.blocks.insert(block.id, position);
            for (offset, op) in block.operations.iter().enumerate() {
                index.operations.insert(op.id, (position, offset));
            }
        }
        index.values = ir
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
        index.edges = ir
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
        if let Some(program) = &ir.structured {
            let mut pending = vec![(&program.root, None)];
            while let Some((node, parent)) = pending.pop() {
                index.nodes.insert(node.id, parent);
                if let StructuredKind::Dispatcher { cases, .. } = &node.kind {
                    for (case, body) in cases.iter().enumerate() {
                        for (offset, transition) in body.transitions.iter().enumerate() {
                            index
                                .transitions
                                .insert(transition.id, (node.id, case, offset));
                        }
                    }
                }
                for (slot, child) in children(node).into_iter().enumerate().rev() {
                    pending.push((child, Some((node.id, slot))));
                }
            }
        }
        index
    }
    pub fn block<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a Block> {
        ir.blocks.get(*self.blocks.get(&id)?)
    }
    pub fn containing_block(&self, id: EntityId) -> Option<usize> {
        self.blocks
            .get(&id)
            .copied()
            .or_else(|| self.operations.get(&id).map(|(block, _)| *block))
    }
    pub fn operation<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a Operation> {
        let (block, offset) = *self.operations.get(&id)?;
        ir.blocks.get(block)?.operations.get(offset)
    }
    pub fn value<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a Value> {
        #[cfg(test)]
        self.value_reads.set(self.value_reads.get() + 1);
        ir.values.get(*self.values.get(&id)?)
    }
    pub fn edge<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a Edge> {
        ir.edges.get(*self.edges.get(&id)?)
    }
    pub fn node<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a StructuredNode> {
        let mut slots = Vec::new();
        let mut cursor = id;
        while let Some((parent, slot)) = *self.nodes.get(&cursor)? {
            slots.push(slot);
            cursor = parent;
        }
        let mut node = &ir.structured.as_ref()?.root;
        for slot in slots.into_iter().rev() {
            node = child(node, slot)?;
        }
        (node.id == id).then_some(node)
    }
    pub fn transition<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a DispatchTransition> {
        let (node, case, offset) = *self.transitions.get(&id)?;
        let StructuredKind::Dispatcher { cases, .. } = &self.node(ir, node)?.kind else {
            return None;
        };
        cases.get(case)?.transitions.get(offset)
    }
}

fn child(node: &StructuredNode, slot: usize) -> Option<&StructuredNode> {
    match &node.kind {
        StructuredKind::Sequence(nodes) => nodes.get(slot),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => match slot {
            0 => Some(then_branch),
            1 => Some(else_branch),
            _ => None,
        },
        StructuredKind::While { header, body, .. } => match slot {
            0 => Some(header),
            1 => Some(body),
            _ => None,
        },
        StructuredKind::Indirect { arms, .. } => arms.get(slot).map(|arm| &arm.body),
        _ => None,
    }
}

pub(super) fn children(node: &StructuredNode) -> Vec<&StructuredNode> {
    match &node.kind {
        StructuredKind::Sequence(nodes) => nodes.iter().collect(),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => vec![then_branch, else_branch],
        StructuredKind::While { header, body, .. } => vec![header, body],
        StructuredKind::Indirect { arms, .. } => arms.iter().map(|arm| &arm.body).collect(),
        _ => vec![],
    }
}
