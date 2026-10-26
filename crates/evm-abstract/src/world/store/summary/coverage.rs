//! Pointwise sufficient coverage with the same sparse defaults as summary join.

use super::{Code, EvmStoreSummary, EvmValue, InitialIndependence, OrderedMap, SummaryPlane};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::{CloneError, WorkBudget, clone_payment::checkpoint},
};

#[cfg(test)]
mod tests;

fn numeric_map<K: Ord + Clone, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &OrderedMap<K, EvmValue<N>>,
    right: &OrderedMap<K, EvmValue<N>>,
    left_default: &EvmValue<N>,
    right_default: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    if !summary::covers_scalar(left_default, right_default, domain, budget)? {
        return Ok(false);
    }
    // Visiting both key streams can repeat a key, but allocates no union. Each
    // omitted cell resolves against its OWN input's owner/global default.
    for key in left.keys().chain(right.keys()) {
        if !summary::covers_scalar(
            left.get(key).unwrap_or(left_default),
            right.get(key).unwrap_or(right_default),
            domain,
            budget,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn independence(left: &InitialIndependence, right: &InitialIndependence) -> bool {
    (!left.global_default || right.global_default)
        && left
            .defaults
            .keys()
            .chain(right.defaults.keys())
            .all(|owner| !left.default_at(*owner) || right.default_at(*owner))
        && left
            .slots
            .keys()
            .chain(right.slots.keys())
            .all(|(owner, key)| !left.at(*owner, *key) || right.at(*owner, *key))
}

impl<N: WordElement> SummaryPlane<N> {
    fn covers_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        if !independence(&self.initial_independence, &other.initial_independence)
            || !numeric_map(
                &self.defaults,
                &other.defaults,
                &self.global_default,
                &other.global_default,
                domain,
                budget,
            )?
        {
            return Ok(false);
        }
        for (owner, key) in self.slots.keys().chain(other.slots.keys()) {
            if !summary::covers_scalar(
                self.at(*owner, *key),
                other.at(*owner, *key),
                domain,
                budget,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn lifecycle<K: Ord + Clone>(
    left: &OrderedMap<K, Option<bool>>,
    right: &OrderedMap<K, Option<bool>>,
    left_default: Option<bool>,
    right_default: Option<bool>,
) -> bool {
    let covers = |left: Option<bool>, right| left.is_none() || left == right;
    covers(left_default, right_default)
        && left.keys().chain(right.keys()).all(|key| {
            covers(
                left.get(key).copied().unwrap_or(left_default),
                right.get(key).copied().unwrap_or(right_default),
            )
        })
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Sufficient Top-or-native-equal coverage of every scalar and compatible
    /// code, lifecycle, touch, initial-dependency and event metadata. False also
    /// means unsupported/unavailable: callers must keep their paid join path.
    /// Structural work precedes all traversal; native queries pay separately.
    pub(crate) fn covers_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        let entries = self
            .clone_prefix_work()
            .saturating_add(other.clone_prefix_work());
        let levels = (usize::BITS - entries.max(1).leading_zeros()) as usize;
        // Both sparse key streams, fixed keys (including full log sources),
        // owner-dependent lookups and both initial-independence maps. No key
        // union, metadata result or executable Store is allocated.
        if !summary::try_proof_work(
            budget,
            entries
                .saturating_add(1)
                .saturating_mul(96usize.saturating_add(levels.saturating_mul(48))),
        )? {
            return Ok(false);
        }
        if !self
            .persistent
            .covers_with_payment(&other.persistent, domain, budget)?
            || !self
                .transient
                .covers_with_payment(&other.transient, domain, budget)?
            || !numeric_map(
                &self.balances,
                &other.balances,
                &self.balance_default,
                &other.balance_default,
                domain,
                budget,
            )?
        {
            return Ok(false);
        }
        // Nonces have an implicit Top default. Inspect only retained guarantees;
        // a missing incoming nonce cannot discharge a retained constraint.
        for (owner, value) in &self.nonces {
            let Some(incoming) = other.nonces.get(owner) else {
                return Ok(false);
            };
            if !summary::covers_scalar(value, incoming, domain, budget)? {
                return Ok(false);
            }
        }
        // Code/existence join intersects identical entries. Extra incoming facts
        // are harmless; every retained fact must occur in both inputs.
        for (owner, code) in &self.codes {
            let Some(incoming) = other.codes.get(owner) else {
                return Ok(false);
            };
            match (code, incoming) {
                (Code::Runtime(left), Code::Runtime(right)) => {
                    // Only the existing immutable capture identity shortcut is
                    // sufficient here; independently decoded equality falls back.
                    if !left.shares_capture(right) {
                        return Ok(false);
                    }
                }
                (Code::Runtime(_), _) | (_, Code::Runtime(_)) => return Ok(false),
                _ if code != incoming => return Ok(false),
                _ => {}
            }
        }
        if self
            .existence
            .iter()
            .any(|(owner, value)| other.existence.get(owner) != Some(value))
            || !lifecycle(
                &self.created,
                &other.created,
                self.created_default,
                other.created_default,
            )
            || !lifecycle(
                &self.pending_destruction,
                &other.pending_destruction,
                self.pending_destruction_default,
                other.pending_destruction_default,
            )
            || !lifecycle(
                &self.touched,
                &other.touched,
                self.touched_default,
                other.touched_default,
            )
        {
            return Ok(false);
        }
        if self.logs_unknown {
            // The existing join clears every payload when logs are unknown.
            checkpoint(budget)?;
            return Ok(self.possible_logs.is_empty());
        }
        if other.logs_unknown {
            return Ok(false);
        }
        for (key, incoming) in &other.possible_logs {
            let Some(retained) = self.possible_logs.get(key) else {
                return Ok(false);
            };
            if !summary::try_proof_work(
                budget,
                retained
                    .topics
                    .len()
                    .saturating_add(incoming.topics.len())
                    .saturating_add(1)
                    .saturating_mul(16),
            )? || retained.topics.len() != incoming.topics.len()
            {
                return Ok(false);
            }
            for (left, right) in retained.topics.iter().zip(&incoming.topics) {
                if !summary::covers_scalar(left, right, domain, budget)? {
                    return Ok(false);
                }
            }
            if !retained
                .data
                .covers_with_payment(&incoming.data, domain, budget)?
            {
                return Ok(false);
            }
        }
        checkpoint(budget)?;
        Ok(true)
    }
}
