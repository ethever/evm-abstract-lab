//! Backwards first-write fill of an exact canonical Error(string) range.
use super::{Memory, Word};
use crate::{EntityId, MAX_ERROR_PAYLOAD_BYTES, U256};
use std::collections::BTreeSet;
pub(super) fn terminal_range<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    op: &crate::Operation,
) -> Option<(Word, usize)> {
    let offset = memory.word(*op.inputs.first()?)?;
    let size = usize::try_from(memory.word(*op.inputs.get(1)?)?.exact()?).ok()?;
    if !(68..=MAX_ERROR_PAYLOAD_BYTES).contains(&size)
        || (size - 68) % 32 != 0
        || offset.end(size).is_none()
    {
        return None;
    }
    Some((offset, size))
}
pub(super) fn decode(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 68
        || bytes[..4] != [8, 0xc3, 0x79, 0xa0]
        || U256::from_be_slice(&bytes[4..36]) != U256::from(32)
    {
        return None;
    }
    let len = usize::try_from(U256::from_be_slice(&bytes[36..68])).ok()?;
    let padded = len.checked_add(31)? / 32 * 32;
    if 68_usize.checked_add(padded)? != bytes.len() || bytes[68 + len..].iter().any(|b| *b != 0) {
        return None;
    }
    String::from_utf8(bytes[68..68 + len].to_vec()).ok()
}
/// Relative interval intersection requires proved ordinary, nonwrapping memory
/// ranges. Arithmetic equality here is exact symbolic identity, never bounds.
pub(super) fn overlap(
    base: Word,
    size: usize,
    write: Word,
    count: usize,
) -> Option<(usize, usize, usize)> {
    base.end(size)?;
    write.end(count)?;
    if base.base != write.base {
        return None;
    }
    if let Some(delta) = write.relative(base)
        && delta < size
    {
        return Some((delta, 0, count.min(size - delta)));
    }
    if let Some(delta) = base.relative(write)
        && delta < count
    {
        return Some((0, delta, size.min(count - delta)));
    }
    None
}
/// Materialize only the intersecting bytes, not an arbitrarily large copy.
pub(super) fn writer_bytes<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    op: &crate::Operation,
    start: usize,
    len: usize,
) -> Option<Vec<u8>> {
    if !memory.charge(len.saturating_add(1)) {
        return None;
    }
    match op.opcode {
        0x52 | 0x53 if op.inputs.len() == 2 => {
            let word = memory.word(op.inputs[1])?.exact()?.to_be_bytes::<32>();
            let data = if op.opcode == 0x52 {
                &word[..]
            } else {
                &word[31..]
            };
            Some(data.get(start..start.checked_add(len)?)?.to_vec())
        }
        0x39 if op.inputs.len() == 3 => {
            let source = memory.word(op.inputs[1])?.exact()?;
            let count = usize::try_from(memory.word(op.inputs[2])?.exact()?).ok()?;
            if start.checked_add(len)? > count {
                return None;
            }
            let code = memory.ir.runtime_bytecode.as_deref()?;
            if code.len() != memory.ir.bytecode_len {
                return None;
            }
            // Source indices past code are zero. In particular, a huge offset
            // must not wrap back to the beginning of the captured runtime.
            let source = usize::try_from(source)
                .ok()
                .and_then(|n| n.checked_add(start));
            Some(
                (0..len)
                    .map(|n| {
                        source
                            .and_then(|s| s.checked_add(n))
                            .and_then(|index| code.get(index))
                            .copied()
                            .unwrap_or(0)
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}
pub(super) fn search<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    mut version: EntityId,
    base: Word,
    size: usize,
) -> Option<(Vec<u8>, Vec<EntityId>)> {
    if !memory.charge(size.saturating_mul(2)) {
        return None;
    }
    let mut bytes = vec![None; size];
    let mut remaining = size;
    if let Some(start) = memory.zero_suffix(version, base, size) {
        if start > size {
            return None;
        }
        bytes[start..].fill(Some(0));
        remaining = start;
    }
    let mut writes = Vec::new();
    let mut seen = BTreeSet::new();
    while remaining > 0 {
        if !memory.charge(1) || !seen.insert(version) {
            return None;
        }
        if let Some(op) = memory.writer(version)
            && !memory.writer_disjoint(op, base, size)
        {
            let (offset, count) = memory.write_range(op)?;
            let (dst, src, len) = overlap(base, size, offset, count)?;
            let data = writer_bytes(memory, op, src, len)?;
            if !memory.charge(len) {
                return None;
            }
            if writes.len() == MAX_ERROR_PAYLOAD_BYTES {
                return None;
            }
            writes.push(op.id);
            for i in 0..len {
                if bytes[dst + i].is_none() {
                    bytes[dst + i] = Some(data[i]);
                    remaining -= 1;
                }
            }
        }
        if remaining > 0 {
            version = memory.predecessor_memory(version)?;
        }
    }
    writes.reverse();
    Some((bytes.into_iter().collect::<Option<Vec<_>>>()?, writes))
}
