//! Independent all-account effects for prepaid root terminal coverage.
//!
//! This constructor does not rename a live Store or its saved history. Omitted
//! owners remain unconstrained through the global defaults and scoped inputs.

use super::{EvmStore, InitialIndependence, OrderedMap, Plane, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{SlotStateId, SourceId},
};
use std::collections::BTreeMap;

fn effects_plane<N: WordElement>() -> Result<Plane<N>, StoreError> {
    Ok(Plane {
        roots: OrderedMap::new(),
        base: SlotStateId::input(SourceId::fresh()?),
        operands: BTreeMap::new(),
        conditional: Default::default(),
        failure: None,
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: EvmValue::<N>::top(),
        initial_independence: InitialIndependence::new(true),
    })
}

impl<N: WordElement> EvmStore<N> {
    /// Bound arbitrary transaction-local effects, including omitted owners.
    ///
    /// The caller prepays construction, validation and retained lifetime. Each
    /// plane has a checked fresh source; account scoping separates owners within
    /// it. No original operands, observations or must-touch facts are rebound.
    /// This is a pre-finalization Store: transaction cleanup must still run.
    pub(crate) fn root_terminal_all_effects_checked_with<D: EvmNumericAlgebra<Element = N>>(
        domain: AlgebraRef<'_, D>,
    ) -> Result<Self, StoreError> {
        let out = Self {
            persistent: effects_plane()?,
            transient: effects_plane()?,
            balances: OrderedMap::new(),
            balance_default: EvmValue::<N>::top(),
            codes: OrderedMap::new(),
            nonces: OrderedMap::new(),
            existence: OrderedMap::new(),
            created: OrderedMap::new(),
            created_default: None,
            pending_destruction: OrderedMap::new(),
            pending_destruction_default: None,
            touched: OrderedMap::new(),
            touched_default: None,
            possible_logs: OrderedMap::new(),
            logs_unknown: true,
        };
        out.check_existing(domain.spec().relations().expressions())?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;

impl EvmStore<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn root_terminal_all_effects_checked(
        domain: crate::domain::Domain,
    ) -> Result<Self, StoreError> {
        Self::root_terminal_all_effects_checked_with(
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
    }
}
