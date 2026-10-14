//! Query-local pointer identity lookups. Tables never define serialization order.
//! Capacity admission is a bounded logical policy, not an allocator-RSS bound.

use super::{Cancellation, MaintenanceBudget, Result};
use crate::decided_cache::budget::{hash_growth_work, hash_work};
use std::collections::HashMap;

pub(super) fn get(
    table: &HashMap<usize, u32>,
    pointer: usize,
    budget: &mut MaintenanceBudget<'_>,
) -> Result<Option<u32>> {
    if !budget.charge(hash_work(1)) {
        return Err(());
    }
    Ok(table.get(&pointer).copied())
}

pub(super) fn insert(
    table: &mut HashMap<usize, u32>,
    pointer: usize,
    id: u32,
    limit: usize,
    cancellation: &Cancellation,
    budget: &mut MaintenanceBudget<'_>,
) -> Result<()> {
    let capacity_bound = limit.checked_mul(2).ok_or(())?.max(4);
    if cancellation.is_cancelled() || table.len() >= limit || table.capacity() > capacity_bound {
        return Err(());
    }
    if !budget.charge(hash_work(1)) {
        return Err(());
    }
    if table.len() == table.capacity() {
        let target = table.capacity().checked_mul(2).ok_or(())?.max(4).min(limit);
        let new_bound = target.checked_mul(2).ok_or(())?.max(4);
        // Standard reserve promises a lower bound only. Prepay our supported
        // growth policy before reserve, then reject an unexpected capacity.
        // Rehash has expected/amortized work; full pointer Eq is authoritative.
        if !budget.charge(hash_growth_work(table.capacity(), new_bound))
            || cancellation.is_cancelled()
        {
            return Err(());
        }
        table
            .try_reserve(target.checked_sub(table.len()).ok_or(())?)
            .map_err(|_| ())?;
        if table.capacity() > new_bound || table.capacity() > capacity_bound {
            return Err(());
        }
    }
    if cancellation.is_cancelled() || table.len() >= table.capacity() {
        return Err(());
    }
    table.insert(pointer, id);
    Ok(())
}
