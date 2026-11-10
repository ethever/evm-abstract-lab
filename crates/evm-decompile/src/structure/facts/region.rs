//! Normal joins before explicit source boundaries, without a global termination
//! claim. Each natural-loop entry is a boundary before executing that loop; its
//! complete body remains the planner's responsibility. Distinct boundaries keep
//! distinct source identities, including a stop that itself eventually fails.
use super::{Facts, Loop};
use crate::{EntityId, ExitKind, Ir, Terminator};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::structure) struct RegionScope {
    pub continuation: Option<EntityId>,
    pub loop_header: Option<EntityId>,
}

pub(in crate::structure) struct NormalRegion {
    pub postdom: BTreeMap<EntityId, BTreeSet<EntityId>>,
}

impl NormalRegion {
    fn empty() -> Self {
        Self {
            postdom: BTreeMap::new(),
        }
    }
}

impl Facts {
    pub(in crate::structure) fn normal_region(
        &self,
        ir: &Ir,
        scope: RegionScope,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<NormalRegion> {
        // Vectors and the reverse-closure queues are paid before allocation.
        // One scoped result is retained by the planner; its source-set storage
        // is additionally bounded below, independently of plan-arena rollback.
        if !pay(self
            .blocks
            .len()
            .saturating_add(self.edges.len())
            .saturating_mul(32))
        {
            return None;
        }
        let active = scope.loop_header.and_then(|id| self.loops.get(&id));
        if scope.loop_header.is_some() && active.is_none() {
            return Some(NormalRegion::empty());
        }
        let inside = |id: EntityId| active.is_none_or(|lp| lp.nodes.contains(&id));
        let boundary = |id: EntityId| {
            Some(id) == scope.continuation
                || self.loops.contains_key(&id)
                || active.is_some_and(|lp: &Loop| lp.exit == Some(id))
        };
        let included: Vec<_> = ir
            .blocks
            .iter()
            .map(|block| inside(block.id) || active.is_some_and(|lp| lp.exit == Some(block.id)))
            .collect();
        let boundaries: Vec<_> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| included[index] && boundary(block.id))
            .collect();
        let mut closure = |failed_only: bool| {
            let mut ranks = vec![None; ir.blocks.len()];
            let mut remaining: Vec<_> = self.successors.iter().map(Vec::len).collect();
            let mut largest = vec![0_usize; ir.blocks.len()];
            let mut pending = VecDeque::new();
            for (index, block) in ir.blocks.iter().enumerate() {
                if !included[index] {
                    continue;
                }
                let terminal = if boundaries[index] {
                    !failed_only
                } else {
                    match block.terminator {
                        Terminator::Trap { .. } => true,
                        Terminator::Exit { kind, .. } => !failed_only || kind == ExitKind::Revert,
                        _ => false,
                    }
                };
                if terminal {
                    ranks[index] = Some(0);
                    pending.push_back(index);
                }
            }
            while let Some(done) = pending.pop_front() {
                if !pay(self.predecessors[done].len().saturating_add(1)) {
                    return None;
                }
                for &pred in &self.predecessors[done] {
                    if ranks[pred].is_some()
                        || !included[pred]
                        || boundaries[pred]
                        || !self.known[pred]
                    {
                        continue;
                    }
                    remaining[pred] -= 1;
                    largest[pred] = largest[pred].max(ranks[done]?);
                    if remaining[pred] == 0 {
                        ranks[pred] = Some(largest[pred].saturating_add(1));
                        pending.push_back(pred);
                    }
                }
            }
            Some(ranks)
        };
        // These least closures never turn a pending edge or residual cycle into
        // evidence. Cutting a boundary can only remove actual failure evidence;
        // it cannot classify a continue/break or normal terminal as a failure.
        let ranks = closure(false)?;
        let failed = closure(true)?;
        if !pay(ir.blocks.len().saturating_mul(16)) {
            return None;
        }
        let mut order: Vec<_> = ranks
            .iter()
            .enumerate()
            .filter_map(|(index, rank)| rank.map(|rank| (rank, index)))
            .collect();
        if !pay(order
            .len()
            .saturating_mul(order.len().max(1).ilog2() as usize + 1))
        {
            return None;
        }
        order.sort_unstable();
        let mut result = NormalRegion::empty();
        let mut slots = 0_usize;
        for (_, index) in order {
            if failed[index].is_some() {
                continue;
            }
            let block = &ir.blocks[index];
            let targets = if boundaries[index] {
                &[][..]
            } else {
                self.successors[index].as_slice()
            };
            let mut normal = targets.iter().copied().filter(|to| failed[*to].is_none());
            let mut post = if let Some(first) = normal.next() {
                let first = &result.postdom[&ir.blocks[first].id];
                // Prepay the temporary clone as well as its retained result.
                // Refusing this optional fact leaves the existing proof intact.
                if slots.saturating_add(first.len().saturating_mul(2))
                    > crate::MAX_STRUCTURED_WIRE_NODES
                {
                    return Some(NormalRegion::empty());
                }
                if !pay(first.len().saturating_mul(8).saturating_add(1)) {
                    return None;
                }
                first.clone()
            } else {
                if !pay(8) {
                    return None;
                }
                BTreeSet::from([EntityId(0)])
            };
            for target in normal {
                let other = &result.postdom[&ir.blocks[target].id];
                if !pay(post.len().saturating_add(other.len()).saturating_mul(8)) {
                    return None;
                }
                post.retain(|id| other.contains(id));
            }
            if slots.saturating_add(post.len()).saturating_add(1) > crate::MAX_STRUCTURED_WIRE_NODES
            {
                return Some(NormalRegion::empty());
            }
            if !pay(16) {
                return None;
            }
            post.insert(block.id);
            slots = slots.saturating_add(post.len());
            result.postdom.insert(block.id, post);
        }
        Some(result)
    }
}
