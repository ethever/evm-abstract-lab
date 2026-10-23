//! Paid operational effect envelope for process-only Store representations.
//!
//! Every native field enters a fresh map by a paid copy or by moving a paid
//! constructor result. A factory quote never buys a later copy/query of its
//! returned value. Replaying the result uses the existing paid Store builder.

use super::metered::pay_structure;
use super::{
    Code, EvmStore, EvmStoreSummary, InitialIndependence, OrderedMap, SummaryPlane, lifecycle,
};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary as scalar},
    resource::{
        CloneError, SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
    world::StoreError,
};
use alloy_primitives::Address;
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

impl<N: WordElement> SummaryPlane<N> {
    fn process_effect_plane<D: EvmNumericAlgebra<Element = N>>(
        &self,
        stable: &BTreeSet<Address>,
        writable: Option<Address>,
        named_aliases: bool,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_entries().saturating_add(stable.len()), budget)?;
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: scalar::top(domain, budget)?,
            initial_independence: InitialIndependence::new(true),
        };
        for owner in stable
            .iter()
            .copied()
            .filter(|owner| !named_aliases && Some(*owner) != writable)
        {
            out.defaults
                .insert(owner, scalar::clone_value(self.default_at(owner), budget)?);
            let independent = self.initial_independence.default_at(owner);
            if !independent {
                out.initial_independence.defaults.insert(owner, false);
            }
        }
        for ((owner, slot), value) in &self.slots {
            if !named_aliases && stable.contains(owner) && Some(*owner) != writable {
                out.slots
                    .insert((*owner, *slot), scalar::clone_value(value, budget)?);
            }
        }
        for ((owner, slot), independent) in &self.initial_independence.slots {
            if !named_aliases && stable.contains(owner) && Some(*owner) != writable {
                out.initial_independence
                    .set_slot(*owner, *slot, *independent);
            }
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Structural traversal of a process bound shares its captured Programs.
    /// Scalar hooks, Store replay and actual code comparisons keep their own
    /// invoices; this is never a native clone/query tariff for N.
    pub(crate) fn process_work_size(&self) -> usize {
        let work = self.work_size();
        if work == usize::MAX {
            return work;
        }
        let bytes = self.codes.values().fold(0usize, |work, code| {
            work.saturating_add(if let Code::Runtime(program) = code {
                program.byte_len().saturating_mul(2)
            } else {
                0
            })
        });
        work.saturating_sub(bytes)
            .saturating_add(self.codes.len().saturating_mul(16))
    }

    /// Cover arbitrary process effects. `named_aliases` weakens all possible
    /// named aliases of a symbolic root binding. Otherwise `writable=None`
    /// grants no named storage writes; the independent outside complement
    /// already covers weak outside writes.
    /// `protected` is the frozen named vocabulary, never evolving Store keys.
    pub(crate) fn process_effects_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        protected: &BTreeSet<Address>,
        writable: Option<Address>,
        named_aliases: bool,
        is_static: bool,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(
            self.clone_prefix_work().saturating_add(protected.len()),
            budget,
        )?;
        let mut touched = OrderedMap::new();
        let touched_default = lifecycle::or(self.touched_default, None);
        for (owner, value) in &self.touched {
            let value = lifecycle::or(*value, None);
            if value != touched_default {
                touched.insert(*owner, value);
            }
        }
        if is_static {
            let mut out = self.clone_with_payment(budget)?;
            // Replacing the complete metadata map introduces no native COW.
            out.touched = touched;
            out.touched_default = touched_default;
            checkpoint(budget)?;
            return Ok(out);
        }
        let mut stable = BTreeSet::new();
        for owner in protected {
            let code = self.codes.get(owner);
            let captured_nonempty = matches!(code, Some(Code::Delegation(_)))
                || matches!(code, Some(Code::Runtime(program)) if program.byte_len() != 0);
            // A created account retains its runtime/nonce identity until the
            // transaction ends, including while its constructor is active.
            let created = self
                .created
                .get(owner)
                .copied()
                .unwrap_or(self.created_default)
                == Some(true);
            if captured_nonempty || created {
                stable.insert(*owner);
            }
        }
        let persistent = self.persistent.process_effect_plane(
            &stable,
            writable,
            named_aliases,
            domain,
            budget,
        )?;
        let transient = self.transient.process_effect_plane(
            &stable,
            writable,
            named_aliases,
            domain,
            budget,
        )?;
        let balance_default = scalar::top(domain, budget)?;
        let mut codes = OrderedMap::new();
        let mut nonces = OrderedMap::new();
        let mut existence = OrderedMap::new();
        let mut created = OrderedMap::new();
        let mut pending_destruction = OrderedMap::new();
        for owner in stable {
            if let Some(code) = self.codes.get(&owner) {
                pay(budget, 16)?;
                codes.insert(owner, code.clone());
            }
            let nonce = if named_aliases || Some(owner) == writable {
                // CREATE may increment the current owner's nonce. A definitely
                // created account starts at one and never returns to zero.
                if self
                    .created
                    .get(&owner)
                    .copied()
                    .unwrap_or(self.created_default)
                    == Some(true)
                {
                    scalar::unsigned_range(
                        alloy_primitives::U256::ONE,
                        alloy_primitives::U256::from(u64::MAX),
                        domain,
                        budget,
                    )?
                } else {
                    scalar::top(domain, budget)?
                }
            } else if let Some(value) = self.nonces.get(&owner) {
                scalar::clone_value(value, budget)?
            } else {
                scalar::top(domain, budget)?
            };
            nonces.insert(owner, nonce);
            if let Some(value) = self.existence.get(&owner) {
                existence.insert(owner, *value);
            }
            created.insert(
                owner,
                self.created
                    .get(&owner)
                    .copied()
                    .unwrap_or(self.created_default),
            );
            let pending = if Some(owner) == writable
                && created.get(&owner).copied().flatten() != Some(false)
            {
                None
            } else {
                self.pending_destruction
                    .get(&owner)
                    .copied()
                    .unwrap_or(self.pending_destruction_default)
            };
            pending_destruction.insert(owner, pending);
        }
        // Earlier logs remain possible; native topic/byte copies own their full
        // invoices. New arbitrary logs are represented by logs_unknown.
        let mut possible_logs = OrderedMap::new();
        for (site, log) in &self.possible_logs {
            possible_logs.insert(site.clone(), log.clone_with_payment(budget)?);
        }
        let out = Self {
            persistent,
            transient,
            balances: OrderedMap::new(),
            balance_default,
            codes,
            nonces,
            existence,
            created,
            created_default: None,
            pending_destruction,
            pending_destruction_default: None,
            touched,
            touched_default,
            possible_logs,
            logs_unknown: true,
        };
        checkpoint(budget)?;
        Ok(out)
    }
}

impl<N: WordElement> EvmStore<N> {
    /// Process preparation shares code allocations even in its transient
    /// native carrier. Native scalar clone/query invoices remain independent.
    pub(crate) fn process_work_size(&self) -> usize {
        let work = self.work_size();
        if work == usize::MAX {
            return work;
        }
        let bytes = self.codes.values().fold(0usize, |work, code| {
            work.saturating_add(if let Code::Runtime(program) = code {
                program.byte_len()
            } else {
                0
            })
        });
        work.saturating_sub(bytes)
            .saturating_add(self.codes.len().saturating_mul(16))
    }

    /// Stage a process envelope and replay it through actual paid native hooks.
    /// A refusal returns no new Store and does not modify the original input.
    pub(crate) fn process_effects_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        protected: &BTreeSet<Address>,
        writable: Option<Address>,
        named_aliases: bool,
        is_static: bool,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, SummaryError<StoreError>> {
        let summary = self.summary_projection_with_payment(domain, budget)?;
        let staged = summary.process_effects_with_payment(
            protected,
            writable,
            named_aliases,
            is_static,
            domain,
            budget,
        )?;
        staged.instantiate_with_payment(domain, budget)
    }
}
