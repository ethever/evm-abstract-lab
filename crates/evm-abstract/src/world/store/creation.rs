//! Paid native allocation shares the pure Store lifecycle transition.

use super::EvmStore;
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement, summary},
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use alloy_primitives::{Address, U256};

impl<N: WordElement> EvmStore<N> {
    pub(crate) fn nonce_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmValue<N>, CloneError> {
        pay(budget, self.nonces.len().saturating_add(1))?;
        match self.nonces.get(&address) {
            Some(value) => summary::clone_value(value, budget),
            None => summary::top(domain, budget),
        }
    }

    pub(crate) fn balance_with_payment(
        &self,
        address: Address,
        budget: &mut WorkBudget,
    ) -> Result<EvmValue<N>, CloneError> {
        pay(budget, self.balances.len().saturating_add(1))?;
        summary::clone_value(
            self.balances.get(&address).unwrap_or(&self.balance_default),
            budget,
        )
    }

    pub(crate) fn begin_creation_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<(), CloneError> {
        // reset_account scans each plane's slots and independence overrides;
        // the remaining indexed metadata operations have at most this many
        // existing entries to inspect. This fee owns no native scalar hook.
        let entries = self
            .persistent
            .slots
            .len()
            .saturating_add(self.transient.slots.len())
            .saturating_add(self.persistent.initial_independence.slots.len())
            .saturating_add(self.transient.initial_independence.slots.len())
            .saturating_add(self.code_identity_entries());
        pay(budget, entries.saturating_add(32))?;
        let persistent_zero = summary::literal(U256::ZERO, domain, budget)?;
        let transient_zero = summary::literal(U256::ZERO, domain, budget)?;
        let nonce = summary::literal(U256::from(1), domain, budget)?;
        self.begin_creation_with_values(address, persistent_zero, transient_zero, nonce);
        checkpoint(budget)
    }
}
