//! Forward must inequalities over the current executable stack, never wire facts.
//! Each relation is established by numeric bounds or a taken comparison. Raw
//! stack/arithmetic transfers preserve it; joins require both incoming states.
use super::state::{AbstractValue, Cell};
use crate::{EntityId, U256};
use evm_domain::identity::InputIdentity;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;
mod transfer;

type Key = (usize, usize, U256);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    low: U256,
    high: U256,
}
impl Span {
    fn of(value: &AbstractValue) -> Self {
        let (low, high) = value.numeric().interval().unsigned_bounds();
        Self { low, high }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Comparison {
    opcode: u8,
    left: EntityId,
    right: EntityId,
    inverted: bool,
    left_range: Span,
    right_range: Span,
    left_input: Option<InputIdentity>,
    right_input: Option<InputIdentity>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Bounds {
    ranges: Vec<Span>,
    // Ordinary mathematical left <= coefficient * right + intercept.
    pairs: BTreeMap<Key, U256>,
    predicates: BTreeMap<EntityId, Comparison>,
    inputs: BTreeMap<InputIdentity, Span>,
}
impl Bounds {
    pub(super) fn work_size(&self) -> usize {
        self.ranges
            .len()
            .saturating_mul(2)
            .saturating_add(self.pairs.len().saturating_mul(5))
            .saturating_add(self.predicates.len().saturating_mul(20))
            .saturating_add(self.inputs.len().saturating_mul(8))
            .saturating_add(1)
    }
    pub(super) fn forget_predicates(&mut self) {
        self.predicates.clear();
    }
    fn span(&self, stack: &[Cell], slot: usize) -> Span {
        self.ranges
            .get(slot)
            .copied()
            .unwrap_or_else(|| Span::of(&stack[slot].abstract_value))
    }
    fn insert(&mut self, key: Key, intercept: U256) {
        if key.0 != key.1 {
            self.pairs
                .entry(key)
                .and_modify(|old| *old = (*old).min(intercept))
                .or_insert(intercept);
        }
    }
    fn numeric_intercept(&self, stack: &[Cell], key: Key) -> U256 {
        let left = self.span(stack, key.0).high;
        let right = self.span(stack, key.1).low;
        // Overflow makes the mathematical RHS larger than every word.
        key.2
            .checked_mul(right)
            .map_or(U256::ZERO, |product| left.saturating_sub(product))
    }
    fn implied(&self, stack: &[Cell], key: Key) -> U256 {
        self.pairs
            .get(&key)
            .copied()
            .unwrap_or_else(|| self.numeric_intercept(stack, key))
    }
    pub(super) fn joined(
        old: (&Self, &[Cell]),
        new: (&Self, &[Cell]),
        joined: &[Cell],
        singletons: &[Option<U256>],
        widen: bool,
        discover: bool,
    ) -> Self {
        let (old, old_stack) = old;
        let (new, new_stack) = new;
        let mut result = Self::default();
        for slot in 0..joined.len() {
            let a = old.span(old_stack, slot);
            let b = new.span(new_stack, slot);
            result.ranges.push(Span {
                low: if widen && b.low < a.low {
                    U256::ZERO
                } else {
                    a.low.min(b.low)
                },
                high: if widen && b.high > a.high {
                    U256::MAX
                } else {
                    a.high.max(b.high)
                },
            });
        }
        // Only common, scoped immutable inputs survive an entry join. No
        // runtime definition or expression resemblance establishes this key.
        for (key, a) in &old.inputs {
            let Some(b) = new.inputs.get(key) else {
                continue;
            };
            let range = Span {
                low: if widen && b.low < a.low {
                    U256::ZERO
                } else {
                    a.low.min(b.low)
                },
                high: if widen && b.high > a.high {
                    U256::MAX
                } else {
                    a.high.max(b.high)
                },
            };
            if range.low != U256::ZERO || range.high != U256::MAX {
                result.inputs.insert(*key, range);
            }
        }
        for key in old.pairs.keys().chain(new.pairs.keys()) {
            let previous = old.pairs.get(key).copied();
            // Widening never recreates a key discarded on an earlier trip.
            if widen && previous.is_none() {
                continue;
            }
            let a = previous.unwrap_or_else(|| old.numeric_intercept(old_stack, *key));
            let b = new.implied(new_stack, *key);
            let intercept = a.max(b);
            if widen && previous.is_some_and(|before| intercept > before) {
                continue;
            }
            result.insert(*key, intercept);
        }
        if discover && !widen {
            // A stride is only a candidate coefficient. Admission below proves
            // the inequality from the actual joined input bounds, independently
            // of any prediction about later loop iterations.
            for (left, cell) in joined.iter().enumerate() {
                let Some(values) = cell.abstract_value.constants().filter(|s| s.len() > 1) else {
                    continue;
                };
                let mut previous = None;
                let mut stride = U256::MAX;
                for value in values {
                    if let Some(before) = previous {
                        stride = stride.min(*value - before);
                    }
                    previous = Some(*value);
                }
                if stride == U256::ZERO || stride == U256::MAX {
                    continue;
                }
                for (right, other) in joined.iter().enumerate() {
                    if left == right || singletons[right].is_some() {
                        continue;
                    }
                    let (low, high) = other.abstract_value.numeric().interval().unsigned_bounds();
                    if high == U256::MAX {
                        continue;
                    }
                    let needed = cell
                        .abstract_value
                        .numeric()
                        .interval()
                        .unsigned_bounds()
                        .1
                        .saturating_sub(stride.checked_mul(low).unwrap_or(U256::MAX));
                    // One coefficient of initial slack survives C advancing
                    // before P, without requiring a signed intercept carrier.
                    result.insert((left, right, stride), needed.max(stride));
                }
            }
        }
        result
    }
    fn duplicate(&mut self, slot: usize) {
        let next = self.ranges.len();
        self.ranges.push(self.ranges[slot]);
        let old = self.pairs.clone();
        for ((left, right, a), b) in old {
            if left == slot {
                self.insert((next, right, a), b);
            }
            if right == slot {
                self.insert((left, next, a), b);
            }
        }
    }
    fn swap(&mut self, left: usize, right: usize) {
        self.ranges.swap(left, right);
        let rename = |i| {
            if i == left {
                right
            } else if i == right {
                left
            } else {
                i
            }
        };
        self.pairs = std::mem::take(&mut self.pairs)
            .into_iter()
            .map(|((l, r, a), b)| ((rename(l), rename(r), a), b))
            .collect();
    }
    fn step(
        &mut self,
        opcode: u8,
        args: &[Cell],
        singletons: &[Option<U256>],
        output: Option<&Cell>,
    ) {
        let before = self.ranges.len();
        let retained = before - args.len();
        let old = std::mem::take(&mut self.pairs);
        for (&(left, right, a), &b) in &old {
            if left < retained && right < retained {
                self.insert((left, right, a), b);
            }
        }
        let mut range = output.map(|v| Span::of(&v.abstract_value));
        if opcode == 0x01 && output.is_some() {
            let upper = args[0]
                .abstract_value
                .numeric()
                .interval()
                .unsigned_bounds()
                .1
                .checked_add(
                    args[1]
                        .abstract_value
                        .numeric()
                        .interval()
                        .unsigned_bounds()
                        .1,
                );
            if upper.is_some() {
                for variable in 0..2 {
                    let Some(increment) = singletons[1 - variable] else {
                        continue;
                    };
                    let slot = before - 1 - variable;
                    if let Some(ref mut range) = range
                        && let Some(low) = self.ranges[slot].low.checked_add(increment)
                    {
                        range.low = range.low.max(low);
                    }
                    for (&(left, right, a), &b) in &old {
                        if left == slot
                            && right < retained
                            && let Some(b) = b.checked_add(increment)
                        {
                            self.insert((retained, right, a), b);
                        }
                        if right == slot
                            && left < retained
                            && let Some(delta) = a.checked_mul(increment)
                        {
                            self.insert((left, retained, a), b.saturating_sub(delta));
                        }
                    }
                }
            }
        }
        if let Some(output) = output {
            let predicate = match opcode {
                0x10 | 0x11 => Some(Comparison {
                    opcode,
                    left: args[0].value,
                    right: args[1].value,
                    inverted: false,
                    left_range: Span::of(&args[0].abstract_value),
                    right_range: Span::of(&args[1].abstract_value),
                    left_input: args[0].abstract_value.identity().input().copied(),
                    right_input: args[1].abstract_value.identity().input().copied(),
                }),
                0x15 => self.predicates.get(&args[0].value).cloned().map(|mut p| {
                    p.inverted = !p.inverted;
                    p
                }),
                _ => None,
            };
            if let Some(predicate) = predicate {
                self.predicates.insert(output.value, predicate);
            }
        }
        self.ranges.truncate(retained);
        if let Some(range) = range {
            self.ranges.push(range);
        }
    }
    fn remember_input(&mut self, input: Option<InputIdentity>, range: Span) {
        let Some(input) = input else { return };
        if range.low > range.high || (range.low == U256::ZERO && range.high == U256::MAX) {
            return;
        }
        self.inputs
            .entry(input)
            .and_modify(|old| {
                let low = old.low.max(range.low);
                let high = old.high.min(range.high);
                if low <= high {
                    *old = Span { low, high };
                }
            })
            .or_insert(range);
    }
    fn assume(&mut self, stack: &[Cell], condition: EntityId, nonzero: bool) {
        let Some(p) = self.predicates.get(&condition).cloned() else {
            return;
        };
        let positive = nonzero != p.inverted;
        let (left, right, lr, rr, li, ri, strict) = match (p.opcode, positive) {
            (0x10, true) => (
                p.left,
                p.right,
                p.left_range,
                p.right_range,
                p.left_input,
                p.right_input,
                true,
            ),
            (0x10, false) => (
                p.right,
                p.left,
                p.right_range,
                p.left_range,
                p.right_input,
                p.left_input,
                false,
            ),
            (0x11, true) => (
                p.right,
                p.left,
                p.right_range,
                p.left_range,
                p.right_input,
                p.left_input,
                true,
            ),
            (0x11, false) => (
                p.left,
                p.right,
                p.left_range,
                p.right_range,
                p.left_input,
                p.right_input,
                false,
            ),
            _ => return,
        };
        let distance = U256::from(u64::from(strict));
        let high = rr.high.checked_sub(distance).map(|h| h.min(lr.high));
        let low = lr.low.checked_add(distance).map(|l| l.max(rr.low));
        if let Some(high) = high {
            self.remember_input(li, Span { low: lr.low, high });
        }
        if let Some(low) = low {
            self.remember_input(ri, Span { low, high: rr.high });
        }
        let lefts: Vec<_> = stack
            .iter()
            .enumerate()
            .filter_map(|(i, c)| (c.value == left).then_some(i))
            .collect();
        let rights: Vec<_> = stack
            .iter()
            .enumerate()
            .filter_map(|(i, c)| (c.value == right).then_some(i))
            .collect();
        for &l in &lefts {
            if let Some(high) = high {
                self.ranges[l].high = self.ranges[l].high.min(high);
            }
        }
        for &r in &rights {
            if let Some(low) = low {
                self.ranges[r].low = self.ranges[r].low.max(low);
            }
        }
        // A canonical pair avoids Cartesian growth across surviving aliases.
        if let (Some(&l), Some(&r)) = (lefts.first(), rights.first()) {
            self.insert((l, r, U256::from(1)), U256::ZERO);
        }
    }
}
