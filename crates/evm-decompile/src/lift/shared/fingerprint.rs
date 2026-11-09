//! Cheap normalized grouping. None of these hashes participate in admission.
use super::DiscoveryError;
use crate::{
    EntityId, Ir, LiftedExpressionKind as X, LiftedProgram, SharedRegion, StructuredKind as K,
};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy)]
pub(super) struct ExpressionShape {
    pub hash: u64,
    /// Expanded arithmetic size, capped before shared DAGs multiply it.
    pub operations: usize,
    pub bytes: usize,
}

pub(super) struct Fingerprints<'a> {
    expressions: BTreeMap<EntityId, ExpressionShape>,
    blocks: BTreeMap<EntityId, &'a crate::LiftedBlock>,
    transfers: BTreeMap<EntityId, &'a crate::LiftedTransfer>,
    controls: BTreeMap<EntityId, &'a crate::LiftedControl>,
    operations: BTreeMap<EntityId, &'a crate::Operation>,
}

impl<'a> Fingerprints<'a> {
    pub fn new(
        ir: &'a Ir,
        program: &'a LiftedProgram,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, DiscoveryError> {
        let mut expressions = BTreeMap::<EntityId, ExpressionShape>::new();
        for expression in &program.expressions {
            let operands = expression.kind.operands();
            if !pay(operands.len().saturating_add(4)) {
                return Err(DiscoveryError::Payment);
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (expression.ty as u8).hash(&mut h);
            // Leaves are template parameters. Literal encodings, source PCs,
            // SSA numbers and aliases cannot split otherwise equal templates.
            let tag = match expression.kind {
                X::Word(_) | X::Value(_) | X::Predicate(_) => 0_u8,
                X::WordFromPredicate(_) => 1,
                X::NonZero(_) => 2,
                X::Not(_) => 3,
                X::Compare { relation, .. } => {
                    (relation as u8).hash(&mut h);
                    4
                }
                X::WordOp { opcode, .. } => {
                    opcode.hash(&mut h);
                    5
                }
            };
            tag.hash(&mut h);
            operands.len().hash(&mut h);
            let mut operations = usize::from(!operands.is_empty());
            let mut bytes = if operands.is_empty() { 9_usize } else { 12 };
            for operand in operands {
                let shape = expressions.get(&operand).ok_or(DiscoveryError::Invalid(
                    crate::SharedProcedureError::Missing(operand),
                ))?;
                shape.hash.hash(&mut h);
                operations = operations.saturating_add(shape.operations).min(65_536);
                bytes = bytes.saturating_add(shape.bytes).min(65_536);
            }
            expressions.insert(
                expression.id,
                ExpressionShape {
                    hash: h.finish(),
                    operations,
                    bytes,
                },
            );
        }
        Ok(Self {
            expressions,
            blocks: program.blocks.iter().map(|b| (b.block, b)).collect(),
            transfers: program.transfers.iter().map(|t| (t.edge, t)).collect(),
            controls: program.controls.iter().map(|c| (c.node, c)).collect(),
            operations: ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .map(|o| (o.id, o))
                .collect(),
        })
    }

    pub fn expression(&self, id: EntityId) -> Option<ExpressionShape> {
        self.expressions.get(&id).copied()
    }

    pub fn region(
        &self,
        region: &SharedRegion,
        nodes: &BTreeMap<EntityId, &crate::StructuredNode>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<u64, DiscoveryError> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for id in &region.nodes {
            if !pay(1) {
                return Err(DiscoveryError::Payment);
            }
            let tag = match &nodes[id].kind {
                K::Sequence(children) => {
                    children.len().hash(&mut h);
                    0_u8
                }
                K::Block { block } => {
                    let high = self.blocks[block];
                    high.parameters.len().hash(&mut h);
                    high.statements.len().hash(&mut h);
                    for statement in &high.statements {
                        if !pay(statement.inputs.len().saturating_add(1)) {
                            return Err(DiscoveryError::Payment);
                        }
                        let opcode = self.operations[&statement.operation].opcode;
                        if matches!(opcode, 0x5f..=0x7f) {
                            0x60_u8.hash(&mut h);
                        } else {
                            opcode.hash(&mut h);
                        }
                        statement.outputs.len().hash(&mut h);
                        for input in &statement.inputs {
                            self.expressions[input].hash.hash(&mut h);
                        }
                    }
                    1
                }
                K::Transfer { edge } => {
                    for argument in &self.transfers[edge].arguments {
                        if !pay(1) {
                            return Err(DiscoveryError::Payment);
                        }
                        self.expressions[argument].hash.hash(&mut h);
                    }
                    2
                }
                K::If { .. } => {
                    self.expressions[&self.controls[id].condition]
                        .hash
                        .hash(&mut h);
                    3
                }
                K::While {
                    condition,
                    condition_nonzero,
                    ..
                } => {
                    condition.is_some().hash(&mut h);
                    condition_nonzero.hash(&mut h);
                    if condition.is_some() {
                        self.expressions[&self.controls[id].condition]
                            .hash
                            .hash(&mut h);
                    }
                    4
                }
                K::Break => 5,
                K::Continue => 6,
                K::Exit { .. } => 7,
                _ => 8,
            };
            tag.hash(&mut h);
        }
        Ok(h.finish())
    }
}
