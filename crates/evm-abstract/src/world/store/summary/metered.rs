//! Staged summary operations keep native fees separate from sparse-map work.
//!
//! Results are built in fresh maps. Mutating a shared snapshot would introduce
//! backend copy-on-write clones unrelated to the scalar being updated. Native
//! values enter these maps only by a paid copy or by moving a paid hook result.

use super::{
    BTreeMap, BTreeSet, Code, EvmStoreSummary, EvmValue, OrderedMap, SummaryLog, SummaryPlane,
    U256, lifecycle, sparse,
};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary as scalar},
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};

mod layout;
#[cfg(test)]
mod tests;

type NumericMap<K, N> = OrderedMap<K, EvmValue<N>>;

pub(super) fn representable_default_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, CloneError> {
    if scalar::singleton(value, budget)? == Some(U256::ZERO) {
        scalar::scalar(value, domain, budget)
    } else {
        scalar::top(domain, budget)
    }
}

/// Fund key census, ordered-map lookup/insertion and fixed wrapper work. This
/// bound uses cached lengths only and supplies no native query/algebra credit.
pub(super) fn pay_structure(entries: usize, budget: &mut WorkBudget) -> Result<(), CloneError> {
    let levels = (usize::BITS - entries.max(1).leading_zeros()) as usize;
    pay(
        budget,
        entries
            .saturating_add(1)
            .saturating_mul(24usize.saturating_add(levels.saturating_mul(12))),
    )
}

fn combine_numeric<K: Ord + Clone, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    (left, left_default): (&NumericMap<K, N>, &EvmValue<N>),
    (right, right_default): (&NumericMap<K, N>, &EvmValue<N>),
    domain: AlgebraRef<'_, D>,
    widen: bool,
    budget: &mut WorkBudget,
) -> Result<(NumericMap<K, N>, EvmValue<N>), CloneError> {
    pay_structure(left.len().saturating_add(right.len()), budget)?;
    let default = scalar::join_scalar(left_default, right_default, domain, widen, budget)?;
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    let mut values = OrderedMap::new();
    for key in keys {
        let value = scalar::join_scalar(
            left.get(&key).unwrap_or(left_default),
            right.get(&key).unwrap_or(right_default),
            domain,
            widen,
            budget,
        )?;
        if !scalar::equal(&value, &default, domain, budget)? {
            values.insert(key, value);
        }
    }
    checkpoint(budget)?;
    Ok((values, default))
}

impl<N: WordElement> SummaryPlane<N> {
    fn join_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(
            self.clone_entries().saturating_add(other.clone_entries()),
            budget,
        )?;
        let (defaults, global_default) = combine_numeric(
            (&self.defaults, &self.global_default),
            (&other.defaults, &other.global_default),
            domain,
            widen,
            budget,
        )?;
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults,
            global_default,
            initial_independence: self.initial_independence.join(&other.initial_independence),
        };
        let keys: BTreeSet<_> = self
            .slots
            .keys()
            .chain(other.slots.keys())
            .copied()
            .collect();
        for (owner, key) in keys {
            let value = scalar::join_scalar(
                self.at(owner, key),
                other.at(owner, key),
                domain,
                widen,
                budget,
            )?;
            if !scalar::equal(&value, out.default_at(owner), domain, budget)? {
                out.slots.insert((owner, key), value);
            }
        }
        if widen {
            let owners: BTreeSet<_> = out.slots.keys().map(|(owner, _)| *owner).collect();
            let mut growing = BTreeMap::new();
            for owner in owners {
                let previous: BTreeSet<_> = self
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), _)| *key)
                    .collect();
                let joined: BTreeSet<_> = out
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), _)| *key)
                    .collect();
                if joined.len() > domain.capacity() && !joined.is_subset(&previous) {
                    if scalar::is_top(&out.global_default, domain, budget)? {
                        out.defaults.remove(&owner);
                    } else {
                        out.defaults.insert(owner, scalar::top(domain, budget)?);
                    }
                    let mut remove = Vec::new();
                    for ((_, key), value) in
                        out.slots.range((owner, U256::ZERO)..=(owner, U256::MAX))
                    {
                        if !previous.contains(key) || scalar::is_top(value, domain, budget)? {
                            remove.push((owner, *key));
                        }
                    }
                    for key in remove {
                        out.slots.remove(&key);
                    }
                    growing.insert(owner, previous);
                }
            }
            // One pass retains the same per-owner must facts, avoiding a full
            // independence-map scan for every growing owner. Values are bools.
            out.initial_independence.slots.retain(|(owner, key), _| {
                growing
                    .get(owner)
                    .is_none_or(|previous| previous.contains(key))
            });
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

fn join_log<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &SummaryLog<N>,
    right: &SummaryLog<N>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
    budget: &mut WorkBudget,
) -> Result<SummaryLog<N>, CloneError> {
    pay_structure(left.topics.len().saturating_add(right.topics.len()), budget)?;
    let mut topics = Vec::with_capacity(left.topics.len());
    for (left, right) in left.topics.iter().zip(&right.topics) {
        topics.push(scalar::join_scalar(left, right, domain, widen, budget)?);
    }
    let data = left
        .data
        .join_with_payment(&right.data, domain, widen, budget)?;
    checkpoint(budget)?;
    Ok(SummaryLog { topics, data })
}

// Code equality compares the fork and captured byte buffers. Their sizes are
// cached; these comparisons contain no native scalars or decoded-state queries.
pub(super) fn code_comparison_work(left: &Code, right: &Code) -> usize {
    if matches!((left,right),(Code::Runtime(left),Code::Runtime(right)) if left.shares_capture(right))
    {
        // Program's derived Arc equality on Eq data uses pointer identity.
        // This prices that exact successful shortcut, never a digest match or
        // an assumed equality between separately decoded allocations.
        return 32;
    }
    [left, right].into_iter().fold(16usize, |work, code| {
        work.saturating_add(match code {
            Code::Runtime(program) => program.byte_len(),
            _ => 1,
        })
    })
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Union using actual intermediate tariffs. Caller snapshots remain intact
    /// on refusal; prior paid work stays in the original cumulative ledger.
    pub(crate) fn join_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(
            self.clone_prefix_work()
                .saturating_add(other.clone_prefix_work()),
            budget,
        )?;
        let mut codes = OrderedMap::new();
        for (owner, code) in &self.codes {
            if let Some(incoming) = other.codes.get(owner) {
                pay(budget, code_comparison_work(code, incoming))?;
                if code == incoming {
                    codes.insert(*owner, code.clone());
                }
            }
        }
        let existence = self
            .existence
            .iter()
            .filter_map(|(owner, value)| {
                (other.existence.get(owner) == Some(value)).then_some((*owner, *value))
            })
            .collect();
        let mut logs_unknown = self.logs_unknown || other.logs_unknown;
        if !logs_unknown {
            // A topic-count conflict discards every log in the pure operation.
            // Inspect only those lengths before attempting any native work.
            logs_unknown = other.possible_logs.iter().any(|(key, incoming)| {
                self.possible_logs
                    .get(key)
                    .is_some_and(|previous| previous.topics.len() != incoming.topics.len())
            });
        }
        let mut possible_logs = OrderedMap::new();
        if !logs_unknown {
            let keys: BTreeSet<_> = self
                .possible_logs
                .keys()
                .chain(other.possible_logs.keys())
                .cloned()
                .collect();
            for key in keys {
                let log = match (self.possible_logs.get(&key), other.possible_logs.get(&key)) {
                    (Some(left), Some(right)) => join_log(left, right, domain, widen, budget)?,
                    (Some(log), None) | (None, Some(log)) => log.clone_with_payment(budget)?,
                    (None, None) => unreachable!("key came from the input union"),
                };
                possible_logs.insert(key, log);
            }
        }
        let (balances, balance_default) = combine_numeric(
            (&self.balances, &self.balance_default),
            (&other.balances, &other.balance_default),
            domain,
            widen,
            budget,
        )?;
        let left_nonce_default = scalar::top(domain, budget)?;
        let right_nonce_default = scalar::top(domain, budget)?;
        let (nonces, _) = combine_numeric(
            (&self.nonces, &left_nonce_default),
            (&other.nonces, &right_nonce_default),
            domain,
            widen,
            budget,
        )?;
        let (created, created_default) = sparse::combine(
            (&self.created, &self.created_default),
            (&other.created, &other.created_default),
            lifecycle::join,
        );
        let (pending_destruction, pending_destruction_default) = sparse::combine(
            (&self.pending_destruction, &self.pending_destruction_default),
            (
                &other.pending_destruction,
                &other.pending_destruction_default,
            ),
            lifecycle::join,
        );
        let (touched, touched_default) = sparse::combine(
            (&self.touched, &self.touched_default),
            (&other.touched, &other.touched_default),
            lifecycle::join,
        );
        let out = Self {
            persistent: self.persistent.join_with_payment(
                &other.persistent,
                domain,
                widen,
                budget,
            )?,
            transient: self
                .transient
                .join_with_payment(&other.transient, domain, widen, budget)?,
            balances,
            balance_default,
            codes,
            nonces,
            existence,
            created,
            created_default,
            pending_destruction,
            pending_destruction_default,
            touched,
            touched_default,
            possible_logs,
            logs_unknown,
        };
        checkpoint(budget)?;
        Ok(out)
    }
}
