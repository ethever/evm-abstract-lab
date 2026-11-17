//! Paid boundary between forward stack bounds and native numeric carriers.
use super::{Bounds, Span};
use crate::{
    EntityId, Progress,
    recover::{
        Recovery,
        state::{AbstractValue, Cell, State},
    },
};
use evm_domain::{EvmNumericAlgebra, NumericValue, SummaryConstruction};

impl<P: FnMut(Progress)> Recovery<'_, P> {
    pub(in crate::recover) fn bounds_stack(&mut self, state: &mut State, opcode: u8) -> Option<()> {
        if !self.owner.pay(
            state
                .bounds
                .work_size()
                .saturating_mul(
                    analysis_kernel::cost::tree_work(state.bounds.pairs.len()).saturating_add(1),
                )
                .saturating_mul(8),
        ) {
            return None;
        }
        let len = state.stack.len();
        if (0x80..=0x8f).contains(&opcode) {
            state.bounds.duplicate(len - usize::from(opcode - 0x7f));
        } else {
            state
                .bounds
                .swap(len - 1, len - usize::from(opcode - 0x8f) - 1);
        }
        Some(())
    }
    pub(in crate::recover) fn bounds_step(
        &mut self,
        state: &mut State,
        opcode: u8,
        args: &[Cell],
        outputs: usize,
    ) -> Option<()> {
        if !self.owner.pay(
            state
                .bounds
                .work_size()
                .saturating_mul(
                    analysis_kernel::cost::tree_work(state.bounds.pairs.len()).saturating_add(1),
                )
                .saturating_mul(8),
        ) {
            return None;
        }
        let mut singletons = Vec::new();
        if opcode == 0x01 {
            if !self.owner.pay(args.iter().fold(0usize, |n, c| {
                n.saturating_add(c.abstract_value.singleton_work())
            })) {
                return None;
            }
            singletons = args.iter().map(|c| c.abstract_value.singleton()).collect();
        }
        if outputs != 0 {
            let range = state
                .stack
                .last()
                .and_then(|cell| cell.abstract_value.identity().input())
                .and_then(|input| state.bounds.inputs.get(input))
                .copied();
            if let Some(range) = range {
                self.refine_bound_value(
                    &mut state.stack.last_mut().expect("output").abstract_value,
                    range,
                )?;
            }
        }
        let output = (outputs != 0).then(|| state.stack.last().expect("operation output"));
        state.bounds.step(opcode, args, &singletons, output);
        Some(())
    }
    pub(in crate::recover) fn reduce_bounds(&mut self, state: &mut State) -> Option<()> {
        let size = state.stack.len();
        for (range, cell) in state.bounds.ranges.iter_mut().zip(&state.stack) {
            let numeric = Span::of(&cell.abstract_value);
            range.low = range.low.max(numeric.low);
            range.high = range.high.min(numeric.high);
        }
        for _ in 0..size {
            if !self.owner.pay(
                state
                    .bounds
                    .work_size()
                    .saturating_add(size)
                    .saturating_mul(4),
            ) {
                return None;
            }
            let mut changed = false;
            for (&(left, right, a), &b) in &state.bounds.pairs {
                let high = a
                    .checked_mul(state.bounds.ranges[right].high)
                    .and_then(|v| v.checked_add(b));
                if let Some(high) = high {
                    let old = &mut state.bounds.ranges[left].high;
                    if high < *old {
                        *old = high;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (range, cell) in state.bounds.ranges.iter().zip(&mut state.stack) {
            if range.low > range.high {
                continue;
            }
            let previous = Span::of(&cell.abstract_value);
            if *range == previous {
                continue;
            }
            self.refine_bound_value(&mut cell.abstract_value, *range)?;
        }
        Some(())
    }
    fn refine_bound_value(&mut self, value: &mut AbstractValue, range: Span) -> Option<()> {
        if range.low > range.high {
            return Some(());
        }
        let current = Span::of(value);
        if range.low <= current.low && current.high <= range.high {
            return Some(());
        }
        if !self
            .owner
            .pay(self.algebra.numeric_algebra().summary_construction_work(
                SummaryConstruction::UnsignedRange {
                    lower: range.low,
                    upper: range.high,
                },
            ))
        {
            return None;
        }
        let bounds = NumericValue::unsigned_range(range.low, range.high).expect("ordered range");
        let metadata = bounds
            .work_size()
            .saturating_mul(8)
            .saturating_add(value.numeric().work_size());
        let result = self
            .algebra
            .borrowed()
            .intersect_numeric_with_payment(value.numeric(), &bounds, metadata, |work| {
                self.owner.pay(work).then_some(()).ok_or(())
            })
            .ok()?;
        // The final paid native step may itself publish a cancellation callback.
        if !self.owner.pay(0) {
            return None;
        }
        if let Some(numeric) = result {
            value.replace_numeric_after_singleton(numeric, None);
        }
        Some(())
    }
    pub(in crate::recover) fn assume_bounds(
        &mut self,
        state: &mut State,
        condition: EntityId,
        nonzero: bool,
    ) -> Option<()> {
        if !self.owner.pay(
            state
                .bounds
                .work_size()
                .saturating_add(state.stack.len())
                .saturating_mul(8),
        ) {
            return None;
        }
        state.bounds.assume(&state.stack, condition, nonzero);
        self.reduce_bounds(state)
    }
    pub(in crate::recover) fn join_bounds(
        &mut self,
        old: &Bounds,
        old_stack: &[Cell],
        new: &State,
        joined: &mut State,
        widen: bool,
        feedback: bool,
    ) -> Option<()> {
        let size = joined.stack.len();
        // Eligibility is structural: no native singleton query is needed to
        // decide whether this join will actually discover a coefficient.
        if !self.owner.pay(size.saturating_mul(8).saturating_add(1)) {
            return None;
        }
        let lefts: Vec<_> = if feedback && !widen {
            joined
                .stack
                .iter()
                .enumerate()
                .filter_map(|(slot, cell)| {
                    cell.abstract_value
                        .constants()
                        .is_some_and(|values| values.len() > 1)
                        .then_some(slot)
                })
                .collect()
        } else {
            Vec::new()
        };
        let discover = !lefts.is_empty();
        let eligible: Vec<_> = if discover {
            joined
                .stack
                .iter()
                .map(|cell| {
                    cell.abstract_value.numeric().interval().unsigned_bounds().1 != crate::U256::MAX
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut singletons = Vec::new();
        if discover {
            let native =
                joined
                    .stack
                    .iter()
                    .zip(&eligible)
                    .fold(0usize, |work, (cell, eligible)| {
                        work.saturating_add(if *eligible {
                            cell.abstract_value.singleton_work()
                        } else {
                            0
                        })
                    });
            if !self.owner.pay(native) {
                return None;
            }
            singletons = joined
                .stack
                .iter()
                .zip(&eligible)
                .map(|(cell, eligible)| {
                    if *eligible {
                        cell.abstract_value.singleton()
                    } else {
                        None
                    }
                })
                .collect();
        }
        let available = eligible
            .iter()
            .zip(&singletons)
            .filter(|(eligible, singleton)| **eligible && singleton.is_none())
            .count();
        let candidates = lefts.iter().fold(0usize, |count, left| {
            let self_pair = usize::from(eligible[*left] && singletons[*left].is_none());
            count.saturating_add(available.saturating_sub(self_pair))
        });
        let enumeration = lefts.iter().fold(0usize, |work, slot| {
            work.saturating_add(
                joined.stack[*slot]
                    .abstract_value
                    .constants()
                    .map_or(0, |values| values.len()),
            )
            .saturating_add(size.saturating_mul(12))
        });
        let pairs = old.pairs.len().saturating_add(new.bounds.pairs.len());
        let maximum = pairs
            .saturating_add(candidates)
            .saturating_add(old.inputs.len());
        let lookup = analysis_kernel::cost::key_lookup_work(maximum, 5);
        let insertion = analysis_kernel::cost::key_insert_work(maximum, 5, 5);
        let topology = pairs
            .saturating_mul(lookup.saturating_mul(3).saturating_add(insertion))
            .saturating_add(candidates.saturating_mul(lookup.saturating_add(insertion)))
            .saturating_add(
                old.inputs.len().saturating_mul(
                    analysis_kernel::cost::key_lookup_work(new.bounds.inputs.len(), 8)
                        .saturating_add(analysis_kernel::cost::key_insert_work(maximum, 8, 2)),
                ),
            );
        if !self.owner.pay(
            old.work_size()
                .saturating_add(new.bounds.work_size())
                .saturating_add(size.saturating_mul(8))
                .saturating_mul(4)
                .saturating_add(enumeration)
                .saturating_add(topology),
        ) {
            return None;
        }
        joined.bounds = Bounds::joined(
            (old, old_stack),
            (&new.bounds, &new.stack),
            &joined.stack,
            &singletons,
            widen,
            discover,
        );
        self.reduce_bounds(joined)
    }
}
