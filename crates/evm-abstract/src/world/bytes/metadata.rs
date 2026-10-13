//! Read-only retention metadata for bounded native snapshot consumers.
use super::ByteArray;
use crate::domain::symbolic::ExprId;

impl ByteArray {
    /// Number of entries visited when metering length, default, sparse cells,
    /// operand facts and allocation witnesses. Uses cached container lengths;
    /// consumers can prepay this scan before inspecting any entry.
    pub fn metadata_len(&self) -> usize {
        self.bytes
            .len()
            .saturating_add(self.operands.len())
            .saturating_add(self.allocations.len())
            .saturating_add(2)
    }

    /// Immutable allocation witnesses that participate in complete native
    /// equality and cloning, independently of the byte content root.
    pub fn allocation_facts(&self) -> impl ExactSizeIterator<Item = (&ExprId, &ExprId)> {
        self.allocations.iter().map(|(offset, size)| (offset, size))
    }
}
