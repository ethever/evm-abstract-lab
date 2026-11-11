//! Legacy Error(string) range admission and local source byte primitives.
use super::{Memory, Word};
use crate::MAX_ERROR_PAYLOAD_BYTES;
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
