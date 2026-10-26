//! Removable FIFO obligations containing only live pending identities.
//!
//! Nodes are dense, with FIFO links independent of their storage positions. A
//! removal unlinks its node and moves the last node into the gap, updating that
//! node's index and links. No satisfied occurrence survives to be scanned or
//! reactivated. Enqueue/removal take O(log live) map work; selection takes O(live)
//! visits plus removal. Spare vector capacity follows peak concurrent live work,
//! never the number of historical enqueues. Snapshots copy only live nodes.
//!
//! The owner reserves [`Worklist::enqueue_work`] before enqueueing. Kernel fanout
//! reserves a batch bound including geometric vector growth before any wake.

use crate::{KernelError, Meter, cost, kernel::reserve};
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Node<K> {
    id: K,
    previous: Option<usize>,
    next: Option<usize>,
}

/// One removable FIFO node per live obligation, independent of semantic values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worklist<K: Copy + Ord> {
    nodes: Vec<Node<K>>,
    positions: BTreeMap<K, usize>,
    head: Option<usize>,
    tail: Option<usize>,
}
impl<K: Copy + Ord> Default for Worklist<K> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            positions: BTreeMap::new(),
            head: None,
            tail: None,
        }
    }
}
impl<K: Copy + Ord> Worklist<K> {
    /// Conservative lookup, insertion, link and possible vector growth work.
    pub fn enqueue_work(&self) -> usize {
        self.enqueue_work_for(1)
    }

    pub(crate) fn enqueue_work_for(&self, additional: usize) -> usize {
        cost::enqueue_work(self.len(), self.nodes.capacity(), additional)
    }

    /// Enqueue after the caller reserves `enqueue_work`; duplicates keep position.
    pub fn enqueue(&mut self, id: K) {
        let Entry::Vacant(position) = self.positions.entry(id) else {
            return;
        };
        if self.nodes.len() == self.nodes.capacity() {
            self.nodes.reserve_exact(self.nodes.len().max(1));
        }
        let index = self.nodes.len();
        position.insert(index);
        self.nodes.push(Node {
            id,
            previous: self.tail,
            next: None,
        });
        if let Some(tail) = self.tail {
            self.nodes[tail].next = Some(index);
        } else {
            self.head = Some(index);
        }
        self.tail = Some(index);
    }

    /// Conservative map removal, moved-node lookup and constant link repairs.
    pub(crate) fn removal_work(&self) -> usize {
        cost::removal_work(self.len())
    }

    /// Remove an obligation immediately; a later enqueue receives a fresh tail.
    pub fn satisfy(&mut self, id: K) {
        let Some(index) = self.positions.remove(&id) else {
            return;
        };
        let node = &self.nodes[index];
        let (previous, next) = (node.previous, node.next);
        if let Some(previous) = previous {
            self.nodes[previous].next = next;
        } else {
            self.head = next;
        }
        if let Some(next) = next {
            self.nodes[next].previous = previous;
        } else {
            self.tail = previous;
        }
        self.nodes.swap_remove(index);
        if index < self.nodes.len() {
            let moved = &self.nodes[index];
            let (previous, next) = (moved.previous, moved.next);
            *self
                .positions
                .get_mut(&moved.id)
                .expect("live node position") = index;
            if let Some(previous) = previous {
                self.nodes[previous].next = Some(index);
            } else {
                self.head = Some(index);
            }
            if let Some(next) = next {
                self.nodes[next].previous = Some(index);
            } else {
                self.tail = Some(index);
            }
        }
    }
    /// Whether this identity has an unsatisfied obligation.
    pub fn contains(&self, id: &K) -> bool {
        self.positions.contains_key(id)
    }
    /// Whether any live obligation remains.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// Live obligation count without traversal.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// Sorted live identities, useful for parking incomplete frontiers.
    pub fn pending(&self) -> impl Iterator<Item = K> + '_ {
        self.positions.keys().copied()
    }
    /// Live identities in FIFO order, including temporarily in-flight jobs.
    pub fn in_order(&self) -> impl Iterator<Item = K> + '_ {
        std::iter::successors(self.head, |index| self.nodes[*index].next)
            .map(|index| self.nodes[index].id)
    }
    /// Live nodes and lookup entries for adapter-owned copy/drop accounting.
    pub fn work_size(&self) -> usize {
        self.nodes
            .len()
            .saturating_mul(3)
            .saturating_add(self.positions.len().saturating_mul(2))
    }
    /// Physical initialized records; always equal to the live obligation count.
    pub fn records(&self) -> usize {
        self.nodes.len()
    }
    /// Pop FIFO after adapter admission and removal-work reservation.
    pub fn pop_front(&mut self) -> Option<K> {
        let id = self.nodes.get(self.head?)?.id;
        self.satisfy(id);
        Some(id)
    }
    /// Metered FIFO selection excluding temporarily in-flight identities.
    ///
    /// Reserve at most one visit per live node plus map/link removal. Selection
    /// and all cancellation checks precede mutation, including a final check
    /// after the eligibility callback. Skipped self-wakes keep their FIFO place.
    pub fn pop_eligible<M: Meter>(
        &mut self,
        meter: &mut M,
        eligible: impl Fn(K) -> bool,
    ) -> Result<Option<K>, KernelError<M::Stop>> {
        if self.is_empty() {
            return Ok(None);
        }
        let cost = cost::selection_work(self.len());
        reserve(meter, cost)?;
        let mut choice = None;
        for id in self.in_order() {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            if eligible(id) {
                choice = Some(id);
                break;
            }
        }
        meter.checkpoint().map_err(KernelError::Stopped)?;
        if let Some(id) = choice {
            self.satisfy(id);
        }
        Ok(choice)
    }
    /// Select a positive deepest priority with FIFO ties.
    ///
    /// Reserve two visits per live node plus map/link removal, independent of
    /// satisfied history. All-zero priorities return None. Every cancellation
    /// check, including the one after the final callback, precedes mutation.
    pub fn pop_deepest<M: Meter>(
        &mut self,
        meter: &mut M,
        depth: impl Fn(K) -> usize,
    ) -> Result<Option<K>, KernelError<M::Stop>> {
        if self.is_empty() {
            return Ok(None);
        }
        let cost = cost::priority_selection_work(self.len());
        reserve(meter, cost)?;
        let mut choice = None;
        let mut maximum = 0;
        for id in self.in_order() {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            let priority = depth(id);
            if priority > maximum {
                maximum = priority;
                choice = Some(id);
            }
        }
        meter.checkpoint().map_err(KernelError::Stopped)?;
        if let Some(id) = choice {
            self.satisfy(id);
        }
        Ok(choice)
    }
}
impl<K: Copy + Ord> FromIterator<K> for Worklist<K> {
    /// The owner accounts live records when adopting this low-level queue.
    fn from_iter<T: IntoIterator<Item = K>>(iter: T) -> Self {
        let mut result = Self::default();
        for id in iter {
            result.enqueue(id);
        }
        result
    }
}

#[cfg(test)]
#[path = "tests/queue_cost.rs"]
mod tests;
