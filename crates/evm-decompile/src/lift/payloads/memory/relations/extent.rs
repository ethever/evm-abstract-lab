//! A checked upper bound for every destination written since empty memory.
//! The bound may exceed the allocation pointer. Unknown destinations or widths
//! discard it; pointer conventions and ABI padding never establish zero bytes.
use super::{Block, EntityId, Expr, Flow, MAX_TERMS, Range, Source, State, U256, ValueKind};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(super) struct Extent {
    pub initial: Expr,
    pub step: U256,
    pub range: Range,
}

impl State {
    pub(super) fn le(&self, a: &Expr, b: &Expr) -> bool {
        let (Some(ar), Some(br)) = (self.ordinary(a), self.ordinary(b)) else {
            return false;
        };
        if ar.high <= br.low {
            return true;
        }
        let half = U256::MAX >> 1;
        ar.high <= half
            && br.high <= half
            && b.difference(a)
                .and_then(|delta| self.ordinary(&delta))
                .is_some_and(|delta| delta.high <= half)
    }

    pub(super) fn written(&mut self, range: Option<&(Expr, Expr)>) {
        self.extent = self.extent.take().and_then(|extent| {
            let (offset, width) = range?;
            let size = self.ordinary(width)?;
            if size.high == U256::ZERO {
                return Some(extent);
            }
            self.ordinary(offset)?.high.checked_add(size.high)?;
            let end = offset.add(width);
            if self.le(&extent, &end) {
                Some(Box::new(end))
            } else if self.le(&end, &extent) {
                Some(extent)
            } else {
                None
            }
        });
    }
}

impl<P: FnMut(usize) -> bool> Source<'_, P> {
    pub(super) fn remember_extent(
        &mut self,
        state: &State,
        observations: &mut Vec<(EntityId, Expr, Range)>,
    ) -> bool {
        let Some((memory, expr)) = state.memory.zip(state.extent.as_deref()) else {
            return true;
        };
        let Some(range) = state.ordinary(expr) else {
            return true;
        };
        if !self.charge(MAX_TERMS * MAX_TERMS * 2) {
            return false;
        }
        observations.push((memory, expr.clone(), range));
        true
    }

    pub(super) fn empty_extent(&self, block: &Block) -> Option<Box<Expr>> {
        let initial = match self.values.get(&block.memory_parameter).map(|v| &v.kind) {
            Some(ValueKind::Memory { operation: None }) => true,
            Some(ValueKind::MemoryParameter { block: owner }) => *owner == block.id,
            _ => false,
        };
        (self.valid_cfg
            && Some(block.id) == self.ir.entry
            && block.source.is_some_and(|span| span.pc == 0)
            && self.incoming.get(&block.id).is_none_or(Vec::is_empty)
            && initial)
            .then(|| Box::new(Expr::constant(U256::ZERO)))
    }

    /// Candidate templates come from already proposed scalar/cell recurrences.
    /// Their slack is an upper bound, not an asserted exact memory extent.
    /// Admission separately checks the true entry and every source backedge.
    pub(super) fn propose_extent<'a>(
        &mut self,
        initial: &State,
        anchors: impl Iterator<Item = (&'a Expr, U256)>,
        backedges: &[EntityId],
        first: &BTreeMap<EntityId, Flow>,
    ) -> Option<Extent> {
        let extent = initial.extent.as_ref()?;
        let mut best: Option<((bool, U256, U256), Extent)> = None;
        for (anchor, step) in anchors {
            if !self.charge(
                backedges
                    .len()
                    .saturating_add(1)
                    .saturating_mul(MAX_TERMS * MAX_TERMS * 4),
            ) {
                return None;
            }
            if step > (U256::MAX >> 1) {
                continue;
            }
            let slack = |state: &State, extent: &Expr, anchor: &Expr| {
                if state.le(extent, anchor) {
                    Some(U256::ZERO)
                } else {
                    extent
                        .difference(anchor)
                        .and_then(|delta| state.ordinary(&delta))
                        .map(|delta| delta.high)
                }
            };
            let mut required = slack(initial, extent, anchor);
            let next = anchor.add(&Expr::constant(step));
            for edge in backedges {
                match first.get(edge) {
                    Some(Flow::Dead) => {}
                    Some(Flow::Known(state)) => {
                        required = required
                            .zip(state.extent.as_ref().and_then(|e| slack(state, e, &next)))
                            .map(|(a, b)| a.max(b));
                    }
                    _ => required = None,
                }
            }
            let Some(required) = required.filter(|s| *s <= (U256::MAX >> 1)) else {
                continue;
            };
            let initial_expr = anchor.add(&Expr::constant(required));
            let Some(range) = initial.ordinary(&initial_expr) else {
                continue;
            };
            // A large literal can dominate one unrolled body with no slack
            // while losing all useful pointer relations. Prefer a growing,
            // tightly bounded source recurrence; admission checks it afresh.
            let rank = (step == U256::ZERO, range.high, required);
            if best.as_ref().is_none_or(|(old, _)| rank < *old) {
                best = Some((
                    rank,
                    Extent {
                        initial: initial_expr,
                        step,
                        range,
                    },
                ));
            }
        }
        best.map(|(_, extent)| extent)
    }
}

/// At a content join, an incoming bound can be replaced by a fixed cell only
/// after each predecessor proves that its own bound lies below its own cell.
/// The ordinary cell merge then supplies the common, possibly fresh identity.
pub(super) fn join_anchors(a: &State, b: &State) -> Vec<Expr> {
    a.cells
        .iter()
        .filter_map(|(address, value)| {
            let other = b.cells.get(address)?;
            (a.extent.as_ref().is_some_and(|e| a.le(e, value))
                && b.extent.as_ref().is_some_and(|e| b.le(e, other)))
            .then(|| address.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests;
