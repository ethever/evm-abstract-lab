//! Lazy consumers of an already prepared returndata copy. The copy remains at
//! its source position; only the failed-path terminal suffix belongs here.
use super::{Memory, capsule_operation};
use crate::{EntityId, LiftedByteSegment, LiftedByteSource, LiftedRevertDataKind};
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

/// This predicate is scoped by a PreparedConsumer certificate. It must never
/// make RETURNDATASIZE transparent in an ordinary failure route.
pub(in crate::lift) fn consumer_operation(op: &crate::Operation) -> bool {
    (op.opcode == 0xfd
        || op.effects.is_pure()
            && op.memory_input.is_none()
            && op.memory_output.is_none()
            && op.effect_input.is_none()
            && op.effect_output.is_none())
        && capsule_operation(op)
        || return_data_size_operation(op)
}

fn return_data_size_operation(op: &crate::Operation) -> bool {
    op.opcode == 0x3d
        && op.inputs.is_empty()
        && op.outputs.len() == 1
        && op.memory_input.is_none()
        && op.memory_output.is_none()
        && op.effect_input.is_some()
        && op.effect_output.is_some()
        && op.effects == crate::effects::effects(0x3d)
}

pub(super) fn start<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    block: &crate::Block,
    body: &crate::LiftedBlock,
    proof: &crate::LiftedRevertData,
    dependencies: &BTreeSet<EntityId>,
) -> Option<usize> {
    if !memory.charge(
        block
            .operations
            .len()
            .saturating_add(body.statements.len())
            .saturating_mul(4)
            .saturating_add(8),
    ) {
        return None;
    }
    let LiftedRevertDataKind::Segments(parts) = &proof.content else {
        return None;
    };
    let [
        LiftedByteSegment::Slice {
            source: LiftedByteSource::ReturnData(call),
            ..
        },
    ] = parts.as_slice()
    else {
        return None;
    };
    let [writer] = proof.writes.as_slice() else {
        return None;
    };
    let copy = memory.operations.get(writer).copied()?;
    if copy.opcode != 0x3e
        || copy.inputs.len() != 3
        || block.operations.iter().any(|op| op.id == *writer)
        || copy.effects != crate::effects::effects(0x3e)
        || copy.memory_input.is_none()
        || copy.memory_output.is_none()
        || copy.effect_input.is_none()
        || copy.effect_output.is_none()
        || !memory.acyclic_operation(copy)
        || memory.return_data_call(copy.effect_input?) != Some(*call)
    {
        return None;
    }
    // The byte certificate proves this particular reaching copy and its range.
    // RDS is admitted separately and only when it supplies the terminal range
    // from the same buffer epoch. An unrelated read remains a suffix barrier.
    let mut start = 0;
    for (position, statement) in body.statements.iter().enumerate().rev() {
        if !memory.charge(2) {
            return None;
        }
        let op = memory.operations.get(&statement.operation).copied()?;
        let allowed = if return_data_size_operation(op) {
            dependencies.contains(&op.id)
                && memory.acyclic_operation(op)
                && memory.return_data_call(op.effect_input?) == Some(*call)
        } else if op.opcode == 0xfd {
            position + 1 == body.statements.len()
                && op.id == proof.operation
                && consumer_operation(op)
        } else {
            consumer_operation(op)
        };
        if !allowed {
            start = position + 1;
            break;
        }
    }
    (start < body.statements.len()
        && body
            .statements
            .last()
            .is_some_and(|statement| statement.operation == proof.operation))
    .then_some(start)
}
