use super::{CacheScope, DecidedCache};
use crate::Cancellation;

/// Short borrow of an analysis owner and its existing work ledger. Optional
/// maintenance never marks exhaustion, and dropping it never refunds work.
#[derive(Debug)]
pub struct CacheQuery<'a> {
    pub(super) cache: &'a mut DecidedCache,
    pub(super) scope: CacheScope,
    pub(super) budget: MaintenanceBudget<'a>,
}
impl<'a> CacheQuery<'a> {
    /// Attach no native state and copy no analysis ledger.
    pub fn new(
        maximum: usize,
        consumed: &'a mut usize,
        cancellation: Cancellation,
        cache: &'a mut DecidedCache,
        scope: CacheScope,
    ) -> Self {
        Self {
            cache,
            scope,
            budget: MaintenanceBudget {
                maximum,
                consumed,
                cancellation,
                work: 0,
                allowance: usize::MAX,
            },
        }
    }
    /// Bound optional maintenance independently of the mandatory query permit.
    /// A refused step falls back to the oracle and retains previously paid work.
    pub fn with_allowance(mut self, allowance: usize) -> Self {
        self.budget.allowance = allowance;
        self
    }
}
impl Drop for CacheQuery<'_> {
    fn drop(&mut self) {
        self.cache.stats.maintenance_work = self
            .cache
            .stats
            .maintenance_work
            .saturating_add(self.budget.work as u64);
    }
}
#[derive(Debug)]
pub(super) struct MaintenanceBudget<'a> {
    maximum: usize,
    consumed: &'a mut usize,
    cancellation: Cancellation,
    work: usize,
    allowance: usize,
}
impl MaintenanceBudget<'_> {
    pub(super) fn charge(&mut self, amount: usize) -> bool {
        if amount == usize::MAX
            || self.cancellation.is_cancelled()
            || amount > self.allowance
            || amount > self.maximum.saturating_sub(*self.consumed)
        {
            return false;
        }
        *self.consumed += amount;
        self.work += amount;
        self.allowance -= amount;
        true
    }
}
/// Logical work for balanced-tree comparisons and path/maintenance steps.
/// The factor includes scans within tree nodes and balancing metadata. It is a
/// conservative accounting unit, not a claim about CPU instruction counts.
pub(super) fn tree_work(entries: usize, bytes: usize) -> usize {
    let levels = entries.checked_ilog2().unwrap_or(0) as usize + 1;
    64usize
        .saturating_mul(levels)
        .saturating_mul(bytes.saturating_add(1))
}

/// One logical unit per byte visited (or byte pair compared), plus scan setup.
/// This describes linear work, not a CPU instruction or cycle count.
pub(super) fn linear_scan_work(bytes: usize) -> usize {
    bytes.saturating_add(1)
}

/// Fixed-width hashing, probing, full pointer equality and slot access. This is
/// an expected/amortized estimate; collisions do not have a constant-time bound.
pub(super) fn hash_work(operations: usize) -> usize {
    64usize.saturating_mul(operations)
}

/// Pay expected old-table rehash/moves separately from linear new-table storage.
/// The new capacity bound includes load-factor slack; two metadata bytes per
/// slot and fixed padding cover control initialization in this logical policy.
/// It bounds admitted capacity, not allocator RSS or adversarial probe cost.
pub(super) fn hash_growth_work(old_capacity: usize, new_capacity_bound: usize) -> usize {
    hash_work(old_capacity)
        .saturating_add(
            new_capacity_bound
                .saturating_mul(std::mem::size_of::<(usize, u32)>().saturating_add(2)),
        )
        .saturating_add(64)
}
