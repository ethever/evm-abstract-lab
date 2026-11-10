//! Constant MSTORE terminal certificates and occurrence-local lazy construction.
//! Discovery searches reaching writes backwards. Certificate admission replays
//! the supplied writes forwards and separately checks their memory chain; it
//! never calls discovery or semantic projection.
use crate::{EntityId, Ir, LiftedConstruction, LiftedErrorPayload, LiftedProgram, LiftedRevert};
use std::collections::{BTreeMap, BTreeSet};
mod memory;
use memory::{Memory, Word};
mod bytes;
use bytes::{decode, overlap, search, store_bytes, terminal_range};
mod construction;
pub(super) use construction::capsule_operation;
use construction::construction_dependencies;
mod certificate;
pub(super) use certificate::verify;

#[cfg(test)]
mod tests;

pub(super) fn discover(
    ir: &Ir,
    program: &mut LiftedProgram,
    mut new_id: impl FnMut(Vec<EntityId>) -> EntityId,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<()> {
    let mut memory = Memory::new(ir, pay)?;
    let mut occurrences = BTreeMap::<_, usize>::new();
    let mut exits = BTreeMap::<_, usize>::new();
    let mut nodes: Vec<_> = ir
        .structured
        .as_ref()
        .map(|s| &s.root)
        .into_iter()
        .collect();
    while let Some(node) = nodes.pop() {
        if !memory.charge(3) {
            return None;
        }
        match node.kind {
            crate::StructuredKind::Block { block } => *occurrences.entry(block).or_default() += 1,
            crate::StructuredKind::Exit { block } => *exits.entry(block).or_default() += 1,
            crate::StructuredKind::Dispatcher { ref cases, .. } => {
                for case in cases {
                    *occurrences.entry(case.block).or_default() += 1;
                    *exits.entry(case.block).or_default() += 1;
                }
            }
            _ => {}
        }
        super::children(node, &mut nodes);
    }
    let constants: BTreeMap<_, _> = ir
        .values
        .iter()
        .filter_map(|v| {
            if let crate::ValueKind::Constant(c) = v.kind {
                Some((v.id, c))
            } else {
                None
            }
        })
        .collect();
    let mut payloads = Vec::<LiftedErrorPayload>::new();
    let mut reverts = Vec::new();
    let mut constructions = Vec::new();
    let mut identities = BTreeMap::new();
    let high: BTreeMap<_, _> = program.blocks.iter().map(|b| (b.block, b)).collect();
    for block in &ir.blocks {
        if !memory.charge(1) {
            return None;
        }
        let Some(op) = block
            .operations
            .last()
            .filter(|o| o.opcode == 0xfd && o.inputs.len() == 2)
        else {
            continue;
        };
        if !matches!(&block.terminator,crate::Terminator::Exit { kind:crate::ExitKind::Revert, values } if *values==op.inputs)
        {
            continue;
        }
        let Some((offset, size)) = terminal_range(&mut memory, op) else {
            continue;
        };
        let Some((bytes, writes)) = search(&mut memory, op.memory_input?, offset, size) else {
            continue;
        };
        let Some(message) = decode(&bytes) else {
            continue;
        };
        if !memory.charge(
            bytes
                .len()
                .saturating_mul(2)
                .saturating_add(message.len())
                .saturating_add(writes.len()),
        ) {
            return None;
        }
        let payload = if let Some(id) = identities.get(&bytes) {
            *id
        } else {
            let id = new_id(
                std::iter::once(op.id)
                    .chain(writes.iter().flat_map(|id| {
                        std::iter::once(*id).chain(memory.operations[id].inputs.iter().copied())
                    }))
                    .collect(),
            );
            identities.insert(bytes.clone(), id);
            payloads.push(LiftedErrorPayload { id, message, bytes });
            id
        };
        let id = new_id(
            std::iter::once(op.id)
                .chain(op.inputs.iter().copied())
                .chain(op.memory_input)
                .chain(writes.iter().copied())
                .collect(),
        );
        reverts.push(LiftedRevert {
            id,
            execution_domain: ir.execution_domain,
            execution_fork: ir.execution_fork,
            block: block.id,
            operation: op.id,
            memory: op.memory_input?,
            offset: op.inputs[0],
            size: op.inputs[1],
            payload,
            writes,
        });
        // A capsule is a suffix of this exact source block. Prefix observations
        // stay visible, and guard discovery refuses them. Original statements
        // are retained unchanged as the lazy execution/fault witness.
        if let Some(body) = high.get(&block.id) {
            if !memory.charge(
                block
                    .operations
                    .iter()
                    .fold(body.statements.len().saturating_add(1), |n, o| {
                        n.saturating_add(o.inputs.len()).saturating_add(10)
                    }),
            ) {
                return None;
            }
            let operations: BTreeMap<_, _> = block.operations.iter().map(|o| (o.id, o)).collect();
            let dependencies = construction_dependencies(block, reverts.last()?, &constants);
            let start = body
                .statements
                .iter()
                .rposition(|s| {
                    !capsule_operation(operations[&s.operation])
                        || (matches!(operations[&s.operation].opcode, 0x51..=0x53)
                            && !dependencies.contains(&s.operation))
                })
                .map_or(0, |p| p + 1);
            let members: BTreeSet<_> = body.statements[start..]
                .iter()
                .map(|s| s.operation)
                .collect();
            if occurrences.get(&block.id).is_some_and(|count| *count > 0)
                && occurrences.get(&block.id) == exits.get(&block.id)
                && start < body.statements.len()
                && body.statements.last().is_some_and(|s| s.operation == op.id)
                && body.statements[start..]
                    .iter()
                    .any(|s| matches!(operations[&s.operation].opcode, 0x52 | 0x53))
                && reverts.last()?.writes.iter().all(|w| members.contains(w))
            {
                if !memory.charge(body.statements.len().saturating_add(1)) {
                    return None;
                }
                let plan = new_id(
                    std::iter::once(block.id)
                        .chain(body.statements[start..].iter().map(|s| s.operation))
                        .collect(),
                );
                constructions.push(LiftedConstruction {
                    id: plan,
                    block: block.id,
                    start,
                    end: body.statements.len(),
                    revert: id,
                });
            }
        }
    }
    if memory.refused {
        return None;
    }
    program.payloads = payloads;
    program.reverts = reverts;
    program.constructions = constructions;
    Some(())
}
