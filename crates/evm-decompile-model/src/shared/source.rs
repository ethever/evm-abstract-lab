//! Finite role-by-role correspondence. Candidate hashes never enter admission.
use super::{
    MAX_SHARED_REGION_ENTITIES, MAX_SHARED_REGION_NODES, SharedBinding, SharedInterface,
    SharedInvocation, SharedProcedureError as Error, SharedProcedureKind,
};
use crate::{
    Block, Edge, EntityId, Ir, LiftedProgram, StructuredKind as K, StructuredNode, Terminator as T,
    Value, ValueKind as V,
};
use std::collections::{BTreeMap, BTreeSet};
mod admit;
mod arithmetic;
mod correspondence;
mod flow;
mod region;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct Site {
    owner: Option<EntityId>,
    position: usize,
    continuation: Option<EntityId>,
    end: usize,
}
/// Indexed source owned by a single bounded verification/discovery pass.
pub struct SharedSourceIndex<'a> {
    pub(crate) ir: &'a Ir,
    pub(crate) nodes: BTreeMap<EntityId, &'a StructuredNode>,
    pub(crate) blocks: BTreeMap<EntityId, &'a Block>,
    pub(crate) edges: BTreeMap<EntityId, &'a Edge>,
    pub(crate) values: BTreeMap<EntityId, &'a Value>,
    sites: BTreeMap<EntityId, Site>,
    order: BTreeMap<EntityId, usize>,
    flow: Option<flow::Flow>,
    expression_reads: BTreeMap<EntityId, BTreeSet<EntityId>>,
    expressions: BTreeMap<EntityId, &'a crate::LiftedExpression>,
    expression_positions: BTreeMap<EntityId, usize>,
    operations: BTreeMap<EntityId, &'a crate::Operation>,
    high_blocks: BTreeMap<EntityId, &'a crate::LiftedBlock>,
    high_transfers: BTreeMap<EntityId, &'a crate::LiftedTransfer>,
    controls: BTreeMap<EntityId, &'a crate::LiftedControl>,
    constructions: BTreeMap<EntityId, &'a crate::LiftedConstruction>,
    reverts: BTreeMap<EntityId, &'a crate::LiftedRevert>,
    payloads: BTreeMap<EntityId, &'a crate::LiftedErrorPayload>,
}
/// Bounded source summary. It stores IDs, never another copy of executable code.
pub struct SharedRegion {
    /// Independently checked execution contract, including pure loop bodies.
    pub kind: SharedProcedureKind,

    /// Exact Structured root or arithmetic expression root.
    pub root: EntityId,

    /// Source AST occurrences in execution traversal order.
    pub nodes: Vec<EntityId>,

    /// Source blocks whose logical primitive bodies execute in this fragment.
    pub blocks: Vec<EntityId>,

    /// Sorted finite source and semantic correspondence domain.
    pub entities: Vec<EntityId>,

    /// Checked logical word and state boundary.
    pub interface: SharedInterface,

    /// Raw witness operation count for executed block occurrences.
    pub operations: usize,

    /// Retained logical primitive count before guard/capsule formatting.
    pub retained_operations: usize,
    /// Every terminal body and its source Exit are enclosed together.
    pub terminal_closed: bool,
}
fn charge(pay: &mut impl FnMut(usize) -> bool, n: usize) -> Result<(), Error> {
    if pay(n) { Ok(()) } else { Err(Error::Limit) }
}
fn value_sort(value: &Value) -> u8 {
    match value.kind {
        V::Memory { .. } | V::MemoryParameter { .. } => 1,
        V::Effect { .. } | V::EffectParameter { .. } => 2,
        _ => 0,
    }
}
impl<'a> SharedSourceIndex<'a> {
    /// Charge complete index/adjacency construction before allocation.
    pub fn new(ir: &'a Ir, pay: &mut impl FnMut(usize) -> bool) -> Result<Self, Error> {
        Self::for_program(ir, ir.lifted.as_ref(), pay)
    }
    /// Use an unpublished projection while discovering source-certified plans.
    pub fn for_program(
        ir: &'a Ir,
        program: Option<&'a LiftedProgram>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, Error> {
        Self::for_program_with_event_capacity(ir, program, pay, crate::MAX_LIFTED_RECORDS * 8)
    }
    /// Inspect an optional source candidate with a smaller retained-event budget.
    /// The caller cannot raise the model's hard ceiling. All admitted work still
    /// uses the supplied cumulative payer; refusal carries the exact capacity.
    pub fn for_program_with_event_capacity(
        ir: &'a Ir,
        program: Option<&'a LiftedProgram>,
        pay: &mut impl FnMut(usize) -> bool,
        max_event_slots: usize,
    ) -> Result<Self, Error> {
        let max_event_slots = max_event_slots.min(crate::MAX_LIFTED_RECORDS * 8);
        let mut size = ir.values.len().saturating_add(ir.edges.len());
        for b in &ir.blocks {
            size = size.saturating_add(b.parameters.len()).saturating_add(1);
            for o in &b.operations {
                size = size
                    .saturating_add(o.inputs.len())
                    .saturating_add(o.outputs.len())
                    .saturating_add(8);
            }
        }
        for e in &ir.edges {
            size = size.saturating_add(e.arguments.len()).saturating_add(4);
        }
        charge(
            pay,
            size.saturating_mul(4)
                .saturating_add(program.map_or(0, LiftedProgram::record_count)),
        )?;
        if let (Some(supplied), Some(source)) = (program, ir.lifted.as_ref())
            && !std::ptr::eq(supplied, source)
        {
            charge(
                pay,
                supplied
                    .record_count()
                    .saturating_add(source.record_count())
                    .saturating_mul(4),
            )?;
            if supplied.expressions != source.expressions
                || supplied.blocks != source.blocks
                || supplied.transfers != source.transfers
                || supplied.controls != source.controls
                || supplied.failures != source.failures
                || supplied.payloads != source.payloads
                || supplied.reverts != source.reverts
                || supplied.constructions != source.constructions
            {
                return Err(Error::Correspondence(EntityId(0)));
            }
        }
        if let Some(program) = program {
            program.validate().map_err(Error::BaseSource)?;
        }
        let mut index = Self {
            ir,
            nodes: BTreeMap::new(),
            blocks: ir.blocks.iter().map(|b| (b.id, b)).collect(),
            edges: ir.edges.iter().map(|e| (e.id, e)).collect(),
            values: ir.values.iter().map(|v| (v.id, v)).collect(),
            sites: BTreeMap::new(),
            order: BTreeMap::new(),
            flow: None,
            expression_reads: BTreeMap::new(),
            expressions: program
                .into_iter()
                .flat_map(|p| &p.expressions)
                .map(|e| (e.id, e))
                .collect(),
            expression_positions: program
                .into_iter()
                .flat_map(|p| p.expressions.iter().enumerate())
                .map(|(position, e)| (e.id, position))
                .collect(),
            operations: ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .map(|o| (o.id, o))
                .collect(),
            high_blocks: program
                .into_iter()
                .flat_map(|p| &p.blocks)
                .map(|b| (b.block, b))
                .collect(),
            high_transfers: program
                .into_iter()
                .flat_map(|p| &p.transfers)
                .map(|t| (t.edge, t))
                .collect(),
            controls: program
                .into_iter()
                .flat_map(|p| &p.controls)
                .map(|c| (c.node, c))
                .collect(),
            constructions: program
                .into_iter()
                .flat_map(|p| &p.constructions)
                .map(|c| (c.block, c))
                .collect(),
            reverts: program
                .into_iter()
                .flat_map(|p| &p.reverts)
                .map(|r| (r.id, r))
                .collect(),
            payloads: program
                .into_iter()
                .flat_map(|p| &p.payloads)
                .map(|p| (p.id, p))
                .collect(),
        };
        if let Some(ast) = &ir.structured {
            enum Visit<'b> {
                Enter(
                    &'b StructuredNode,
                    Option<EntityId>,
                    usize,
                    Option<EntityId>,
                ),
                Leave(EntityId),
            }
            let mut todo = vec![Visit::Enter(&ast.root, None, 0, None)];
            while let Some(task) = todo.pop() {
                charge(pay, 4)?;
                let (node, owner, position, continuation) = match task {
                    Visit::Enter(n, o, p, c) => (n, o, p, c),
                    Visit::Leave(id) => {
                        index.sites.get_mut(&id).ok_or(Error::Missing(id))?.end = index.order.len();
                        continue;
                    }
                };
                if index.nodes.len() >= crate::MAX_STRUCTURED_WIRE_NODES
                    || index.nodes.insert(node.id, node).is_some()
                {
                    return Err(Error::Site(node.id));
                }
                index.order.insert(node.id, index.order.len());
                index.sites.insert(
                    node.id,
                    Site {
                        owner,
                        position,
                        continuation,
                        end: 0,
                    },
                );
                todo.push(Visit::Leave(node.id));
                match &node.kind {
                    K::Sequence(children) => {
                        charge(pay, children.len())?;
                        for (i, child) in children.iter().enumerate().rev() {
                            todo.push(Visit::Enter(
                                child,
                                Some(node.id),
                                i,
                                children.get(i + 1).map(|n| n.id).or(continuation),
                            ));
                        }
                    }
                    K::If {
                        then_branch,
                        else_branch,
                        ..
                    } => {
                        todo.push(Visit::Enter(else_branch, Some(node.id), 1, continuation));
                        todo.push(Visit::Enter(then_branch, Some(node.id), 0, continuation));
                    }
                    K::While { header, body, .. } => {
                        todo.push(Visit::Enter(body, Some(node.id), 1, Some(header.id)));
                        todo.push(Visit::Enter(header, Some(node.id), 0, Some(node.id)));
                    }
                    K::Indirect { arms, .. } => {
                        charge(pay, arms.len())?;
                        for (i, arm) in arms.iter().enumerate().rev() {
                            todo.push(Visit::Enter(&arm.body, Some(node.id), i, continuation));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(program) = program {
            let mut slots = 0_usize;
            for e in &program.expressions {
                let operands = e.kind.operands();
                let size = operands.iter().fold(1_usize, |n, id| {
                    n.saturating_add(index.expression_reads.get(id).map_or(0, BTreeSet::len))
                });
                charge(pay, size.saturating_mul(3))?;
                let required = slots.saturating_add(size);
                let limit = crate::MAX_LIFTED_RECORDS * 8;
                if required > limit {
                    return Err(Error::Capacity {
                        resource: crate::SharedCapacity::ExpressionReadSlots,
                        required,
                        limit,
                    });
                }
                let mut reads = BTreeSet::new();
                if let crate::LiftedExpressionKind::Value(id) = e.kind {
                    reads.insert(id);
                }
                for id in operands {
                    reads.extend(
                        index
                            .expression_reads
                            .get(&id)
                            .ok_or(Error::Missing(id))?
                            .iter()
                            .copied(),
                    );
                }
                slots = slots.saturating_add(reads.len());
                index.expression_reads.insert(e.id, reads);
            }
        }
        index.check_source_projection(pay)?;
        if let Some(program) = program {
            charge(
                pay,
                size.saturating_mul(16)
                    .saturating_add(program.record_count().saturating_mul(8)),
            )?;
            let base = crate::lifted::sources::GuardSources::new(
                program,
                ir,
                &index.expressions,
                &index.high_blocks,
                &index.edges,
            );
            if let Some(error) = base.error {
                return Err(Error::BaseSource(error));
            }
        }

        if ir.structured.is_some() {
            let flow = if max_event_slots == crate::MAX_LIFTED_RECORDS * 8 {
                flow::Flow::new(&index, pay)
            } else {
                flow::Flow::new_with_slots(&index, pay, max_event_slots)
            };
            index.flow = match flow {
                Ok(flow) => Some(flow),
                // An unsupported control shape cannot invalidate an unrelated
                // expression-only template. It simply has no AST-region proof.
                Err(Error::Region(_)) => None,
                Err(error) => return Err(error),
            };
        }
        Ok(index)
    }
    fn check_source_projection(&self, pay: &mut impl FnMut(usize) -> bool) -> Result<(), Error> {
        if self.blocks.len() != self.ir.blocks.len()
            || self.edges.len() != self.ir.edges.len()
            || self.values.len() != self.ir.values.len()
            || self.operations.len()
                != self
                    .ir
                    .blocks
                    .iter()
                    .map(|b| b.operations.len())
                    .sum::<usize>()
            || self.high_blocks.len() != self.ir.blocks.len()
            || self.high_transfers.len() != self.ir.edges.len()
        {
            return Err(Error::Correspondence(EntityId(0)));
        }
        for expression in self.expressions.values() {
            charge(pay, 2)?;
            self.blocks
                .get(&expression.block)
                .ok_or(Error::Missing(expression.block))?;
            if let crate::LiftedExpressionKind::Value(id) = expression.kind {
                let value = self.values.get(&id).ok_or(Error::Missing(id))?;
                if value_sort(value) != 0 {
                    return Err(Error::Correspondence(expression.id));
                }
            }
        }
        for edge in &self.ir.edges {
            charge(pay, edge.arguments.len().saturating_add(8))?;
            let target = self.blocks.get(&edge.to).ok_or(Error::Missing(edge.to))?;
            self.blocks
                .get(&edge.from)
                .ok_or(Error::Missing(edge.from))?;
            let high = self
                .high_transfers
                .get(&edge.id)
                .ok_or(Error::Missing(edge.id))?;
            let logical = self
                .high_blocks
                .get(&edge.to)
                .ok_or(Error::Missing(edge.to))?;
            if high.arguments.iter().any(|id| {
                self.expressions
                    .get(id)
                    .is_none_or(|e| e.block != edge.from)
            }) {
                return Err(Error::Correspondence(edge.id));
            }
            if high.arguments.len() != logical.parameters.len()
                || edge.arguments.len() != target.parameters.len()
            {
                return Err(Error::Correspondence(edge.id));
            }
            for id in edge
                .arguments
                .iter()
                .copied()
                .chain([edge.memory, edge.effect])
            {
                self.values.get(&id).ok_or(Error::Missing(id))?;
            }
        }
        for raw in &self.ir.blocks {
            let high = self
                .high_blocks
                .get(&raw.id)
                .ok_or(Error::Missing(raw.id))?;
            let mut current = 0;
            for (position, op) in raw.operations.iter().enumerate() {
                if op.replacement.is_some() && (!op.effects.is_pure() || op.outputs.len() != 1) {
                    return Err(Error::Correspondence(op.id));
                }
                charge(
                    pay,
                    op.inputs
                        .len()
                        .saturating_add(op.outputs.len())
                        .saturating_add(8),
                )?;
                let retained = high
                    .statements
                    .get(current)
                    .filter(|s| s.operation == op.id);
                if let Some(statement) = retained {
                    if statement.inputs.len() != op.inputs.len() {
                        return Err(Error::Correspondence(statement.id));
                    }
                    let mut outputs = op.outputs.iter();
                    for value in &statement.outputs {
                        if !outputs.by_ref().any(|v| v == value) {
                            return Err(Error::Correspondence(statement.id));
                        }
                    }
                    current += 1;
                } else if !op.effects.is_pure()
                    || (position + 1 == raw.operations.len()
                        && matches!(raw.terminator, T::Trap { .. }))
                {
                    return Err(Error::Correspondence(op.id));
                }
            }
            if current != high.statements.len() {
                return Err(Error::Correspondence(raw.id));
            }
        }
        Ok(())
    }
    /// Every source occurrence, in deterministic AST order.
    pub fn node_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.nodes.keys().copied()
    }
    /// Every arithmetic candidate anchor, without traversing or expanding a DAG.
    pub fn expression_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.expressions.keys().copied()
    }
    /// Resolve the body form from the disjoint source arenas, then check the
    /// advertised contract. A producer cannot relabel a stateful body as pure.
    pub fn source_region(
        &self,
        kind: SharedProcedureKind,
        root: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<SharedRegion, Error> {
        if kind == SharedProcedureKind::Legacy {
            return Err(Error::Region(root));
        }
        let region = match (
            self.nodes.contains_key(&root),
            self.expressions.contains_key(&root),
        ) {
            (true, false) => self.region(root, pay)?,
            (false, true) => self.arithmetic_region(root, pay)?,
            _ => return Err(Error::Region(root)),
        };
        if region.kind != kind {
            return Err(Error::Interface(root));
        }
        Ok(region)
    }
    /// Record actual ownership/continuation without letting candidates invent it.
    pub fn invocation(
        &self,
        id: EntityId,
        procedure: EntityId,
        source: EntityId,
        bindings: Vec<SharedBinding>,
    ) -> Result<SharedInvocation, Error> {
        let s = self.sites.get(&source).ok_or(Error::Missing(source))?;
        Ok(SharedInvocation {
            id,
            procedure,
            source,
            owner: s.owner,
            position: s.position,
            continuation: s.continuation,
            bindings,
        })
    }
    /// Ancestor intervals avoid recursive overlap scans or textual expansion.
    pub fn overlaps(&self, a: EntityId, b: EntityId) -> bool {
        let Some((&a0, sa)) = self.order.get(&a).zip(self.sites.get(&a)) else {
            return true;
        };
        let Some((&b0, sb)) = self.order.get(&b).zip(self.sites.get(&b)) else {
            return true;
        };
        a0 < sb.end && b0 < sa.end
    }
}
