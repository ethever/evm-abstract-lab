//! Local certificate replay, independent of backwards payload discovery.
use super::{Memory, overlap, terminal_range, writer_bytes};
use crate::{Ir, LiftedProgram};
use std::collections::{BTreeMap, BTreeSet};
mod data;
mod prepared;

pub(in crate::lift) fn verify(
    ir: &Ir,
    program: &LiftedProgram,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<bool> {
    if !pay(ir.runtime_bytecode.as_ref().map_or(0, Vec::len)) {
        return None;
    }
    if program.validate().is_err() || program.validate_guard_sources(ir).is_err() {
        return Some(false);
    }
    if !pay(program.constructions.len()) {
        return None;
    }
    if program.revert_data.is_empty()
        && program
            .constructions
            .iter()
            .any(|plan| plan.kind == crate::LiftedConstructionKind::PreparedConsumer)
    {
        return Some(false);
    }
    let mut memory = Memory::new(ir, pay)?;
    let code_hash = if let Some(code) = &ir.runtime_bytecode {
        if code.len() != ir.bytecode_len || !memory.charge(code.len()) {
            return None;
        }
        Some(alloy_primitives::keccak256(code))
    } else {
        None
    };
    let raw: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let payloads: BTreeMap<_, _> = program.payloads.iter().map(|p| (p.id, p)).collect();
    for proof in &program.reverts {
        if proof.execution_domain != ir.execution_domain
            || proof.execution_fork != ir.execution_fork
        {
            return Some(false);
        }
        let copies_code = proof.writes.iter().any(|id| {
            memory
                .operations
                .get(id)
                .is_some_and(|op| op.opcode == 0x39)
        });
        if proof.code_hash != if copies_code { code_hash } else { None }
            || (copies_code && code_hash.is_none())
        {
            return Some(false);
        }
        if !memory.charge(proof.writes.len().saturating_add(1)) {
            return None;
        }
        let Some(block) = raw.get(&proof.block) else {
            return Some(false);
        };
        let Some(op) = block.operations.last() else {
            return Some(false);
        };
        if op.opcode != 0xfd
            || op.id != proof.operation
            || op.inputs.as_slice() != [proof.offset, proof.size]
            || op.memory_input != Some(proof.memory)
        {
            return Some(false);
        }
        let Some(payload) = payloads.get(&proof.payload) else {
            return Some(false);
        };
        let Some((base, size)) = terminal_range(&mut memory, op) else {
            return Some(false);
        };
        if size != payload.bytes.len() || !memory.charge(size.saturating_mul(2)) {
            return Some(false);
        }
        // Check completeness and ordering of the supplied source memory chain.
        // Every possibly overlapping intervening writer must be named. Memory
        // joins are traversed only when their complete incoming identities agree.
        let mut current = proof.memory;
        let mut pending = proof.writes.len();
        let mut seen = BTreeSet::new();
        while pending > 0 {
            if !memory.charge(1) {
                return None;
            }
            if !seen.insert(current) {
                return Some(false);
            }
            if let Some(writer) = memory.writer(current)
                && !memory.writer_disjoint(writer, base, size)
            {
                let Some((address, count)) = memory.write_range(writer) else {
                    return Some(false);
                };
                if proof.writes[pending - 1] != writer.id
                    || overlap(base, size, address, count).is_none()
                {
                    return Some(false);
                }
                pending -= 1;
            }
            if pending > 0 {
                let Some(previous) = memory.predecessor_memory(current) else {
                    return Some(false);
                };
                current = previous;
            }
        }
        // Forward last-write-wins replay is intentionally a different traversal
        // and byte construction from discovery's backwards first-write fill.
        let mut result = vec![None; size];
        if let Some(start) = memory.zero_suffix(proof.memory, base, size) {
            if start > size {
                return Some(false);
            }
            result[start..].fill(Some(0));
        }
        for write in &proof.writes {
            let Some(writer) = memory.operations.get(write).copied() else {
                return Some(false);
            };
            let Some((address, count)) = memory.write_range(writer) else {
                return Some(false);
            };
            let Some((dst, src, len)) = overlap(base, size, address, count) else {
                return Some(false);
            };
            let Some(data) = writer_bytes(&mut memory, writer, src, len) else {
                return Some(false);
            };
            if !memory.charge(len) {
                return None;
            }
            for i in 0..len {
                result[dst + i] = Some(data[i]);
            }
        }
        if result
            .iter()
            .zip(&payload.bytes)
            .any(|(actual, expected)| *actual != Some(*expected))
        {
            return Some(false);
        }
    }
    if !program.revert_data.is_empty() {
        let mut epochs = data::Epochs::new(ir, &mut memory.pay)?;
        for proof in &program.revert_data {
            if !data::verify(&mut memory, proof, code_hash, &mut epochs)? {
                return Some(false);
            }
        }
        if !prepared::verify(&mut memory, program, &mut epochs)? {
            return Some(false);
        }
    }
    Some(!memory.refused)
}
