//! Positions in one immutable IR snapshot. No borrowed tree or cloned subtree
//! is retained: syntax locations use parent/child coordinates in linear space.

use evm_abstract_protocol::decompile_model::{
    Block, DispatchTransition, Edge, EntityId, Ir, LiftedBlock, LiftedConstruction, LiftedControl,
    LiftedErrorPayload, LiftedExpression, LiftedRevert, LiftedStatement, LiftedTransfer, Operation,
    SharedInvocation, SharedProcedure, StructuredKind, StructuredNode, Value,
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
    expressions: BTreeMap<EntityId, usize>,
    lifted_blocks: BTreeMap<EntityId, usize>,
    statements: BTreeMap<EntityId, (usize, usize)>,
    lifted_transfers: BTreeMap<EntityId, usize>,
    controls: BTreeMap<EntityId, usize>,
    payloads: BTreeMap<EntityId, usize>,
    reverts: BTreeMap<EntityId, usize>,
    constructions: BTreeMap<EntityId, usize>,
    procedures: BTreeMap<EntityId, usize>,
    invocations: BTreeMap<EntityId, usize>,
    source_blocks: BTreeMap<EntityId, usize>,
    source_edges: BTreeMap<EntityId, usize>,
    source_nodes: BTreeMap<EntityId, usize>,
    semantic_owners: BTreeMap<EntityId, usize>,
    render_work_limit: u64,
    pub shared: super::lifted::shared::Prepared,
    #[cfg(test)]
    pub value_reads: std::cell::Cell<usize>,
}

impl StageIndex {
    // Entry policy is model-owned and shared with execution/export. Supply
    // this immutable stage's edge index so transfer anchors stay source-based.
    pub fn first_block(&self, ir: &Ir, node: &StructuredNode) -> Option<EntityId> {
        node.entry_block_with(|edge| self.edge(ir, edge).map(|edge| edge.from))
    }
    #[cfg(test)]
    pub fn new(ir: &Ir) -> Self {
        Self::with_render_work_limit(ir, u64::MAX)
    }
    pub fn with_render_work_limit(ir: &Ir, render_work_limit: u64) -> Self {
        let mut index = Self {
            render_work_limit,
            ..Self::default()
        };
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
        if let Some(program) = &ir.lifted {
            for (position, expression) in program.expressions.iter().enumerate() {
                index.expressions.insert(expression.id, position);
                if let Some(block) = index.blocks.get(&expression.block) {
                    index.semantic_owners.insert(expression.id, *block);
                }
            }
            for (position, block) in program.blocks.iter().enumerate() {
                index.lifted_blocks.insert(block.id, position);
                index.source_blocks.insert(block.block, position);
                if let Some(owner) = index.blocks.get(&block.block) {
                    index.semantic_owners.insert(block.id, *owner);
                    for statement in &block.statements {
                        index.semantic_owners.insert(statement.id, *owner);
                    }
                }
                for (offset, statement) in block.statements.iter().enumerate() {
                    index.statements.insert(statement.id, (position, offset));
                }
            }
            for (position, transfer) in program.transfers.iter().enumerate() {
                index.lifted_transfers.insert(transfer.id, position);
                index.source_edges.insert(transfer.edge, position);
                if let Some(edge) = index.edge(ir, transfer.edge)
                    && let Some(block) = index.blocks.get(&edge.from)
                {
                    index.semantic_owners.insert(transfer.id, *block);
                }
            }
            for (position, control) in program.controls.iter().enumerate() {
                index.controls.insert(control.id, position);
                index.source_nodes.insert(control.node, position);
                if let Some(block) = index.semantic_owners.get(&control.condition).copied() {
                    index.semantic_owners.insert(control.id, block);
                }
            }
            index.payloads = program
                .payloads
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id, i))
                .collect();
            for (position, revert) in program.reverts.iter().enumerate() {
                index.reverts.insert(revert.id, position);
                if let Some(owner) = index.blocks.get(&revert.block) {
                    index.semantic_owners.insert(revert.id, *owner);
                }
            }
            for (position, construction) in program.constructions.iter().enumerate() {
                index.constructions.insert(construction.id, position);
                if let Some(owner) = index.blocks.get(&construction.block) {
                    index.semantic_owners.insert(construction.id, *owner);
                }
            }
            for (position, procedure) in program.procedures.iter().enumerate() {
                index.procedures.insert(procedure.id, position);
                if let Some(owner) = index
                    .node(ir, procedure.body)
                    .and_then(|node| index.first_block(ir, node))
                    .or_else(|| {
                        index
                            .expression(ir, procedure.body)
                            .map(|expression| expression.block)
                    })
                    .and_then(|block| index.blocks.get(&block).copied())
                {
                    index.semantic_owners.insert(procedure.id, owner);
                }
            }
            for (position, invocation) in program.invocations.iter().enumerate() {
                index.invocations.insert(invocation.id, position);
                if let Some(owner) = index
                    .node(ir, invocation.source)
                    .and_then(|node| index.first_block(ir, node))
                    .or_else(|| {
                        index
                            .expression(ir, invocation.source)
                            .map(|expression| expression.block)
                    })
                    .and_then(|block| index.blocks.get(&block).copied())
                {
                    index.semantic_owners.insert(invocation.id, owner);
                }
            }
        }
        index.shared = super::lifted::shared::Prepared::new(ir, &index.render_work());
        index
    }
    pub fn render_work(&self) -> super::lifted::work::Work {
        super::lifted::work::Work::new(self.render_work_limit)
    }
    pub fn block<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a Block> {
        ir.blocks.get(*self.blocks.get(&id)?)
    }
    pub fn containing_block(&self, id: EntityId) -> Option<usize> {
        self.blocks
            .get(&id)
            .copied()
            .or_else(|| self.operations.get(&id).map(|(block, _)| *block))
            .or_else(|| self.semantic_owners.get(&id).copied())
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
    /// A direct If arm carries the already selected condition polarity.
    pub fn branch_arm(&self, ir: &Ir, id: EntityId) -> Option<bool> {
        let (parent, slot) = self.nodes.get(&id).copied().flatten()?;
        matches!(&self.node(ir, parent)?.kind, StructuredKind::If { .. }).then_some(slot == 0)
    }
    pub fn transition<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a DispatchTransition> {
        let (node, case, offset) = *self.transitions.get(&id)?;
        let StructuredKind::Dispatcher { cases, .. } = &self.node(ir, node)?.kind else {
            return None;
        };
        cases.get(case)?.transitions.get(offset)
    }

    pub fn expression<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedExpression> {
        ir.lifted
            .as_ref()?
            .expressions
            .get(*self.expressions.get(&id)?)
    }
    pub fn lifted_block<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedBlock> {
        ir.lifted
            .as_ref()?
            .blocks
            .get(*self.lifted_blocks.get(&id)?)
    }
    pub fn statement<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedStatement> {
        let (block, offset) = *self.statements.get(&id)?;
        ir.lifted
            .as_ref()?
            .blocks
            .get(block)?
            .statements
            .get(offset)
    }
    pub fn lifted_transfer<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedTransfer> {
        ir.lifted
            .as_ref()?
            .transfers
            .get(*self.lifted_transfers.get(&id)?)
    }
    pub fn control<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedControl> {
        ir.lifted.as_ref()?.controls.get(*self.controls.get(&id)?)
    }
    pub fn payload<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedErrorPayload> {
        ir.lifted.as_ref()?.payloads.get(*self.payloads.get(&id)?)
    }
    pub fn revert<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedRevert> {
        ir.lifted.as_ref()?.reverts.get(*self.reverts.get(&id)?)
    }
    pub fn construction<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedConstruction> {
        ir.lifted
            .as_ref()?
            .constructions
            .get(*self.constructions.get(&id)?)
    }
    pub fn procedure<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a SharedProcedure> {
        ir.lifted
            .as_ref()?
            .procedures
            .get(*self.procedures.get(&id)?)
    }
    pub fn invocation<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a SharedInvocation> {
        ir.lifted
            .as_ref()?
            .invocations
            .get(*self.invocations.get(&id)?)
    }
    pub fn source_block<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedBlock> {
        ir.lifted
            .as_ref()?
            .blocks
            .get(*self.source_blocks.get(&id)?)
    }
    pub fn source_transfer<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedTransfer> {
        ir.lifted
            .as_ref()?
            .transfers
            .get(*self.source_edges.get(&id)?)
    }
    pub fn source_control<'a>(&self, ir: &'a Ir, id: EntityId) -> Option<&'a LiftedControl> {
        ir.lifted
            .as_ref()?
            .controls
            .get(*self.source_nodes.get(&id)?)
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
