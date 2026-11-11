//! Format-independent reaching-byte queries. No selector, ABI length or literal
//! content decides whether a terminal memory range is queried.
use super::{Memory, Word, overlap, writer_bytes};
use crate::{
    EntityId, LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertDataKind,
    MAX_REVERT_DATA_BYTES, Operation, U256,
};
use std::collections::BTreeSet;
#[cfg(test)]
mod tests;

pub(super) struct Recovered {
    pub content: LiftedRevertDataKind,
    pub writes: Vec<EntityId>,
}
impl Recovered {
    fn raw() -> Self {
        Self {
            content: LiftedRevertDataKind::Raw,
            writes: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Byte(u8),
    Word {
        writer: EntityId,
        value: LiftedDataWord,
        index: u8,
    },
    Slice {
        writer: EntityId,
        source: LiftedByteSource,
        offset: U256,
        index: usize,
    },
}

pub(super) fn query<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    version: EntityId,
    offset: EntityId,
    size: EntityId,
) -> Option<Recovered> {
    if !memory.charge(1) {
        return None;
    }
    let count = memory
        .word(size)
        .and_then(Word::exact)
        .and_then(|n| usize::try_from(n).ok());
    if count == Some(0) {
        return Some(Recovered {
            content: LiftedRevertDataKind::Segments(Vec::new()),
            writes: Vec::new(),
        });
    }
    let whole = whole_copy(memory, version, offset, size);
    if memory.refused {
        return None;
    }
    if let Some(result) = whole {
        return Some(result);
    }
    let Some(count) = count.filter(|n| *n <= MAX_REVERT_DATA_BYTES) else {
        return (!memory.refused).then(Recovered::raw);
    };
    let Some(base) = memory.word(offset).filter(|base| base.end(count).is_some()) else {
        return (!memory.refused).then(Recovered::raw);
    };
    let recovered = search(memory, version, base, count);
    if memory.refused {
        None
    } else {
        Some(recovered.unwrap_or_else(Recovered::raw))
    }
}

fn source_word<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    value: EntityId,
) -> LiftedDataWord {
    memory
        .word(value)
        .and_then(Word::exact)
        .map_or(LiftedDataWord::SourceValue(value), LiftedDataWord::Constant)
}

fn same_word<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    a: EntityId,
    b: EntityId,
) -> bool {
    a == b
        || memory
            .word(a)
            .zip(memory.word(b))
            .is_some_and(|(a, b)| a.same(b))
}

fn whole_copy<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    mut version: EntityId,
    offset: EntityId,
    size: EntityId,
) -> Option<Recovered> {
    let mut seen = BTreeSet::new();
    while memory.writer(version).is_none() {
        if !memory.charge(1) || !seen.insert(version) {
            return None;
        }
        version = memory.predecessor_memory(version)?;
    }
    let copy = memory.writer(version)?;
    if !matches!(copy.opcode, 0x37 | 0x3e)
        || copy.inputs.len() != 3
        || !memory.acyclic_operation(copy)
        || !same_word(memory, copy.inputs[0], offset)
    {
        return None;
    }
    let source = if copy.opcode == 0x37 {
        LiftedByteSource::Calldata
    } else {
        LiftedByteSource::ReturnData(memory.return_data_call(copy.effect_input?)?)
    };
    let same_size = same_word(memory, copy.inputs[2], size)
        || memory
            .return_data_size_call(copy.inputs[2])
            .zip(memory.return_data_size_call(size))
            .is_some_and(|(a, b)| a == b);
    if !same_size || !memory.charge(8) {
        return None;
    }
    Some(Recovered {
        content: LiftedRevertDataKind::Segments(vec![LiftedByteSegment::Slice {
            source,
            offset: source_word(memory, copy.inputs[1]),
            size: source_word(memory, copy.inputs[2]),
        }]),
        writes: vec![copy.id],
    })
}

fn cells<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    op: &Operation,
    first: usize,
    len: usize,
) -> Option<Vec<Cell>> {
    if !memory.charge(len.saturating_add(1)) {
        return None;
    }
    match op.opcode {
        0x52 | 0x53 if op.inputs.len() == 2 => {
            let value = source_word(memory, op.inputs[1]);
            if matches!(value, LiftedDataWord::SourceValue(_)) && !memory.acyclic_operation(op) {
                return None;
            }
            let first = first.checked_add(if op.opcode == 0x53 { 31 } else { 0 })?;
            if first.checked_add(len)? > 32 {
                return None;
            }
            Some(
                (first..first + len)
                    .map(|index| Cell::Word {
                        writer: op.id,
                        value,
                        index: index as u8,
                    })
                    .collect(),
            )
        }
        0x39 => Some(
            writer_bytes(memory, op, first, len)?
                .into_iter()
                .map(Cell::Byte)
                .collect(),
        ),
        0x37 | 0x3e if op.inputs.len() == 3 && memory.acyclic_operation(op) => {
            let offset = memory.word(op.inputs[1])?.exact()?;
            let source = if op.opcode == 0x37 {
                LiftedByteSource::Calldata
            } else {
                LiftedByteSource::ReturnData(memory.return_data_call(op.effect_input?)?)
            };
            offset.checked_add(U256::from(first.checked_add(len)?))?;
            Some(
                (first..first + len)
                    .map(|index| Cell::Slice {
                        writer: op.id,
                        source,
                        offset,
                        index,
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

fn search<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    mut version: EntityId,
    base: Word,
    size: usize,
) -> Option<Recovered> {
    if !memory.charge(size.saturating_mul(3)) {
        return None;
    }
    let mut result = vec![None; size];
    let mut remaining = size;
    if let Some(start) = memory.zero_suffix(version, base, size) {
        if start > size {
            return None;
        }
        result[start..].fill(Some(Cell::Byte(0)));
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
            let (address, count) = memory.write_range(op)?;
            let (destination, source, len) = overlap(base, size, address, count)?;
            let bytes = cells(memory, op, source, len)?;
            if writes.len() == MAX_REVERT_DATA_BYTES || !memory.charge(len) {
                return None;
            }
            writes.push(op.id);
            for (index, byte) in bytes.into_iter().enumerate() {
                if result[destination + index].is_none() {
                    result[destination + index] = Some(byte);
                    remaining -= 1;
                }
            }
        }
        if remaining > 0 {
            version = memory.predecessor_memory(version)?;
        }
    }
    writes.reverse();
    let result = result.into_iter().collect::<Option<Vec<_>>>()?;
    if !memory.charge(size.saturating_mul(2)) {
        return None;
    }
    Some(Recovered {
        content: LiftedRevertDataKind::Segments(segments(&result)?),
        writes,
    })
}

fn segments(cells: &[Cell]) -> Option<Vec<LiftedByteSegment>> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let mut end = start + 1;
        match cells[start] {
            Cell::Byte(byte) => {
                let mut bytes = vec![byte];
                while end < cells.len() {
                    let Cell::Byte(next) = cells[end] else {
                        break;
                    };
                    bytes.push(next);
                    end += 1;
                }
                result.push(LiftedByteSegment::Bytes(bytes));
            }
            Cell::Word {
                writer,
                value,
                index,
            } => {
                while end < cells.len()
                    && usize::from(index) + end - start < 32
                    && cells[end]
                        == (Cell::Word {
                            writer,
                            value,
                            index: index + (end - start) as u8,
                        })
                {
                    end += 1;
                }
                result.push(LiftedByteSegment::Word {
                    value,
                    offset: index,
                    size: (end - start) as u8,
                });
            }
            Cell::Slice {
                writer,
                source,
                offset,
                index,
            } => {
                while end < cells.len()
                    && cells[end]
                        == (Cell::Slice {
                            writer,
                            source,
                            offset,
                            index: index + end - start,
                        })
                {
                    end += 1;
                }
                result.push(LiftedByteSegment::Slice {
                    source,
                    offset: LiftedDataWord::Constant(offset.checked_add(U256::from(index))?),
                    size: LiftedDataWord::Constant(U256::from(end - start)),
                });
            }
        }
        start = end;
    }
    Some(result)
}

/// Compatibility formatting may request exact bytes from the same recovered
/// data. Symbolic words and source slices never get guessed into constants.
pub(super) fn constant_bytes<P: FnMut(usize) -> bool>(
    memory: &mut Memory<'_, P>,
    content: &LiftedRevertDataKind,
) -> Option<Vec<u8>> {
    let LiftedRevertDataKind::Segments(parts) = content else {
        return None;
    };
    let mut count = 0usize;
    for part in parts {
        count = count.checked_add(match part {
            LiftedByteSegment::Bytes(bytes) => bytes.len(),
            LiftedByteSegment::Word {
                value: LiftedDataWord::Constant(_),
                size,
                ..
            } => usize::from(*size),
            _ => return None,
        })?;
        if count > MAX_REVERT_DATA_BYTES {
            return None;
        }
    }
    if !memory.charge(count.saturating_mul(2).saturating_add(parts.len())) {
        return None;
    }
    let mut bytes = Vec::with_capacity(count);
    for part in parts {
        match part {
            LiftedByteSegment::Bytes(data) => bytes.extend_from_slice(data),
            LiftedByteSegment::Word {
                value: LiftedDataWord::Constant(value),
                offset,
                size,
            } => {
                let word = value.to_be_bytes::<32>();
                let start = usize::from(*offset);
                bytes.extend_from_slice(word.get(start..start.checked_add(usize::from(*size))?)?);
            }
            _ => return None,
        }
    }
    Some(bytes)
}
