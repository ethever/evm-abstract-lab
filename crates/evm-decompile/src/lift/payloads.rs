//! Format-independent terminal data and occurrence-local lazy construction.
//! One reaching-memory query feeds data records. Error(string) is only a
//! compatibility adapter over completely recovered constant bytes.
use crate::{
    EntityId, Ir, LiftedConstruction, LiftedConstructionKind, LiftedErrorPayload, LiftedProgram,
    LiftedRevert, LiftedRevertData, LiftedRevertDataKind,
};
use std::collections::{BTreeMap, BTreeSet};
mod memory;
use memory::{Memory, Word};
mod bytes;
use bytes::{overlap, terminal_range, writer_bytes};
mod construction;
mod data;
mod prepared;
pub(super) use construction::capsule_operation;
use construction::data_construction_dependencies;
pub(super) use prepared::consumer_operation;
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
    let code_hash = if let Some(code) = &ir.runtime_bytecode {
        if code.len() != ir.bytecode_len || !memory.charge(code.len()) {
            return None;
        }
        Some(alloy_primitives::keccak256(code))
    } else {
        None
    };
    let mut occurrences = BTreeMap::<_, usize>::new();
    let mut exits = BTreeMap::<_, usize>::new();
    let mut pending: Vec<_> = ir
        .structured
        .as_ref()
        .map(|s| &s.root)
        .into_iter()
        .collect();
    while let Some(node) = pending.pop() {
        if !memory.charge(3) {
            return None;
        }
        match &node.kind {
            crate::StructuredKind::Block { block } => *occurrences.entry(*block).or_default() += 1,
            crate::StructuredKind::Exit { block } => *exits.entry(*block).or_default() += 1,
            crate::StructuredKind::Dispatcher { cases, .. } => {
                if !memory.charge(cases.len()) {
                    return None;
                }
                for case in cases {
                    *occurrences.entry(case.block).or_default() += 1;
                    *exits.entry(case.block).or_default() += 1;
                }
            }
            _ => {}
        }
        super::children(node, &mut pending);
    }
    if !memory.charge(
        ir.values
            .len()
            .saturating_add(program.blocks.len())
            .saturating_mul(4),
    ) {
        return None;
    }
    let constants: BTreeMap<_, _> = ir
        .values
        .iter()
        .filter_map(|value| {
            if let crate::ValueKind::Constant(word) = value.kind {
                Some((value.id, word))
            } else {
                None
            }
        })
        .collect();
    let high: BTreeMap<_, _> = program
        .blocks
        .iter()
        .map(|body| (body.block, body))
        .collect();
    let mut payloads = Vec::<LiftedErrorPayload>::new();
    let mut reverts = Vec::new();
    let mut revert_data = Vec::new();
    let mut constructions = Vec::new();
    let mut identities = BTreeMap::new();
    for block in &ir.blocks {
        if !memory.charge(1) {
            return None;
        }
        let Some(op) = block
            .operations
            .last()
            .filter(|op| op.opcode == 0xfd && op.inputs.len() == 2)
            .filter(|op| {
                matches!(&block.terminator,
                crate::Terminator::Exit {kind:crate::ExitKind::Revert,values} if values==&op.inputs)
            })
        else {
            continue;
        };
        let recovered = data::query(&mut memory, op.memory_input?, op.inputs[0], op.inputs[1])?;
        let copies_code = recovered
            .writes
            .iter()
            .any(|id| memory.operations[id].opcode == 0x39);
        let mut proof = LiftedRevertData {
            id: EntityId(0),
            execution_domain: ir.execution_domain,
            execution_fork: ir.execution_fork,
            code_hash: if copies_code { Some(code_hash?) } else { None },
            block: block.id,
            operation: op.id,
            memory: op.memory_input?,
            offset: op.inputs[0],
            size: op.inputs[1],
            content: recovered.content,
            writes: recovered.writes,
        };
        let references = proof.source_references().count();
        if !memory.charge(
            references
                .saturating_add(proof.record_count())
                .saturating_mul(2),
        ) {
            return None;
        }
        proof.id = new_id(proof.source_references().collect());

        // Legacy fields are an adapter, never a second memory-discovery path.
        let legacy = match proof.decode_format(|work| memory.charge(work)).ok()? {
            crate::LiftedRevertFormat::ErrorString(message) => {
                Some((data::constant_bytes(&mut memory, &proof.content)?, message))
            }
            _ => None,
        };
        let construction_terminal = if let Some((bytes, message)) = legacy {
            if !memory.charge(
                bytes
                    .len()
                    .saturating_mul(3)
                    .saturating_add(message.len())
                    .saturating_add(proof.writes.len())
                    .saturating_add(8),
            ) {
                return None;
            }
            let payload = if let Some(id) = identities.get(&bytes) {
                *id
            } else {
                let count = proof.writes.iter().fold(1usize, |n, id| {
                    n.saturating_add(1)
                        .saturating_add(memory.operations[id].inputs.len())
                });
                if !memory.charge(count) {
                    return None;
                }
                let id = new_id(
                    std::iter::once(op.id)
                        .chain(proof.writes.iter().flat_map(|id| {
                            std::iter::once(*id).chain(memory.operations[id].inputs.iter().copied())
                        }))
                        .collect(),
                );
                identities.insert(bytes.clone(), id);
                payloads.push(LiftedErrorPayload { id, message, bytes });
                id
            };
            if !memory.charge(proof.writes.len().saturating_add(4)) {
                return None;
            }
            let id = new_id(
                std::iter::once(op.id)
                    .chain(op.inputs.iter().copied())
                    .chain(op.memory_input)
                    .chain(proof.writes.iter().copied())
                    .collect(),
            );
            reverts.push(LiftedRevert {
                id,
                execution_domain: ir.execution_domain,
                execution_fork: ir.execution_fork,
                code_hash: proof.code_hash,
                block: block.id,
                operation: op.id,
                memory: op.memory_input?,
                offset: op.inputs[0],
                size: op.inputs[1],
                payload,
                writes: proof.writes.clone(),
            });
            id
        } else {
            proof.id
        };

        // Byte equality and lazy constructor admission remain separate. Original
        // operations stay executable; unrelated observations remain barriers.
        if matches!(&proof.content, LiftedRevertDataKind::Segments(parts) if !parts.is_empty())
            && let Some(body) = high.get(&block.id)
        {
            if !memory.charge(
                block
                    .operations
                    .iter()
                    .fold(body.statements.len().saturating_add(1), |n, op| {
                        n.saturating_add(op.inputs.len()).saturating_add(10)
                    }),
            ) {
                return None;
            }
            let operations: BTreeMap<_, _> =
                block.operations.iter().map(|op| (op.id, op)).collect();
            let dependencies = data_construction_dependencies(block, &proof, &constants);
            let start = body
                .statements
                .iter()
                .rposition(|statement| {
                    let operation = operations[&statement.operation];
                    !capsule_operation(operation)
                        || (matches!(operation.opcode, 0x39 | 0x51..=0x53)
                            && !dependencies.contains(&operation.id))
                })
                .map_or(0, |position| position + 1);
            let members: BTreeSet<_> = body.statements[start..]
                .iter()
                .map(|s| s.operation)
                .collect();
            let constructor = start < body.statements.len()
                && body.statements[start..]
                    .iter()
                    .any(|s| matches!(operations[&s.operation].opcode, 0x39 | 0x52 | 0x53))
                && proof.writes.iter().all(|id| members.contains(id));
            let suffix = if constructor {
                Some((start, LiftedConstructionKind::Constructor))
            } else {
                prepared::start(&mut memory, block, body, &proof, &dependencies)
                    .map(|start| (start, LiftedConstructionKind::PreparedConsumer))
            };
            if occurrences.get(&block.id).is_some_and(|count| *count > 0)
                && occurrences.get(&block.id) == exits.get(&block.id)
                && body.statements.last().is_some_and(|s| s.operation == op.id)
                && let Some((start, kind)) = suffix
            {
                if !memory.charge(body.statements.len().saturating_add(1)) {
                    return None;
                }
                let id = new_id(
                    std::iter::once(block.id)
                        .chain(body.statements[start..].iter().map(|s| s.operation))
                        .collect(),
                );
                constructions.push(LiftedConstruction {
                    id,
                    block: block.id,
                    start,
                    end: body.statements.len(),
                    revert: construction_terminal,
                    kind,
                });
            }
        }
        revert_data.push(proof);
    }
    if memory.refused {
        return None;
    }
    program.payloads = payloads;
    program.reverts = reverts;
    program.revert_data = revert_data;
    program.constructions = constructions;
    Some(())
}
