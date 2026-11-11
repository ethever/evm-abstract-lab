//! Source dependencies bound the exact lazy constructor statement suffix.
use crate::{EntityId, U256};
use std::collections::{BTreeMap, BTreeSet};
pub(in crate::lift) fn capsule_operation(op: &crate::Operation) -> bool {
    if op.effects.is_pure() {
        let arity = evm_bytecode::word::arity(op.opcode);
        return (arity == Some(op.inputs.len()) && op.outputs.len() == 1)
            || match op.opcode {
                0x50 | 0x56 => op.inputs.len() == 1 && op.outputs.is_empty(),
                0x57 => op.inputs.len() == 2 && op.outputs.is_empty(),
                0x5b => op.inputs.is_empty() && op.outputs.is_empty(),
                0x5f..=0x7f => op.inputs.is_empty() && op.outputs.len() == 1,
                0x80..=0x8f => op.inputs.len() == 1 && op.outputs.is_empty(),
                0x90..=0x9f => op.inputs.len() == 2 && op.outputs.is_empty(),
                _ => false,
            };
    }
    let e = &op.effects;
    let arity = match op.opcode {
        0x39 => 3,
        0x51 => 1,
        0x52 | 0x53 | 0xfd => 2,
        _ => return false,
    };
    op.inputs.len() == arity
        && op.outputs.len() == usize::from(op.opcode == 0x51)
        && op.memory_input.is_some()
        && op.memory_output.is_some() == matches!(op.opcode, 0x39 | 0x52 | 0x53)
        && e.memory_read == matches!(op.opcode, 0x51 | 0xfd)
        && e.memory_write == matches!(op.opcode, 0x39 | 0x52 | 0x53)
        && e.terminate == (op.opcode == 0xfd)
        && !e.storage_read
        && !e.storage_write
        && !e.transient_read
        && !e.transient_write
        && !e.return_data_read
        && !e.return_data_write
        && !e.world_read
        && !e.world_write
        && !e.call
        && !e.create
        && !e.log
}

/// Word dependencies plus exact source slot initializers; unrelated memory
/// observations/writes are barriers even though lazy replay could preserve them.
pub(super) fn data_construction_dependencies(
    block: &crate::Block,
    proof: &crate::LiftedRevertData,
    constants: &BTreeMap<EntityId, U256>,
) -> BTreeSet<EntityId> {
    dependencies(block, &proof.writes, proof.offset, proof.size, constants)
}

fn dependencies(
    block: &crate::Block,
    writes: &[EntityId],
    offset: EntityId,
    size: EntityId,
    constants: &BTreeMap<EntityId, U256>,
) -> BTreeSet<EntityId> {
    let definitions: BTreeMap<_, _> = block
        .operations
        .iter()
        .flat_map(|o| o.outputs.iter().map(move |id| (*id, o)))
        .collect();
    let operations: BTreeMap<_, _> = block.operations.iter().map(|o| (o.id, o)).collect();
    // Only exact cell writers are slice prerequisites. Byte/alias validity is
    // independently checked by the source memory certificate, not this index.
    let mut last_stores = BTreeMap::new();
    let mut load_sources = BTreeMap::new();
    for op in &block.operations {
        if let Some(address) = op.inputs.first().and_then(|id| constants.get(id)) {
            if op.opcode == 0x52 {
                last_stores.insert(*address, op);
            } else if op.opcode == 0x51
                && let Some(writer) = last_stores.get(address)
            {
                load_sources.insert(op.id, *writer);
            }
        }
    }
    let mut needed: BTreeSet<_> = writes.iter().copied().collect();
    let mut pending = vec![offset, size];
    for write in writes {
        if let Some(op) = operations.get(write) {
            pending.extend(&op.inputs);
        }
    }
    let mut seen = BTreeSet::new();
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
            continue;
        }
        let Some(op) = definitions.get(&value) else {
            continue;
        };
        needed.insert(op.id);
        pending.extend(&op.inputs);
        if let Some(writer) = load_sources.get(&op.id) {
            needed.insert(writer.id);
            pending.extend(&writer.inputs);
        }
    }
    needed
}
