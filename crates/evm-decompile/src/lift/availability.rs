//! Raw definition availability at block entry, with linear graph storage.
//!
//! Reverse-postorder immediate dominators converge under the caller's work
//! ledger. Tree intervals then answer every copy proof without graph searches.
use crate::{EntityId, Ir};
use std::collections::BTreeMap;

pub(super) struct Availability {
    intervals: Vec<Option<(usize, usize)>>,
}

impl Availability {
    pub(super) fn build(
        ir: &Ir,
        blocks: &BTreeMap<EntityId, usize>,
        incoming: &[Vec<usize>],
        outgoing: &[Vec<usize>],
        complete: bool,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<Self> {
        let n = ir.blocks.len();
        if !pay(n.saturating_mul(12).saturating_add(1)) {
            return None;
        }
        let mut intervals = vec![None; n];
        let Some(entry) = ir.entry.and_then(|id| blocks.get(&id).copied()) else {
            return Some(Self { intervals });
        };
        if !complete {
            return Some(Self { intervals });
        }
        let mut seen = vec![false; n];
        let mut pending = vec![(entry, 0)];
        let mut order = Vec::with_capacity(n);
        seen[entry] = true;
        while let Some((block, next)) = pending.last_mut() {
            if !pay(1) {
                return None;
            }
            if let Some(edge) = outgoing[*block].get(*next) {
                *next += 1;
                let target = blocks[&ir.edges[*edge].to];
                if !seen[target] {
                    seen[target] = true;
                    pending.push((target, 0));
                }
            } else {
                order.push(*block);
                pending.pop();
            }
        }
        order.reverse();
        let mut rank = vec![usize::MAX; n];
        for (i, block) in order.iter().enumerate() {
            rank[*block] = i;
        }
        let mut idom = vec![None; n];
        idom[entry] = Some(entry);
        loop {
            let mut changed = false;
            for block in order.iter().copied().skip(1) {
                if !pay(incoming[block].len().saturating_add(1)) {
                    return None;
                }
                let mut parent = None;
                for edge in &incoming[block] {
                    let predecessor = blocks[&ir.edges[*edge].from];
                    if idom[predecessor].is_none() {
                        continue;
                    }
                    parent = Some(match parent {
                        None => predecessor,
                        Some(mut left) => {
                            let mut right = predecessor;
                            while left != right {
                                if !pay(1) {
                                    return None;
                                }
                                if rank[left] > rank[right] {
                                    left = idom[left]?;
                                } else {
                                    right = idom[right]?;
                                }
                            }
                            left
                        }
                    });
                }
                if idom[block] != parent {
                    idom[block] = parent;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut children = vec![Vec::new(); n];
        for block in order.iter().copied().skip(1) {
            children[idom[block]?].push(block);
        }
        let mut clock = 0;
        pending.push((entry, 0));
        intervals[entry] = Some((clock, 0));
        clock += 1;
        while let Some((block, next)) = pending.last_mut() {
            if !pay(1) {
                return None;
            }
            if let Some(child) = children[*block].get(*next).copied() {
                *next += 1;
                intervals[child] = Some((clock, 0));
                clock += 1;
                pending.push((child, 0));
            } else {
                intervals[*block].as_mut()?.1 = clock;
                pending.pop();
            }
        }
        Some(Self { intervals })
    }

    pub(super) fn reachable(&self, block: usize) -> bool {
        self.intervals[block].is_some()
    }

    pub(super) fn strictly_dominates(&self, definition: usize, consumer: usize) -> bool {
        match (self.intervals[definition], self.intervals[consumer]) {
            (Some((start, end)), Some((point, _))) => start < point && point < end,
            _ => false,
        }
    }
}
