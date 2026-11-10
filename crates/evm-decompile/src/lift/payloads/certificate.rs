//! Local certificate replay, independent of backwards payload discovery.
use super::{Memory, overlap, store_bytes, terminal_range};
use crate::{Ir, LiftedProgram};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::lift) fn verify(
    ir: &Ir,
    program: &LiftedProgram,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<bool> {
    if program.validate().is_err() || program.validate_guard_sources(ir).is_err() {
        return Some(false);
    }
    let mut memory = Memory::new(ir, pay)?;
    let raw: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let payloads: BTreeMap<_, _> = program.payloads.iter().map(|p| (p.id, p)).collect();
    for proof in &program.reverts {
        if proof.execution_domain != ir.execution_domain
            || proof.execution_fork != ir.execution_fork
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
            let Some(data) = store_bytes(&mut memory, writer) else {
                return Some(false);
            };
            if !memory.charge(len) {
                return None;
            }
            for i in 0..len {
                result[dst + i] = Some(data[src + i]);
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
    Some(!memory.refused)
}
