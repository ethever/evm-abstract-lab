//! Independent admission of lazy consumers of already copied return data.
//! Numeric byte replay proves the memory contents. This check proves which
//! failed-path observations may be covered by the selected terminal suffix;
//! it never consumes the prefix COPY or calls constructor discovery.
use super::{Memory, data::Epochs};
use crate::{
    LiftedByteSegment, LiftedByteSource, LiftedConstructionKind, LiftedProgram,
    LiftedRevertDataKind, Operation,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn verify<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    program: &LiftedProgram,
    epochs: &mut Epochs<'_>,
) -> Option<bool> {
    if !memory.charge(program.constructions.len()) {
        return None;
    }
    if !program
        .constructions
        .iter()
        .any(|plan| plan.kind == LiftedConstructionKind::PreparedConsumer)
    {
        return Some(true);
    }
    if !memory.charge(
        program
            .constructions
            .len()
            .saturating_add(program.revert_data.len())
            .saturating_add(program.blocks.len())
            .saturating_add(memory.ir.blocks.len())
            .saturating_mul(3),
    ) {
        return None;
    }
    let data: BTreeMap<_, _> = program
        .revert_data
        .iter()
        .map(|data| (data.id, data))
        .collect();
    let raw: BTreeMap<_, _> = memory
        .ir
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect();
    let high: BTreeMap<_, _> = program
        .blocks
        .iter()
        .map(|block| (block.block, block))
        .collect();
    for plan in &program.constructions {
        if plan.kind != LiftedConstructionKind::PreparedConsumer {
            continue;
        }
        let (Some(proof), Some(block), Some(body)) = (
            data.get(&plan.revert),
            raw.get(&plan.block),
            high.get(&plan.block),
        ) else {
            return Some(false);
        };
        if proof.block != plan.block
            || plan.start >= plan.end
            || plan.end != body.statements.len()
            || body
                .statements
                .last()
                .is_none_or(|statement| statement.operation != proof.operation)
        {
            return Some(false);
        }
        let LiftedRevertDataKind::Segments(parts) = &proof.content else {
            return Some(false);
        };
        let [
            LiftedByteSegment::Slice {
                source: LiftedByteSource::ReturnData(call),
                ..
            },
        ] = parts.as_slice()
        else {
            return Some(false);
        };
        let [writer] = proof.writes.as_slice() else {
            return Some(false);
        };
        if !memory.charge(block.operations.iter().fold(
            body.statements.len().saturating_add(1),
            |n, op| {
                n.saturating_add(op.inputs.len())
                    .saturating_add(op.outputs.len())
                    .saturating_add(5)
            },
        )) {
            return None;
        }
        let operations: BTreeMap<_, _> = block
            .operations
            .iter()
            .enumerate()
            .map(|(i, op)| (op.id, (i, op)))
            .collect();
        let definitions: BTreeMap<_, _> = block
            .operations
            .iter()
            .flat_map(|op| op.outputs.iter().map(move |value| (*value, op)))
            .collect();
        let Some(copy) = memory.operations.get(writer).copied() else {
            return Some(false);
        };
        if copy.opcode != 0x3e
            || copy.inputs.len() != 3
            || operations.contains_key(writer)
            || copy.effects != crate::effects::effects(0x3e)
            || copy.memory_input.is_none()
            || copy.memory_output.is_none()
            || copy.effect_input.is_none()
            || copy.effect_output.is_none()
            || !memory.acyclic_operation(copy)
            || epochs.call(copy.effect_input?, memory) != Some(*call)
        {
            return Some(false);
        }

        // Follow only the original terminal operands, not a published set of
        // claimed needed reads. Unrelated RDS remains an observation barrier.
        let mut needed = BTreeSet::new();
        let mut values = BTreeSet::new();
        let mut pending = vec![proof.offset, proof.size];
        while let Some(value) = pending.pop() {
            if !memory.charge(2) {
                return None;
            }
            if !values.insert(value) {
                continue;
            }
            if let Some(op) = definitions.get(&value) {
                if !memory.charge(op.inputs.len().saturating_add(1)) {
                    return None;
                }
                needed.insert(op.id);
                pending.extend(op.inputs.iter().copied());
            }
        }
        let mut expected_start = 0;
        let mut previous = None;
        for (index, statement) in body.statements.iter().enumerate() {
            if !memory.charge(2) {
                return None;
            }
            let Some((position, op)) = operations.get(&statement.operation).copied() else {
                return Some(false);
            };
            if previous.is_some_and(|previous| previous >= position) {
                return Some(false);
            }
            previous = Some(position);
            let allowed = if op.opcode == 0x3d {
                op.inputs.is_empty()
                    && op.outputs.len() == 1
                    && op.memory_input.is_none()
                    && op.memory_output.is_none()
                    && op.effect_input.is_some()
                    && op.effect_output.is_some()
                    && op.effects == crate::effects::effects(0x3d)
                    && needed.contains(&op.id)
                    && memory.acyclic_operation(op)
                    && epochs.call(op.effect_input?, memory) == Some(*call)
            } else if op.opcode == 0xfd {
                index + 1 == body.statements.len()
                    && op.id == proof.operation
                    && op.inputs.as_slice() == [proof.offset, proof.size]
                    && op.memory_input == Some(proof.memory)
                    && op.memory_output.is_none()
                    && op.outputs.is_empty()
                    && op.effects == crate::effects::effects(0xfd)
            } else {
                pure(op)
            };
            if !allowed {
                expected_start = index + 1;
            }
        }
        if expected_start != plan.start {
            return Some(false);
        }
    }
    Some(!memory.refused)
}

fn pure(op: &Operation) -> bool {
    if !op.effects.is_pure()
        || op.memory_input.is_some()
        || op.memory_output.is_some()
        || op.effect_input.is_some()
        || op.effect_output.is_some()
    {
        return false;
    }
    if evm_bytecode::word::arity(op.opcode) == Some(op.inputs.len()) && op.outputs.len() == 1 {
        return true;
    }
    match op.opcode {
        0x50 | 0x56 => op.inputs.len() == 1 && op.outputs.is_empty(),
        0x57 => op.inputs.len() == 2 && op.outputs.is_empty(),
        0x5b => op.inputs.is_empty() && op.outputs.is_empty(),
        0x5f..=0x7f => op.inputs.is_empty() && op.outputs.len() == 1,
        0x80..=0x8f => op.inputs.len() == 1 && op.outputs.is_empty(),
        0x90..=0x9f => op.inputs.len() == 2 && op.outputs.is_empty(),
        _ => false,
    }
}
