//! Reproduce derived summary Eq without an unquoted native comparison.

use super::{
    Address, AlgebraRef, CloneError, EvmNumericAlgebra, EvmStoreSummary, EvmValue, OrderedMap,
    SummaryPlane, U256, WordElement, WorkBudget, checkpoint, codes_equal, pay, scalar,
};
use crate::world::store::{InitialIndependence, LogKey};

#[cfg(test)]
mod tests;

// This private boundary admits only the summary's fixed-size coordinate types.
// Comparing all bytes is a bound even when an earlier field decides equality.
trait EqualityKey: Ord + Clone {
    const BYTES: usize;
}

impl EqualityKey for Address {
    const BYTES: usize = 20;
}

impl EqualityKey for (Address, U256) {
    const BYTES: usize = 20 + 32;
}

impl EqualityKey for LogKey {
    const BYTES: usize = 20 + 20 + 32 + 1 + size_of::<usize>();
}

/// Borrowed forward iteration constructs no key union and performs no lookup
/// or insertion. Keep the structural rate of 24 per entry/header, with 12 per
/// height frame for iterator setup and release. A complete traversal visits
/// every tree edge only boundedly; the height term also funds imbl's cursor
/// stack. Both inputs are prepaid even if comparison stops at an early key.
/// Scalar/native, Program and byte payloads retain their separate full quotes.
fn pay_map<K: EqualityKey, V: Clone>(
    left: &OrderedMap<K, V>,
    right: &OrderedMap<K, V>,
    metadata_bytes: usize,
    budget: &mut WorkBudget,
) -> Result<(), CloneError> {
    checkpoint(budget)?;
    let left = left.len();
    let right = right.len();
    let heights = (usize::BITS - left.leading_zeros()) as usize
        + (usize::BITS - right.leading_zeros()) as usize;
    pay(
        budget,
        left.saturating_add(right)
            .saturating_add(1)
            .saturating_mul(24)
            .saturating_add(heights.saturating_mul(12))
            .saturating_add(
                left.min(right)
                    .saturating_mul(K::BYTES.saturating_add(metadata_bytes)),
            ),
    )
}

// Only bool, Existence and Option<bool> instantiate V here: at most a tag and
// a boolean. Explicit iteration gives both map backends the same operation;
// no backend-specific shared-tree Eq shortcut supplies an unpaid payload query.
fn metadata_equal<K: EqualityKey, V: Copy + Eq>(
    left: &OrderedMap<K, V>,
    right: &OrderedMap<K, V>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    pay_map(left, right, 2, budget)?;
    if left.len() != right.len() {
        return answer(false, budget);
    }
    answer(left.iter().eq(right.iter()), budget)
}

fn independence_equal(
    left: &InitialIndependence,
    right: &InitialIndependence,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    let equal = metadata_equal(&left.slots, &right.slots, budget)?
        && metadata_equal(&left.defaults, &right.defaults, budget)?
        && left.global_default == right.global_default;
    answer(equal, budget)
}

fn answer(equal: bool, budget: &WorkBudget) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    Ok(equal)
}

fn map_equal<K: EqualityKey, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &OrderedMap<K, EvmValue<N>>,
    right: &OrderedMap<K, EvmValue<N>>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    pay_map(left, right, 0, budget)?;
    if left.len() != right.len() {
        return answer(false, budget);
    }
    for ((left_key, left), (right_key, right)) in left.iter().zip(right.iter()) {
        if left_key != right_key || !scalar::equal(left, right, domain, budget)? {
            return answer(false, budget);
        }
    }
    answer(true, budget)
}

impl<N: WordElement> SummaryPlane<N> {
    fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        pay(budget, 24)?;
        let equal = map_equal(&self.slots, &other.slots, domain, budget)?
            && map_equal(&self.defaults, &other.defaults, domain, budget)?
            && scalar::equal(&self.global_default, &other.global_default, domain, budget)?
            && independence_equal(
                &self.initial_independence,
                &other.initial_independence,
                budget,
            )?;
        answer(equal, budget)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Exact existing representation equality, including native predicates,
    /// scalar metadata, owner coordinates, code, lifecycle and event payloads.
    pub(crate) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        // Fixed Store fields only; planes and each visited map pay their own
        // traversal once, at the original comparison point.
        pay(budget, 24)?;
        if !self
            .persistent
            .eq_with_payment(&other.persistent, domain, budget)?
            || !self
                .transient
                .eq_with_payment(&other.transient, domain, budget)?
            || !map_equal(&self.balances, &other.balances, domain, budget)?
            || !scalar::equal(
                &self.balance_default,
                &other.balance_default,
                domain,
                budget,
            )?
        {
            return answer(false, budget);
        }
        pay_map(&self.codes, &other.codes, 0, budget)?;
        if self.codes.len() != other.codes.len() {
            return answer(false, budget);
        }
        for ((left_owner, left), (right_owner, right)) in self.codes.iter().zip(other.codes.iter())
        {
            if left_owner != right_owner {
                return answer(false, budget);
            }
            if !codes_equal(left, right, budget)? {
                return answer(false, budget);
            }
        }
        if !map_equal(&self.nonces, &other.nonces, domain, budget)?
            || !metadata_equal(&self.existence, &other.existence, budget)?
            || !metadata_equal(&self.created, &other.created, budget)?
            || self.created_default != other.created_default
            || !metadata_equal(
                &self.pending_destruction,
                &other.pending_destruction,
                budget,
            )?
            || self.pending_destruction_default != other.pending_destruction_default
            || !metadata_equal(&self.touched, &other.touched, budget)?
            || self.touched_default != other.touched_default
            || self.possible_logs.len() != other.possible_logs.len()
        {
            return answer(false, budget);
        }
        pay_map(&self.possible_logs, &other.possible_logs, 0, budget)?;
        for ((left_key, left), (right_key, right)) in
            self.possible_logs.iter().zip(other.possible_logs.iter())
        {
            if left_key != right_key || left.topics.len() != right.topics.len() {
                return answer(false, budget);
            }
            pay(
                budget,
                left.topics
                    .len()
                    .saturating_add(right.topics.len())
                    .saturating_add(1)
                    .saturating_mul(24),
            )?;
            for (left, right) in left.topics.iter().zip(&right.topics) {
                if !scalar::equal(left, right, domain, budget)? {
                    return answer(false, budget);
                }
            }
            if !left.data.eq_with_payment(&right.data, domain, budget)? {
                return answer(false, budget);
            }
        }
        answer(self.logs_unknown == other.logs_unknown, budget)
    }
}
