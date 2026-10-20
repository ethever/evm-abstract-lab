//! FIFO obligations with retained stale physical records.
//!
//! This low-level owner does not know the adapter's state/transfer accounting.
//! Enqueue callers reserve [`Worklist::enqueue_work`] before allocating records;
//! [`crate::Kernel`] does so for every dependency wake. Physical records remain
//! observable for replay-copy charges and atomic priority selection.

use crate::{KernelError, Meter};
use std::collections::{BTreeSet, VecDeque};

/// Pending membership and FIFO occurrences, independently of semantic values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worklist<K: Copy + Ord> {
    queue: VecDeque<K>,
    pending: BTreeSet<K>,
}
impl<K: Copy + Ord> Default for Worklist<K> {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            pending: BTreeSet::new(),
        }
    }
}
impl<K: Copy + Ord> Worklist<K> {
    /// Conservative logical allocation/search work for one enqueue.
    pub fn enqueue_work(&self) -> usize {
        self.pending.len().saturating_add(1).ilog2() as usize + 3
    }
    /// Enqueue after the caller reserves `enqueue_work`; duplicate jobs do not allocate.
    pub fn enqueue(&mut self, id: K) {
        if self.pending.insert(id) {
            self.queue.push_back(id);
        }
    }
    /// Satisfy an obligation while retaining its stale physical occurrence.
    pub fn satisfy(&mut self, id: K) {
        self.pending.remove(&id);
    }
    /// Whether this identity still has an unsatisfied occurrence.
    pub fn contains(&self, id: &K) -> bool {
        self.pending.contains(id)
    }
    /// Whether any live obligation remains.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    /// Live membership count without scanning the physical records.
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    /// Sorted live identities, useful for parking incomplete frontiers.
    pub fn pending(&self) -> impl Iterator<Item = K> + '_ {
        self.pending.iter().copied()
    }
    /// Preserve physical FIFO order, excluding satisfied records.
    pub fn in_order(&self) -> impl Iterator<Item = K> + '_ {
        self.queue.iter().copied().filter(|id| self.contains(id))
    }
    /// Physical queue plus membership size for adapter-owned copy accounting.
    pub fn work_size(&self) -> usize {
        self.queue.len().saturating_add(self.pending.len())
    }
    /// Physical records, including stale occurrences.
    pub fn records(&self) -> usize {
        self.queue.len()
    }
    /// Pop FIFO after adapter admission; skipped stale records allocate nothing.
    pub fn pop_front(&mut self) -> Option<K> {
        while let Some(id) = self.queue.pop_front() {
            if self.pending.remove(&id) {
                return Some(id);
            }
        }
        None
    }
    /// Metered FIFO selection excluding temporarily in-flight identities.
    /// Earlier physical records are retained: an in-flight self-wake must not
    /// lose its only pending occurrence while another nested job is admitted.
    pub fn pop_eligible<M: Meter>(
        &mut self,
        meter: &mut M,
        eligible: impl Fn(K) -> bool,
    ) -> Result<Option<K>, KernelError<M::Stop>> {
        if self.is_empty() {
            return Ok(None);
        }
        let cost = self
            .queue
            .len()
            .checked_add(1)
            .ok_or(KernelError::CostOverflow)?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        meter.reserve(cost).map_err(KernelError::Stopped)?;
        let mut choice = None;
        for (position, id) in self.queue.iter().enumerate() {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            if self.contains(id) && eligible(*id) {
                choice = Some(position);
                break;
            }
        }
        Ok(choice
            .and_then(|position| self.queue.remove(position))
            .inspect(|id| self.satisfy(*id)))
    }
    /// Select a positive deepest priority with FIFO ties.
    ///
    /// Reserve exactly `2 * physical_records + 1`, including stale records.
    /// Check cancellation over every physical record; any failure leaves the
    /// queue and pending membership unchanged. All-zero priorities return None.
    pub fn pop_deepest<M: Meter>(
        &mut self,
        meter: &mut M,
        depth: impl Fn(K) -> usize,
    ) -> Result<Option<K>, KernelError<M::Stop>> {
        if self.is_empty() {
            return Ok(None);
        }
        let cost = self
            .queue
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(1))
            .ok_or(KernelError::CostOverflow)?;
        meter.reserve(cost).map_err(KernelError::Stopped)?;
        let mut choice = None;
        let mut maximum = 0;
        for (position, id) in self.queue.iter().enumerate() {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            let priority = depth(*id);
            if self.contains(id) && priority > maximum {
                maximum = priority;
                choice = Some(position);
            }
        }
        Ok(choice
            .and_then(|position| self.queue.remove(position))
            .inspect(|id| self.satisfy(*id)))
    }
}
impl<K: Copy + Ord> FromIterator<K> for Worklist<K> {
    /// The owner must account retained records when adopting this low-level queue.
    fn from_iter<T: IntoIterator<Item = K>>(iter: T) -> Self {
        let mut result = Self::default();
        for id in iter {
            result.enqueue(id);
        }
        result
    }
}
