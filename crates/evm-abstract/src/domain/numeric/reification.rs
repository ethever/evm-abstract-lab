//! A complete small unsigned envelope of the current numeric product only.

use super::{FiniteConstantSet, NumericValue};
use alloy_primitives::U256;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) struct Enumeration<'a> {
    value: &'a NumericValue,
    first: U256,
    count: usize,
}

impl NumericValue {
    /// Planning only: no candidate queries, filtering, CRT or allocation.
    /// The caller prepays fixed-width planning before calling this helper and
    /// the complete construction/lifetime tariff before building its result.
    pub(crate) fn complete_enumeration(&self, capacity: usize) -> Option<Enumeration<'_>> {
        if !self.finite.is_top() {
            return None;
        }
        let (first, last) = self.interval.unsigned_bounds();
        let span = last.checked_sub(first)?.checked_add(U256::from(1))?;
        let count = usize::try_from(span).ok()?;
        (count <= capacity).then_some(Enumeration {
            value: self,
            first,
            count,
        })
    }
}

impl Enumeration<'_> {
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Reuse the numeric candidate unit and Context join's tree-insertion
    /// tariff. Retained payload/root allocation and drop are paid separately
    /// by the caller's existing reserve_owned, before any candidate is visited.
    pub(crate) fn work(&self) -> usize {
        let height = (usize::BITS - self.count.leading_zeros()) as usize + 1;
        self.count
            .saturating_mul(256)
            .saturating_add(self.count.saturating_mul(height).saturating_mul(256))
    }

    /// Enumerating the entire interval and filtering every integer by the
    /// original product gives exactly Gamma(value). All other fields survive.
    /// Empty or interrupted builds install nothing; the caller distinguishes
    /// an ordinary fallback from its shared sticky mandatory stop.
    pub(crate) fn build(&self, mut check: impl FnMut() -> bool) -> Option<NumericValue> {
        let mut values = BTreeSet::new();
        let mut candidate = self.first;
        for index in 0..self.count {
            if !check() {
                return None;
            }
            if self.value.contains(candidate) {
                values.insert(candidate);
            }
            if index + 1 < self.count {
                candidate = candidate.checked_add(U256::from(1))?;
            }
        }
        if !check() {
            return None;
        }
        Some(NumericValue {
            finite: FiniteConstantSet::try_from_values(values).ok()?,
            ..self.value.clone()
        })
    }
}

#[cfg(test)]
mod tests;
