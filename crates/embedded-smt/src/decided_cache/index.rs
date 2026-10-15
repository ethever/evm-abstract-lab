//! Fixed-width routing only. Mathematical cache hits still require full Eq.

use super::budget::{self, MaintenanceBudget};
use std::hash::{DefaultHasher, Hasher};

pub(super) const BUCKET_LIMIT: usize = 4;
pub(super) const INDEX_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Digest([u64; 2]);

pub(super) struct CanonicalKey {
    pub(super) bytes: Vec<u8>,
    pub(super) digest: Digest,
}

pub(super) fn digest(bytes: &[u8], budget: &mut MaintenanceBudget<'_>) -> Option<Digest> {
    // Both complete byte passes are paid before hashing. Hash stability and
    // collision resistance are irrelevant to proof: the bounded bucket checks
    // every canonical byte before returning a stored mathematical answer.
    // Each pass includes its eight-byte domain word and fixed scan setup.
    if !budget.charge(budget::linear_scan_work(bytes.len().saturating_add(8)).saturating_mul(2)) {
        return None;
    }
    let mut first = DefaultHasher::new();
    first.write_u64(0);
    first.write(bytes);
    let mut second = DefaultHasher::new();
    second.write_u64(1);
    second.write(bytes);
    Some(Digest([first.finish(), second.finish()]))
}

pub(super) fn equality_work(entries: usize, bytes: usize) -> usize {
    // Prepay at most four full linear comparisons, even if a length check or
    // early mismatch can finish sooner. Full Eq remains the only hit proof.
    budget::linear_scan_work(bytes).saturating_mul(entries)
}

pub(super) fn bucket_work() -> usize {
    // A bounded scan/move of Entry metadata during age lookup or bucket update.
    64usize.saturating_mul(BUCKET_LIMIT)
}

#[cfg(test)]
pub(super) fn collision() -> Digest {
    Digest([0; 2])
}
