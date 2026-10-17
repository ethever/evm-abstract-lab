//! Union with deletion changes one owner, never the immutable facts it read.

use super::{Code, Existence, Plane, Store, StoreError};
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{ExprId, ExprLimits, SlotStateId, StateError},
};
use alloy_primitives::{Address, U256};

impl Plane {
    fn deleted_root(
        &self,
        address: Address,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<SlotStateId, StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        // Check the old alternative before numeric joining can discard an
        // oversized optional expression. Unchanged fields are checked at install.
        for value in self
            .slots
            .range((address, U256::ZERO)..=(address, U256::MAX))
            .map(|(_, value)| value)
            .chain(std::iter::once(self.default_at(address)))
        {
            crate::world::bytes::check_value(value, limits)?;
        }
        Ok(self
            .root_at(address)?
            .join_guarded(&SlotStateId::zero(), selector, limits.into())?)
    }

    fn join_deleted_numeric(&mut self, address: Address, domain: Domain) {
        let zero = AbstractValue::constant(U256::ZERO);
        let default = domain.join(self.default_at(address), &zero);
        self.slots
            .update_range((address, U256::ZERO)..=(address, U256::MAX), |_, value| {
                *value = domain.join(value, &zero);
            });
        self.defaults.insert(address, default);
        // Reset's independence is true, so old AND reset is exactly old.
        // Operand facts constrain immutable historical expressions, not just
        // current cells. Deletion does not invalidate any of those facts.
    }
}

impl Store {
    /// Cached graph/value sizes for only the fields visited by one MAY action.
    /// The fixed allowance covers owner keys, zero alternatives and ITE nodes,
    /// including materializing roots/defaults/scalars for an implicit owner.
    pub(super) fn maybe_delete_fields(&self, address: Address) -> usize {
        [&self.persistent, &self.transient].into_iter().fold(
            128usize
                .saturating_add(
                    self.balances
                        .get(&address)
                        .unwrap_or(&self.balance_default)
                        .work_size(),
                )
                .saturating_add(self.nonces.get(&address).map_or_else(
                    || AbstractValue::top().work_size(),
                    AbstractValue::work_size,
                )),
            |fields, plane| {
                plane
                    .slots
                    .range((address, U256::ZERO)..=(address, U256::MAX))
                    .fold(
                        fields
                            .saturating_add(
                                plane.roots.get(&address).unwrap_or(&plane.base).work_size(),
                            )
                            .saturating_add(plane.default_at(address).work_size()),
                        |fields, (_, value)| {
                            fields.saturating_add(value.work_size()).saturating_add(2)
                        },
                    )
            },
        )
    }

    fn join_deleted_fields(&mut self, address: Address, domain: Domain) {
        self.persistent.join_deleted_numeric(address, domain);
        self.transient.join_deleted_numeric(address, domain);
        let code = if matches!(self.code(address), Some(Code::Empty)) {
            Code::Empty
        } else {
            Code::Unknown
        };
        self.codes.insert(address, code);
        let existence = if self.existence(address) == Existence::Absent {
            Existence::Absent
        } else {
            Existence::Unknown
        };
        self.existence.insert(address, existence);
    }

    /// Same old/deleted alternatives as Store::join_guarded, with one selector
    /// for both planes and scalars. All fallible preparation precedes mutation.
    fn join_deleted_guarded(
        &mut self,
        address: Address,
        domain: Domain,
        selector: ExprId,
    ) -> Result<(), StoreError> {
        let limits = domain.spec().relations().expressions();
        let zero = AbstractValue::constant(U256::ZERO);
        let balance = self.read_balance(address);
        let nonce = self.nonce(address);
        crate::world::bytes::check_value(&balance, limits)?;
        crate::world::bytes::check_value(&nonce, limits)?;
        let persistent = self
            .persistent
            .deleted_root(address, selector.clone(), limits)?;
        let transient = self
            .transient
            .deleted_root(address, selector.clone(), limits)?;
        let balance = balance.join_guarded(&zero, domain, selector.clone(), limits)?;
        let nonce = nonce.join_guarded(&zero, domain, selector, limits)?;

        self.join_deleted_fields(address, domain);
        self.persistent.roots.insert(address, persistent);
        self.transient.roots.insert(address, transient);
        self.balances.insert(address, balance);
        self.nonces.insert(address, nonce);
        Ok(())
    }

    pub(super) fn maybe_delete_account(&mut self, address: Address, domain: Domain) {
        let result = ExprId::fresh()
            .ok_or(StoreError::State(StateError::IdentityExhausted))
            .and_then(|selector| self.join_deleted_guarded(address, domain, selector));
        if let Err(error) = result {
            // Preserve the unmetered API's numerical upper fallback and visible
            // failure. Checked finalization rejects it before installing state.
            let zero = AbstractValue::constant(U256::ZERO);
            let balance = domain.join(&self.read_balance(address), &zero);
            let nonce = domain.join(&self.nonce(address), &zero);
            self.join_deleted_fields(address, domain);
            self.balances.insert(address, balance);
            self.nonces.insert(address, nonce);
            self.persistent.failure = Some(error.clone());
            self.transient.failure = Some(error);
        }
    }
}

#[cfg(test)]
mod tests;
