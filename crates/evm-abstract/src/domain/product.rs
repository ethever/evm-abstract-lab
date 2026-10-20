//! Private default numeric primitives. These never call the metadata facade.

use super::{
    Domain, FiniteConstantSet, NumericReduction, NumericValue, Profile, known_bits::KnownBits,
    query,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

pub(super) fn finite(finite: FiniteConstantSet) -> NumericValue {
    let Some(values) = finite.as_values() else {
        return NumericValue::top();
    };
    NumericValue {
        bits: KnownBits::from_values(values),
        interval: super::interval::Interval::from_values(values),
        congruence: super::congruence::Congruence::from_values(values),
        nonzero: !values.contains(&U256::ZERO),
        finite,
    }
}

impl Domain {
    pub(super) fn numeric_project(self, value: &NumericValue) -> NumericValue {
        if self.spec().profile() == Profile::Product {
            let mut out = value.clone();
            out.finite = out.finite.into_limited(self.spec().constant_capacity());
            return out;
        }
        query::candidates(value, self.capacity())
            .filter(|values| !values.is_empty() && values.len() <= self.capacity())
            .map_or_else(NumericValue::top, |values| {
                finite(FiniteConstantSet::try_from_values(values).expect("nonempty candidates"))
            })
    }

    pub(super) fn numeric_join(self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        let candidates = left
            .finite
            .join(&right.finite, self.spec().constant_capacity());
        if self.spec().profile() == Profile::ConstantsOnly {
            return finite(candidates);
        }
        NumericValue {
            finite: candidates,
            bits: left.bits.join(&right.bits),
            interval: left.interval.join(&right.interval),
            congruence: left.congruence.join(&right.congruence),
            nonzero: left.nonzero && right.nonzero,
        }
    }

    pub(super) fn numeric_widen(self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        let mut out = next.clone();
        if self.spec().profile() == Profile::Product {
            out.interval = old.interval.widen(&next.interval);
        }
        out
    }

    pub(super) fn numeric_widen_procedure(
        self,
        old: &NumericValue,
        next: &NumericValue,
    ) -> NumericValue {
        if old == next
            || next.constants().is_some_and(|values| {
                values.len() <= self.spec().widening_after_updates().saturating_add(1)
            })
        {
            return next.clone();
        }
        if self.spec().profile() == Profile::ConstantsOnly {
            return NumericValue::top();
        }
        let previous = old.interval.unsigned_bounds();
        let joined = next.interval.unsigned_bounds();
        if previous == joined {
            return next.clone();
        }
        let mut out = self.numeric_widen(old, next);
        let widened = out.interval.unsigned_bounds();
        if joined != widened {
            let implied = KnownBits::from_unsigned_bounds(joined.0, joined.1);
            let retained = KnownBits::from_unsigned_bounds(widened.0, widened.1);
            let discard = (implied.zero() | implied.one()) & !(retained.zero() | retained.one());
            out.bits = KnownBits::new(next.bits.zero() & !discard, next.bits.one() & !discard)
                .expect("removing guarantees preserves disjoint masks");
            out.finite = FiniteConstantSet::top();
        }
        out
    }

    pub(super) fn numeric_finite_apply(self, op: u8, args: &[NumericValue]) -> NumericValue {
        let collect = |values| {
            finite(
                FiniteConstantSet::collect_bounded(values, self.spec().constant_capacity())
                    .expect("reachable finite values are nonempty"),
            )
        };
        let Some(sets) = args
            .iter()
            .map(NumericValue::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                finite(
                    FiniteConstantSet::collect_bounded(
                        (0u64..=256).map(U256::from),
                        self.spec().constant_capacity(),
                    )
                    .unwrap(),
                )
            } else if matches!(op, 0x10..=0x15) {
                finite(
                    FiniteConstantSet::collect_bounded(
                        [U256::ZERO, U256::ONE],
                        self.spec().constant_capacity(),
                    )
                    .unwrap(),
                )
            } else {
                NumericValue::top()
            };
        };
        let singleton = BTreeSet::from([U256::ZERO]);
        let second = sets.get(1).copied().unwrap_or(&singleton);
        let third = sets.get(2).copied().unwrap_or(&singleton);
        collect(sets[0].iter().flat_map(|a| {
            second
                .iter()
                .flat_map(move |b| third.iter().map(move |c| super::evaluate(op, *a, *b, *c)))
        }))
    }

    pub(super) fn numeric_apply(
        self,
        op: u8,
        args: &[NumericValue],
    ) -> NumericReduction<NumericValue> {
        self.numeric_apply_with(op, args, |value| super::reduce::numeric(self, value))
    }
    pub(super) fn numeric_apply_with(
        self,
        op: u8,
        args: &[NumericValue],
        reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
    ) -> NumericReduction<NumericValue> {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return NumericReduction::stable(NumericValue::top());
        }
        if self.spec().profile() == Profile::ConstantsOnly {
            return NumericReduction::stable(self.numeric_finite_apply(op, args));
        }
        super::transfer::apply(self, op, args, reduce)
    }

    pub(super) fn numeric_projection_work(self, value: &NumericValue) -> usize {
        if self.spec().profile() == Profile::Product {
            return 0;
        }
        query::candidate_visits(value, self.capacity()).saturating_mul(256)
    }

    pub(super) fn numeric_refinement_work(self, value: &NumericValue, facts: usize) -> usize {
        let atoms = self.capacity().saturating_add(32);
        let round = self.numeric_round_work();
        let facts = facts
            .saturating_add(1)
            .min(self.spec().fact_limit().saturating_add(1));
        4096usize
            .saturating_add(round.saturating_mul(self.spec().reduction_rounds()))
            .saturating_add(
                facts
                    .saturating_mul(facts.saturating_add(atoms))
                    .saturating_mul(32),
            )
            .saturating_add(value.work_size())
    }

    pub(super) fn numeric_round_work(self) -> usize {
        // Includes the bounded typed channels, dependency graph and wake fanout.
        16384usize
            .saturating_add(512usize.saturating_mul(self.capacity()))
            .saturating_add(64usize.saturating_mul(self.capacity().saturating_add(32)))
    }

    pub(super) fn numeric_operation_work<'a>(
        self,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        let (tuples, all_finite, size, len) = args.fold(
            (1usize, true, 0usize, 0usize),
            |(tuples, all, size, len), v| {
                (
                    tuples.saturating_mul(v.constants().map_or(1, BTreeSet::len)),
                    all && v.constants().is_some(),
                    size.saturating_add(v.work_size()),
                    len.saturating_add(1),
                )
            },
        );
        if self.spec().profile() == Profile::ConstantsOnly {
            return tuples.saturating_add(size);
        }
        if len > 0 && all_finite && tuples <= self.capacity() {
            return tuples.saturating_mul(256).saturating_add(size);
        }
        tuples
            .saturating_add(4096)
            .saturating_add(
                self.numeric_round_work()
                    .saturating_mul(self.spec().reduction_rounds()),
            )
            .saturating_add(size)
    }
}
