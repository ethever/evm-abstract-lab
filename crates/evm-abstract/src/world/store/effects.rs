//! Capabilities of unknown execution, separated from known-owner callbacks.

use super::{InitialIndependence, OrderedMap, Plane, Store, StoreError};
use crate::{domain::Domain, world::Code};
use alloy_primitives::Address;
use std::collections::BTreeSet;

impl Plane {
    fn havoc_except(&mut self, protected: &BTreeSet<Address>) -> Result<(), StoreError> {
        let previous = self.clone();
        self.havoc_all();
        // Retained roots can refer to operands and snapshot sources that are not
        // visible in the sparse cell cache. Keep their complete interpretation.
        self.operands = previous.operands.clone();
        for address in protected {
            self.roots.insert(*address, previous.root_at(*address)?);
            self.defaults
                .insert(*address, previous.default_at(*address).clone());
        }
        self.slots.extend(
            previous
                .slots
                .iter()
                .filter(|((owner, _), _)| protected.contains(owner))
                .map(|(key, value)| (*key, value.clone())),
        );
        self.initial_independence = InitialIndependence::new(true);
        for address in protected {
            let independent = previous.initial_independence.default_at(*address);
            if !independent {
                self.initial_independence.defaults.insert(*address, false);
            }
        }
        for ((owner, key), independent) in &previous.initial_independence.slots {
            if protected.contains(owner) {
                self.initial_independence
                    .set_slot(*owner, *key, *independent);
            }
        }
        Ok(())
    }
}

impl Store {
    /// Unknown-world effects under the selected transaction lifecycle rules.
    /// Every supported fork keeps SELFDESTRUCT code visible until finalization.
    pub fn apply_unknown_effects(
        &mut self,
        protected: &BTreeSet<Address>,
        delegated_owner: Option<Address>,
        is_static: bool,
        fork: crate::Fork,
    ) -> Result<(), StoreError> {
        match fork {
            crate::Fork::Cancun | crate::Fork::Prague | crate::Fork::Osaka => {
                self.unknown_effects(protected, delegated_owner, is_static, Domain::default())
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
    pub(crate) fn unknown_effects(
        &mut self,
        protected: &BTreeSet<Address>,
        writable_owner: Option<Address>,
        is_static: bool,
        domain: Domain,
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
        out.persistent.havoc_except(&stable)?;
        out.transient.havoc_except(&stable)?;
        out.balances.clear();
        out.balance_default = crate::domain::AbstractValue::top();
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
                .insert(owner, crate::domain::AbstractValue::top());
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
