//! Check raw binding/entry availability independently of normal-form rebuilding.
use super::Copies;
use crate::{Ir, LiftedExpressionKind as Kind, LiftedProgram};
use std::collections::{BTreeMap, BTreeSet};

impl Copies<'_> {
    pub(in crate::lift) fn check_program(
        &self,
        ir: &Ir,
        program: &LiftedProgram,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<bool> {
        if !pay(program
            .entity_ids()
            .count()
            .saturating_mul(6)
            .saturating_add(1))
        {
            return None;
        }
        let mut bound = BTreeSet::new();
        let mut seen_blocks = BTreeSet::new();
        let mut seen_statements = BTreeSet::new();
        let mut arities = vec![0; ir.blocks.len()];
        for block in &program.blocks {
            let Some(raw_block) = self.blocks.get(&block.block).copied() else {
                return Some(false);
            };
            if !seen_blocks.insert(raw_block) {
                return Some(false);
            }
            if !pay(block.parameters.len()) {
                return None;
            }
            arities[raw_block] = block.parameters.len();
            let mut previous_parameter = None;
            for parameter in &block.parameters {
                let Some((owner, slot)) = self.parameters.get(parameter) else {
                    return Some(false);
                };
                if self.aliases.contains_key(parameter)
                    || *owner != raw_block
                    || previous_parameter.is_some_and(|previous| previous >= *slot)
                    || !bound.insert(*parameter)
                {
                    return Some(false);
                }
                previous_parameter = Some(*slot);
            }
            let mut previous = None;
            for statement in &block.statements {
                let Some(definition) = self.operations.get(&statement.operation) else {
                    return Some(false);
                };
                let position = definition.position?;
                if definition.block != raw_block || previous.is_some_and(|p| p >= position) {
                    return Some(false);
                }
                previous = Some(position);
                seen_statements.insert(statement.operation);
                let operation = &ir.blocks[raw_block].operations[position];
                if !pay(operation
                    .outputs
                    .len()
                    .saturating_add(statement.outputs.len())
                    .saturating_add(1))
                {
                    return None;
                }
                if statement.inputs.len() != operation.inputs.len() {
                    return Some(false);
                }
                let mut outputs = operation.outputs.iter();
                for output in &statement.outputs {
                    if !outputs.by_ref().any(|raw| raw == output) || !bound.insert(*output) {
                        return Some(false);
                    }
                }
            }
            let raw = &ir.blocks[raw_block];
            for (position, operation) in raw.operations.iter().enumerate() {
                if !pay(1) {
                    return None;
                }
                let boundary = matches!(raw.terminator, crate::Terminator::Pending)
                    || (position + 1 == raw.operations.len()
                        && matches!(raw.terminator, crate::Terminator::Trap { .. }));
                if (!operation.effects.is_pure() || boundary)
                    && !seen_statements.contains(&operation.id)
                {
                    return Some(false);
                }
            }
        }
        if seen_blocks.len() != ir.blocks.len() {
            return Some(false);
        }
        // A local expression is ready after the latest local primitive binding
        // among its leaves. Foreign leaves must already be bound at entry, from
        // a strict dominator; this never treats a later same-block op as ready.
        let mut ready = BTreeMap::new();
        for expression in &program.expressions {
            let Some(block) = self.blocks.get(&expression.block).copied() else {
                return Some(false);
            };
            let operands = match &expression.kind {
                Kind::WordOp { arguments, .. } => arguments.len(),
                Kind::Compare { .. } => 2,
                _ => 1,
            };
            if !pay(operands.saturating_add(1)) {
                return None;
            }
            let mut after = 0;
            if let Kind::Value(value) = expression.kind {
                let Some(definition) = self.definitions.get(&value) else {
                    return Some(false);
                };
                if !bound.contains(&value) || self.aliases.contains_key(&value) {
                    return Some(false);
                }
                if definition.block == block {
                    after = definition.position.map_or(0, |position| position + 1);
                } else if !self.available_at_entry(value, block) {
                    return Some(false);
                }
            }
            for operand in expression.kind.operands() {
                let Some((scope, position)) = ready.get(&operand).copied() else {
                    return Some(false);
                };
                if scope != block {
                    return Some(false);
                }
                after = after.max(position);
            }
            ready.insert(expression.id, (block, after));
        }
        let available = |expression, block, position| {
            ready
                .get(&expression)
                .is_some_and(|(scope, after)| *scope == block && *after <= position)
        };
        for block in &program.blocks {
            let index = self.blocks[&block.block];
            let end = ir.blocks[index].operations.len();
            if !pay(block.exit_values.len().saturating_add(2)) {
                return None;
            }
            if !block
                .condition
                .into_iter()
                .chain(block.target)
                .chain(block.exit_values.iter().copied())
                .all(|expression| available(expression, index, end))
            {
                return Some(false);
            }
            for statement in &block.statements {
                if !pay(statement.inputs.len().saturating_add(1)) {
                    return None;
                }
                let position = self.operations[&statement.operation].position?;
                if !statement
                    .inputs
                    .iter()
                    .all(|input| available(*input, index, position))
                {
                    return Some(false);
                }
            }
        }
        if program.transfers.len() != ir.edges.len() {
            return Some(false);
        }
        for (transfer, edge) in program.transfers.iter().zip(&ir.edges) {
            if !pay(transfer.arguments.len().saturating_add(1)) {
                return None;
            }
            let from = self.blocks[&edge.from];
            if transfer.edge != edge.id
                || transfer.arguments.len() != arities[self.blocks[&edge.to]]
                || !transfer
                    .arguments
                    .iter()
                    .all(|arg| available(*arg, from, ir.blocks[from].operations.len()))
            {
                return Some(false);
            }
        }
        for control in &program.controls {
            if !pay(1) {
                return None;
            }
            let Some((block, after)) = ready.get(&control.condition) else {
                return Some(false);
            };
            if *after > ir.blocks[*block].operations.len() {
                return Some(false);
            }
        }
        Some(true)
    }
}
