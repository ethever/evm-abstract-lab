//! Check complete byte provenance, independently of format and discovery.
//!
//! A backward walk checks that the supplied writer list omits no interfering
//! write. A forward replay then applies last-write-wins to byte identities.
//! Unknown format does not weaken either proof obligation.
use super::super::{Memory, Word, overlap, writer_bytes};
use crate::{
    EntityId, LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertData,
    LiftedRevertDataKind, MAX_REVERT_DATA_BYTES, U256,
};
use alloy_primitives::B256;
use std::collections::BTreeSet;
mod epochs;
pub(super) use epochs::Epochs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Byte {
    Constant(u8),
    Value(EntityId, u8),
    Input(LiftedByteSource, U256),
}

pub(super) fn verify<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    proof: &LiftedRevertData,
    code_hash: Option<B256>,
    epochs: &mut Epochs<'_>,
) -> Option<bool> {
    if !memory.charge(proof.writes.len().saturating_add(1)) {
        return None;
    }
    let Some(block) = memory.ir.block(proof.block) else {
        return Some(false);
    };
    let Some(op) = block.operations.last() else {
        return Some(false);
    };
    if proof.execution_domain != memory.ir.execution_domain
        || proof.execution_fork != memory.ir.execution_fork
        || op.opcode != 0xfd
        || op.id != proof.operation
        || op.inputs.as_slice() != [proof.offset, proof.size]
        || op.memory_input != Some(proof.memory)
        || !matches!(&block.terminator,
            crate::Terminator::Exit { kind: crate::ExitKind::Revert, values }
                if values == &op.inputs)
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
    let LiftedRevertDataKind::Segments(segments) = &proof.content else {
        // Raw is precisely the original memory slice; it asserts no recovered
        // bytes and cannot be used to hide a constructor.
        return Some(proof.writes.is_empty() && proof.code_hash.is_none());
    };
    if let [
        LiftedByteSegment::Slice {
            source,
            offset,
            size,
        },
    ] = segments.as_slice()
        && whole_copy(memory, proof, *source, *offset, *size, epochs)?
    {
        return Some(true);
    }
    let Some(size) = memory
        .word(proof.size)
        .and_then(Word::exact)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n <= MAX_REVERT_DATA_BYTES)
    else {
        return Some(false);
    };
    if !memory.charge(size.saturating_mul(3).saturating_add(segments.len())) {
        return None;
    }
    // A zero-length REVERT ignores its offset, even if that offset is unknown.
    if size == 0 {
        return Some(segments.is_empty() && proof.writes.is_empty());
    }
    let Some(base) = memory
        .word(proof.offset)
        .filter(|base| base.end(size).is_some())
    else {
        return Some(false);
    };
    let Some(expected) = expected(memory, segments, size) else {
        return Some(false);
    };
    if !complete_chain(memory, proof, base, size)? {
        return Some(false);
    }
    let mut actual = vec![None; size];
    if let Some(start) = memory.zero_suffix(proof.memory, base, size) {
        if start > size {
            return Some(false);
        }
        actual[start..].fill(Some(Byte::Constant(0)));
    }
    for id in &proof.writes {
        let Some(writer) = memory.operations.get(id).copied() else {
            return Some(false);
        };
        let Some((address, count)) = memory.write_range(writer) else {
            return Some(false);
        };
        let Some((destination, source, length)) = overlap(base, size, address, count) else {
            return Some(false);
        };
        if !memory.charge(length) {
            return None;
        }
        match writer.opcode {
            0x52 | 0x53 if writer.inputs.len() == 2 => {
                let value = word(memory, writer.inputs[1]);
                if matches!(value, LiftedDataWord::SourceValue(_))
                    && !memory.acyclic_operation(writer)
                {
                    return Some(false);
                }
                let first = source + if writer.opcode == 0x53 { 31 } else { 0 };
                if first.checked_add(length).is_none_or(|end| end > 32) {
                    return Some(false);
                }
                for i in 0..length {
                    actual[destination + i] = Some(byte(value, first + i));
                }
            }
            0x39 => {
                let Some(bytes) = writer_bytes(memory, writer, source, length) else {
                    return Some(false);
                };
                for (i, b) in bytes.into_iter().enumerate() {
                    actual[destination + i] = Some(Byte::Constant(b));
                }
            }
            0x37 | 0x3e if writer.inputs.len() == 3 && memory.acyclic_operation(writer) => {
                let Some(start) = memory.word(writer.inputs[1]).and_then(Word::exact) else {
                    return Some(false);
                };
                let Some(source_kind) = copy_source(memory, writer, epochs) else {
                    return Some(false);
                };
                for i in 0..length {
                    let index = start.checked_add(U256::from(source + i));
                    actual[destination + i] = match index {
                        Some(index) => Some(Byte::Input(source_kind, index)),
                        None if source_kind == LiftedByteSource::Calldata => {
                            Some(Byte::Constant(0))
                        }
                        _ => return Some(false),
                    };
                }
            }
            _ => return Some(false),
        }
    }
    Some(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| *actual == Some(expected)),
    )
}

fn word<P: FnMut(usize) -> bool>(memory: &mut Memory<'_, P>, value: EntityId) -> LiftedDataWord {
    memory
        .word(value)
        .and_then(Word::exact)
        .map_or(LiftedDataWord::SourceValue(value), LiftedDataWord::Constant)
}

fn byte(value: LiftedDataWord, index: usize) -> Byte {
    match value {
        LiftedDataWord::Constant(value) => Byte::Constant(value.to_be_bytes::<32>()[index]),
        LiftedDataWord::SourceValue(value) => Byte::Value(value, index as u8),
    }
}

fn expected<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    segments: &[LiftedByteSegment],
    size: usize,
) -> Option<Vec<Byte>> {
    let mut result = Vec::with_capacity(size);
    for segment in segments {
        match segment {
            LiftedByteSegment::Bytes(bytes) => {
                if result.len().checked_add(bytes.len())? > size {
                    return None;
                }
                result.extend(bytes.iter().copied().map(Byte::Constant));
            }
            LiftedByteSegment::Word {
                value,
                offset,
                size: length,
            } => {
                let (offset, length) = (usize::from(*offset), usize::from(*length));
                if length == 0
                    || offset.checked_add(length)? > 32
                    || result.len().checked_add(length)? > size
                {
                    return None;
                }
                let value = match *value {
                    LiftedDataWord::Constant(_) => *value,
                    LiftedDataWord::SourceValue(value) => word(memory, value),
                };
                result.extend((offset..offset + length).map(|index| byte(value, index)));
            }
            LiftedByteSegment::Slice {
                source,
                offset,
                size: length,
            } => {
                let concrete = |memory: &mut Memory<'_, P>, word: LiftedDataWord| match word {
                    LiftedDataWord::Constant(value) => Some(value),
                    LiftedDataWord::SourceValue(value) => memory.word(value).and_then(Word::exact),
                };
                let offset = concrete(memory, *offset)?;
                let length = usize::try_from(concrete(memory, *length)?).ok()?;
                if result.len().checked_add(length)? > size {
                    return None;
                }
                for index in 0..length {
                    result.push(match offset.checked_add(U256::from(index)) {
                        Some(index) => Byte::Input(*source, index),
                        None if *source == LiftedByteSource::Calldata => Byte::Constant(0),
                        _ => return None,
                    });
                }
            }
        }
    }
    (result.len() == size).then_some(result)
}

fn copy_source<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    copy: &crate::Operation,
    epochs: &mut Epochs<'_>,
) -> Option<LiftedByteSource> {
    match copy.opcode {
        0x37 => Some(LiftedByteSource::Calldata),
        0x3e if memory.acyclic_operation(copy) => {
            let call = epochs.call(copy.effect_input?, memory)?;
            Some(LiftedByteSource::ReturnData(call))
        }
        _ => None,
    }
}

/// Copying a complete window followed by REVERT of that identical window does
/// not require enumerating bytes or bounding its dynamic length. No modular
/// relative-address arithmetic is used. Original COPY and REVERT remain the
/// execution/fault witness, including empty windows and huge memory requests.
fn whole_copy<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    proof: &LiftedRevertData,
    source: LiftedByteSource,
    offset: LiftedDataWord,
    size: LiftedDataWord,
    epochs: &mut Epochs<'_>,
) -> Option<bool> {
    let [id] = proof.writes.as_slice() else {
        return Some(false);
    };
    let Some(copy) = memory.operations.get(id).copied() else {
        return Some(false);
    };
    if !matches!(copy.opcode, 0x37 | 0x3e)
        || copy.inputs.len() != 3
        || !memory.acyclic_operation(copy)
        || copy_source(memory, copy, epochs) != Some(source)
        || word(memory, copy.inputs[1]) != offset
        || word(memory, copy.inputs[2]) != size
    {
        return Some(false);
    }
    let same = |memory: &mut Memory<'_, P>, a, b| {
        a == b
            || memory
                .word(a)
                .zip(memory.word(b))
                .is_some_and(|(a, b)| a.same(b))
    };
    if !same(memory, proof.offset, copy.inputs[0]) {
        return Some(false);
    }
    if !same(memory, proof.size, copy.inputs[2]) {
        let a = epochs.size_call(proof.size, memory);
        let b = epochs.size_call(copy.inputs[2], memory);
        if a.is_none() || a != b {
            return Some(false);
        }
    }
    let mut current = proof.memory;
    let mut seen = BTreeSet::new();
    loop {
        if !memory.charge(1) {
            return None;
        }
        if !seen.insert(current) {
            return Some(false);
        }
        if let Some(writer) = memory.writer(current) {
            return Some(writer.id == copy.id);
        }
        let Some(previous) = memory.predecessor_memory(current) else {
            return Some(false);
        };
        current = previous;
    }
}

fn complete_chain<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    proof: &LiftedRevertData,
    base: Word,
    size: usize,
) -> Option<bool> {
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
    Some(true)
}
