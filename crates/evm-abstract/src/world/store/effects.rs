//! Capabilities of unknown execution, separated from known-owner callbacks.

use super::{EvmStore, InitialIndependence, OrderedMap, Plane, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::{
    domain::{
        EvmValue,
        symbolic::{ExprId, ExprLimits, SlotStateId, SourceId, StateError},
    },
    world::Code,
};
use alloy_primitives::{Address, U256};
use std::collections::BTreeSet;

impl<N: WordElement> Plane<N> {
    /// Replace only unprotected current storage. The caller owns work admission
    /// and whole-Store validation; this helper stages every plane update.
    pub(super) fn havoc_except_checked(
        &mut self,
        protected: &BTreeSet<Address>,
        limits: ExprLimits,
    ) -> Result<(), StoreError> {
        self.havoc_except_with_source(protected, limits, SourceId::fresh)
    }

    #[cfg(test)]
    pub(super) fn havoc_except_with_fresh(
        &mut self,
        protected: &BTreeSet<Address>,
        limits: ExprLimits,
        fresh: impl FnOnce() -> Option<ExprId>,
    ) -> Result<(), StoreError> {
        self.havoc_except_with_source(protected, limits, || {
            fresh()
                .map(|token| SourceId::scoped(token, U256::ZERO))
                .ok_or(StateError::IdentityExhausted)
        })
    }

    fn havoc_except_with_source(
        &mut self,
        protected: &BTreeSet<Address>,
        limits: ExprLimits,
        fresh: impl FnOnce() -> Result<SourceId, StateError>,
    ) -> Result<(), StoreError> {
        let interrupt = || {
            if analysis_control::current_cancellation().is_cancelled() {
                Err(StoreError::State(StateError::Cancelled))
            } else {
                Ok(())
            }
        };
        interrupt()?;
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let base = SlotStateId::input(fresh()?);
        base.join_guarded(&base, ExprId::constant(U256::ZERO), limits.into())?;
        // Retained roots can refer to operands and snapshot sources that are not
        // visible in the sparse cell cache. Keep their complete interpretation.
        let mut staged = Self {
            roots: OrderedMap::new(),
            base,
            operands: self.operands.clone(),
            conditional: self.conditional.clone(),
            failure: None,
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: EvmValue::<N>::top(),
            initial_independence: InitialIndependence::new(true),
        };
        for address in protected {
            interrupt()?;
            let root = self.root_at(*address)?;
            root.join_guarded(&root, ExprId::constant(U256::ZERO), limits.into())?;
            staged.roots.insert(*address, root);
            staged
                .defaults
                .insert(*address, self.default_at(*address).clone());
            let independent = self.initial_independence.default_at(*address);
            if !independent {
                staged.initial_independence.defaults.insert(*address, false);
            }
        }
        for ((owner, key), value) in &self.slots {
            interrupt()?;
            if protected.contains(owner) {
                staged.slots.insert((*owner, *key), value.clone());
            }
        }
        for ((owner, key), independent) in &self.initial_independence.slots {
            interrupt()?;
            if protected.contains(owner) {
                staged
                    .initial_independence
                    .set_slot(*owner, *key, *independent);
            }
        }
        interrupt()?;
        *self = staged;
        Ok(())
    }
}

impl<N: WordElement> EvmStore<N> {
    /// Unknown-world effects under the selected transaction lifecycle rules.
    /// Every supported fork keeps SELFDESTRUCT code visible until finalization.
    pub fn apply_unknown_effects<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        protected: &BTreeSet<Address>,
        delegated_owner: Option<Address>,
        is_static: bool,
        fork: crate::Fork,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), StoreError> {
        match fork {
            crate::Fork::Cancun | crate::Fork::Prague | crate::Fork::Osaka => {
                self.unknown_effects(protected, delegated_owner, is_static, domain)
            }
        }
    }

    /// Arbitrary SSTORE/TSTORE capability at one execution owner. Account code,
    /// nonce, balance, lifecycle and already emitted events remain unchanged.
    pub fn havoc_storage_owner(&mut self, owner: Address) {
        self.persistent.havoc_account(owner);
        self.transient.havoc_account(owner);
    }

    /// Cover unknown internal steps and unknown-to-unknown interactions.
    /// Known executable owners change storage through analyzed callbacks; a
    /// delegated unknown execution additionally writes its current owner.
    /// Static execution preserves values but can touch zero-value recipients.
    /// Errors leave the store unchanged.
    pub(crate) fn unknown_effects<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        protected: &BTreeSet<Address>,
        writable_owner: Option<Address>,
        is_static: bool,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), StoreError> {
        if is_static {
            // STATICCALL (and zero-value CALL under a static parent) still
            // passes through transfer_loaded in revm. Nested successful calls
            // can therefore make empty accounts eligible for EIP-161 clearing.
            self.may_touch_all();
            return Ok(());
        }
        let limits = domain.spec().relations().expressions();
        self.check_existing(limits)?;
        let mut out = self.clone();
        // Arbitrary successful internal calls may touch any recipient, even
        // when no balance is transferred. Earlier definite touches persist.
        out.may_touch_all();
        // Empty/absent accounts can be deployment destinations. Protection is
        // justified by current nonempty code, not merely inclusion in a world.
        let stable: BTreeSet<_> = protected
            .iter()
            .copied()
            .filter(|address| match self.code(*address) {
                Some(Code::Runtime(program)) => program.byte_len() != 0,
                Some(Code::Delegation(_)) => true,
                _ => false,
            })
            .collect();
        out.persistent.havoc_except_checked(&stable, limits)?;
        out.transient.havoc_except_checked(&stable, limits)?;
        out.balances.clear();
        out.balance_default = crate::domain::EvmValue::<N>::top();
        let addresses = self.addresses();
        out.codes = OrderedMap::new();
        out.nonces = OrderedMap::new();
        out.existence = OrderedMap::new();
        out.created.clear();
        out.pending_destruction.clear();
        out.created_default = None;
        out.pending_destruction_default = None;
        for address in addresses.union(&stable).copied() {
            if stable.contains(&address) {
                out.codes
                    .insert(address, self.code(address).unwrap().clone());
                out.nonces.insert(address, self.nonce(address));
                out.existence.insert(address, self.existence(address));
                out.created
                    .insert(address, self.created_in_transaction(address));
                out.pending_destruction
                    .insert(address, self.pending_destruction(address));
            }
        }
        if let Some(owner) = writable_owner {
            out.havoc_storage_owner(owner);
            // CREATE executes with this owner's nonce, even in a delegate frame.
            out.nonces
                .insert(owner, crate::domain::EvmValue::<N>::top());
            // All supported forks defer EIP-6780 code deletion until the actual
            // transaction end. A created owner may selfdestruct in unknown code.
            if out.created_in_transaction(owner) != Some(false) {
                out.pending_destruction.insert(owner, None);
            }
        }
        out.logs_unknown = true;
        out.check_existing(limits)?;
        *self = out;
        Ok(())
    }
}
