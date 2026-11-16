//! Occurrence-local ranges of the original memory requests. A declared resource
//! premise may bound an accepted request; it never narrows source word caches.
//! Constructors still execute every primitive through REVERT before observing
//! these bytes, preserving the complete prefix when a request faults instead.
use super::{Memory, Word};
use crate::{EntityId, ExitKind, Operation, Terminator, U256};

#[derive(Clone, Copy, Debug)]
pub(in crate::lift::payloads) struct AccessRange {
    pub base: Word,
    pub size: usize,
}

impl AccessRange {
    fn new(mut base: Word, size: usize, end_bound: Option<U256>) -> Option<Self> {
        if size == 0 {
            return Some(Self {
                base: Word::constant(U256::ZERO),
                size,
            });
        }
        if let Some(end) = end_bound {
            base.high = base.high.min(end.checked_sub(U256::from(size))?);
            if base.low > base.high {
                return None;
            }
        }
        base.end(size)?;
        Some(Self { base, size })
    }
}

impl<'a, P: FnMut(usize) -> bool> Memory<'a, P> {
    pub fn revert_operation(&self, id: EntityId) -> Option<&'a Operation> {
        let op = *self.operations.get(&id)?;
        let block = self.blocks.get(self.operation_blocks.get(&id)?)?;
        (op.opcode == 0xfd
            && op.inputs.len() == 2
            && op.memory_input.is_some()
            && op.memory_output.is_none()
            && op.effects == crate::effects::effects(0xfd)
            && block.operations.last() == Some(op)
            && matches!(&block.terminator,
                Terminator::Exit { kind: ExitKind::Revert, values } if values == &op.inputs))
        .then_some(op)
    }

    pub fn terminal_access(&mut self, id: EntityId) -> Option<AccessRange> {
        let op = self.revert_operation(id)?;
        let size = usize::try_from(self.word(op.inputs[1])?.exact()?).ok()?;
        let base = if size == 0 {
            Word::constant(U256::ZERO)
        } else {
            self.word(op.inputs[0])?
        };
        AccessRange::new(base, size, self.memory_end_bound)
    }

    /// The caller supplies a reaching memory version, rather than an arbitrary
    /// instruction present elsewhere in the source. Both discovery and replay
    /// walk the actual chain before using this conditional request bound.
    pub fn write_access(&mut self, version: EntityId) -> Option<AccessRange> {
        let op = self.writer(version)?;
        if op.effects != crate::effects::effects(op.opcode) {
            return None;
        }
        let (base, size) = self.write_range(op)?;
        AccessRange::new(base, size, self.memory_end_bound)
    }

    pub fn access_disjoint(&mut self, version: EntityId, target: AccessRange) -> bool {
        let Some(op) = self.writer(version) else {
            return false;
        };
        self.writer_disjoint(op, target.base, target.size)
            || self.write_access(version).is_some_and(|write| {
                self.disjoint(write.base, write.size, target.base, target.size)
            })
    }
}

#[cfg(test)]
mod tests;
