//! Pending fixed-point obligations, independent of node identity and payload joins.
//!
//! Only this owner mutates FIFO records and pending membership. A certificate may
//! satisfy a node without removing its record; a later semantic change requeues
//! it, and its older record keeps its original scheduling position. Retain those
//! records for the engine's scan and replay-copy charges as well.

use super::{FrontierReason, Limit, MachineError};
use crate::resource::WorkBudget;
use std::collections::{BTreeSet, VecDeque};

/// Both fixed points admit a popped node before preparing its payload. Keep
/// cancellation/work exhaustion ahead of the transfer cap when both apply.
pub(super) fn transfer_stop(
    transfers: usize,
    max_transfers: usize,
    budget: &WorkBudget,
) -> Option<FrontierReason> {
    if transfers >= max_transfers || budget.exhausted() {
        Some(if budget.exhausted() {
            MachineError::work_failure().frontier_reason()
        } else {
            FrontierReason::Budget(Limit::Transfers)
        })
    } else {
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Worklist {
    queue: VecDeque<usize>,
    pending: BTreeSet<usize>,
}

impl Worklist {
    /// Call only for a new node or an input whose semantic bound changed.
    pub(super) fn enqueue(&mut self, id: usize) {
        if self.pending.insert(id) {
            self.queue.push_back(id);
        }
    }

    pub(super) fn satisfy(&mut self, id: usize) {
        self.pending.remove(&id);
    }

    pub(super) fn contains(&self, id: &usize) -> bool {
        self.pending.contains(id)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Sorted node order for parking; partitions with equal public keys stay distinct.
    pub(super) fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        self.pending.iter().copied()
    }

    /// Preserve FIFO occurrences for the engine's interrupted-current frontier.
    pub(super) fn in_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.queue.iter().copied().filter(|id| self.contains(id))
    }

    pub(super) fn work_size(&self) -> usize {
        self.queue.len().saturating_add(self.pending.len())
    }

    pub(super) fn pop_front(&mut self) -> Option<usize> {
        while let Some(id) = self.queue.pop_front() {
            if self.pending.remove(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Complete deeper callees first, keeping FIFO order at each positive depth.
    /// Charge and check cancellation over physical records, including stale ones;
    /// an interrupted selection leaves the entire scheduling state untouched.
    pub(super) fn pop_deepest(
        &mut self,
        budget: &mut WorkBudget,
        depth_of: impl Fn(usize) -> usize,
    ) -> Result<Option<usize>, MachineError> {
        if self.is_empty() {
            return Ok(None);
        }
        let cost = self.queue.len().saturating_mul(2).saturating_add(1);
        if cost == usize::MAX || !budget.charge(cost) {
            return Err(MachineError::work_failure());
        }
        let mut choice = None;
        let mut depth = 0;
        for (position, id) in self.queue.iter().enumerate() {
            MachineError::check_cancelled()?;
            let candidate_depth = depth_of(*id);
            if self.contains(id) && candidate_depth > depth {
                choice = Some(position);
                depth = candidate_depth;
            }
        }
        Ok(choice
            .and_then(|position| self.queue.remove(position))
            .inspect(|id| {
                self.satisfy(*id);
            }))
    }
}

impl FromIterator<usize> for Worklist {
    fn from_iter<T: IntoIterator<Item = usize>>(iter: T) -> Self {
        let mut worklist = Self::default();
        for id in iter {
            worklist.enqueue(id);
        }
        worklist
    }
}

#[cfg(test)]
mod tests;
