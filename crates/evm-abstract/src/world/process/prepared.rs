//! A process-only owner quotient over the existing paid native Store summary.

use super::{OwnerCatalogue, ProcessOwner, ProcessOwnerError, ProcessOwnerKey};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement},
    resource::{WorkBudget, clone_payment::pay},
    world::{EvmStore, EvmStoreSummary},
};
use std::sync::Arc;

/// Canonical process input with independent Top facts for every outside owner.
/// It does not authorize an ordinary native frame at a representative address.
#[derive(Debug)]
pub struct PreparedProcessStore<N: WordElement> {
    catalogue: Arc<OwnerCatalogue>,
    key: ProcessOwnerKey,
    state: EvmStoreSummary<N>,
}

impl<N: WordElement> PreparedProcessStore<N> {
    /// Preserve named state and cover the complete outside complement. The
    /// caller must establish the physical owner class independently of ADDRESS.
    /// Process body admission separately calls `OwnerCatalogue::admit_code`.
    pub fn prepare<D: EvmNumericAlgebra<Element = N>>(
        store: &EvmStore<N>,
        catalogue: &Arc<OwnerCatalogue>,
        owner: ProcessOwner,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, ProcessOwnerError> {
        pay(budget, 32)?;
        let key = catalogue.key(owner)?;
        let state = store.summary_projection_with_payment(domain, budget)?;
        let state = state.process_complement(catalogue, domain, budget)?;
        Ok(Self {
            catalogue: Arc::clone(catalogue),
            key,
            state,
        })
    }

    /// Class and content epoch; the solver must separately retain code shape.
    pub fn key(&self) -> ProcessOwnerKey {
        self.key
    }

    /// Join only compatible physical classes, using the existing native join.
    pub fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, ProcessOwnerError> {
        pay(budget, 32)?;
        if self.key != incoming.key {
            return Err(ProcessOwnerError::Shape);
        }
        Ok(Self {
            catalogue: Arc::clone(&self.catalogue),
            key: self.key,
            state: self
                .state
                .join_with_payment(&incoming.state, domain, false, budget)?,
        })
    }

    /// Paid exact equality of canonical bounds, including private native facts.
    pub fn equivalent<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, ProcessOwnerError> {
        pay(budget, 16)?;
        Ok(self.key == other.key && self.state.eq_with_payment(&other.state, domain, budget)?)
    }

    /// Cover arbitrary SSTORE/TSTORE at this physical owner class. This local
    /// rule grants no balance, allocation, logging or SELFDESTRUCT capability.
    /// Static write attempts have no successful storage effect; the solver
    /// separately retains entered Failure and restores its caller checkpoint.
    pub fn storage_effect<D: EvmNumericAlgebra<Element = N>>(
        &self,
        is_static: bool,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, ProcessOwnerError> {
        pay(budget, 32)?;
        let state = if !is_static && let ProcessOwner::Named(owner) = self.key.owner {
            self.state.process_havoc_storage(owner, domain, budget)?
        } else {
            // Residual's independent Top complement already covers every
            // weak outside write, including aliases and reentry. No address is
            // inserted and no named fact is changed by that local action.
            self.state.clone_with_payment(budget)?
        };
        Ok(Self {
            catalogue: Arc::clone(&self.catalogue),
            key: self.key,
            state,
        })
    }

    /// Rebuild independent per-address native Store roots for process use.
    /// Residual frames still require coarse effects and operand overrides.
    pub fn instantiate_for_process<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmStore<N>, ProcessOwnerError> {
        Ok(self.state.instantiate_with_payment(domain, budget)?)
    }
}
