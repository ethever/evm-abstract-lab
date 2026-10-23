//! Named preservation and an independent Top complement for process inputs.
//!
//! Fresh maps avoid implicit copy-on-write clones in either snapshot backend.
//! Every native constructor, comparison and retained copy uses its actual paid
//! hook. Preparation intentionally drops outside account and event precision.

use super::metered::pay_structure;
use super::{EvmStoreSummary, InitialIndependence, OrderedMap, SummaryPlane};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement, summary as scalar},
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
    world::process::OwnerCatalogue,
};
use alloy_primitives::Address;

#[cfg(test)]
mod tests;

fn named_numeric<N: WordElement>(
    values: &OrderedMap<Address, EvmValue<N>>,
    names: &std::collections::BTreeSet<Address>,
    budget: &mut WorkBudget,
) -> Result<OrderedMap<Address, EvmValue<N>>, CloneError> {
    pay_structure(values.len().saturating_add(names.len()), budget)?;
    let mut out = OrderedMap::new();
    for (owner, value) in values {
        if names.contains(owner) {
            out.insert(*owner, scalar::clone_value(value, budget)?);
        }
    }
    Ok(out)
}

impl<N: WordElement> SummaryPlane<N> {
    fn process_complement<D: EvmNumericAlgebra<Element = N>>(
        &self,
        catalogue: &OwnerCatalogue,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        let names = catalogue.named();
        pay_structure(self.clone_entries().saturating_add(names.len()), budget)?;
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: scalar::top(domain, budget)?,
            initial_independence: InitialIndependence::new(true),
        };
        for owner in names {
            let value = self.default_at(*owner);
            if !scalar::equal(value, &out.global_default, domain, budget)? {
                out.defaults
                    .insert(*owner, scalar::clone_value(value, budget)?);
            }
            let independent = self.initial_independence.default_at(*owner);
            if !independent {
                out.initial_independence.defaults.insert(*owner, false);
            }
        }
        for ((owner, slot), value) in &self.slots {
            if names.contains(owner) {
                out.slots
                    .insert((*owner, *slot), scalar::clone_value(value, budget)?);
            }
        }
        for ((owner, slot), independent) in &self.initial_independence.slots {
            if names.contains(owner) {
                out.initial_independence
                    .set_slot(*owner, *slot, *independent);
            }
        }
        checkpoint(budget)?;
        Ok(out)
    }

    fn process_havoc_storage<D: EvmNumericAlgebra<Element = N>>(
        &self,
        owner: Address,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_entries(), budget)?;
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: scalar::clone_value(&self.global_default, budget)?,
            initial_independence: self.initial_independence.clone(),
        };
        let top = scalar::top(domain, budget)?;
        if !scalar::equal(&top, &out.global_default, domain, budget)? {
            out.defaults.insert(owner, top);
        }
        for (address, value) in &self.defaults {
            if *address != owner {
                out.defaults
                    .insert(*address, scalar::clone_value(value, budget)?);
            }
        }
        for ((address, slot), value) in &self.slots {
            if *address != owner {
                out.slots
                    .insert((*address, *slot), scalar::clone_value(value, budget)?);
            }
        }
        out.initial_independence.replace_account(owner);
        checkpoint(budget)?;
        Ok(out)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    pub(crate) fn process_complement<D: EvmNumericAlgebra<Element = N>>(
        &self,
        catalogue: &OwnerCatalogue,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        let names = catalogue.named();
        pay_structure(self.clone_prefix_work().saturating_add(names.len()), budget)?;
        let persistent = self
            .persistent
            .process_complement(catalogue, domain, budget)?;
        let transient = self
            .transient
            .process_complement(catalogue, domain, budget)?;
        let balance_default = scalar::top(domain, budget)?;
        let mut balances = OrderedMap::new();
        let mut created = OrderedMap::new();
        let mut pending_destruction = OrderedMap::new();
        let mut touched = OrderedMap::new();
        for owner in names {
            let value = self.balances.get(owner).unwrap_or(&self.balance_default);
            if !scalar::equal(value, &balance_default, domain, budget)? {
                balances.insert(*owner, scalar::clone_value(value, budget)?);
            }
            for (source, default, target) in [
                (&self.created, self.created_default, &mut created),
                (
                    &self.pending_destruction,
                    self.pending_destruction_default,
                    &mut pending_destruction,
                ),
                (&self.touched, self.touched_default, &mut touched),
            ] {
                let value = source.get(owner).copied().unwrap_or(default);
                if value.is_some() {
                    target.insert(*owner, value);
                }
            }
        }
        let mut codes = OrderedMap::new();
        for (owner, code) in &self.codes {
            if names.contains(owner) {
                // Code owns only a Program handle or a fixed-size descriptor.
                pay(budget, 16)?;
                codes.insert(*owner, code.clone());
            }
        }
        let out = Self {
            persistent,
            transient,
            balances,
            balance_default,
            codes,
            nonces: named_numeric(&self.nonces, names, budget)?,
            existence: self
                .existence
                .iter()
                .filter_map(|(owner, value)| names.contains(owner).then_some((*owner, *value)))
                .collect(),
            created,
            created_default: None,
            pending_destruction,
            pending_destruction_default: None,
            touched,
            touched_default: None,
            possible_logs: OrderedMap::new(),
            logs_unknown: true,
        };
        checkpoint(budget)?;
        Ok(out)
    }

    pub(crate) fn process_havoc_storage<D: EvmNumericAlgebra<Element = N>>(
        &self,
        owner: Address,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        let persistent = self
            .persistent
            .process_havoc_storage(owner, domain, budget)?;
        let transient = self
            .transient
            .process_havoc_storage(owner, domain, budget)?;
        let mut out = self.clone_with_payment(budget)?;
        out.persistent = persistent;
        out.transient = transient;
        checkpoint(budget)?;
        Ok(out)
    }
}
