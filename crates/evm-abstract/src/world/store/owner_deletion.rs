//! Union with deletion changes one owner, never the immutable facts it read.

use super::{Code, EvmStore, Existence, Plane, StoreError};
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::{AbstractValue, Domain};
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{ExprId, ExprLimits, SlotStateId, StateError},
};
#[cfg(test)]
use crate::world::Store;
use alloy_primitives::{Address, U256};

/// A cached old value and the separate cost of constructing its union with zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CleanupValue {
    pub(super) work: usize,
    pub(super) zero_union: usize,
    pub(super) new_entry: bool,
}

impl CleanupValue {
    fn new<N: WordElement>(
        value: &EvmValue<N>,
        new_entry: bool,
        union_work: impl Fn(usize) -> usize + Copy,
    ) -> Self {
        Self {
            work: value.work_size(),
            // collect_bounded inserts at most n + 1 candidates into a fresh
            // BTreeSet. This height belongs to that set, not to a Store map.
            zero_union: value
                .finite_constants()
                .cardinality()
                .map_or(0, |n| union_work(n.saturating_add(1))),
            new_entry,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlaneCleanupFootprint {
    pub(super) root_work: usize,
    pub(super) new_root: bool,
    pub(super) default: CleanupValue,
    pub(super) cells: usize,
    pub(super) cell_work: usize,
    pub(super) cell_unions: usize,
}

/// Discovery is part of the already prepaid Plan classification pass. Counts
/// describe map entries; cached weights describe retained values/graphs only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OwnerCleanupFootprint {
    pub(super) balance: CleanupValue,
    pub(super) nonce: CleanupValue,
    pub(super) new_code: bool,
    pub(super) new_existence: bool,
    pub(super) planes: [PlaneCleanupFootprint; 2],
}

impl OwnerCleanupFootprint {
    /// Exact projection of the old maybe_delete_fields receipt. Delete/mixed
    /// plans retain its old values and the complete legacy charging formula.
    pub(super) fn legacy_fields(self) -> usize {
        self.planes.into_iter().fold(
            128usize
                .saturating_add(self.balance.work)
                .saturating_add(self.nonce.work),
            |fields, plane| {
                fields
                    .saturating_add(plane.root_work)
                    .saturating_add(plane.default.work)
                    .saturating_add(plane.cell_work)
                    .saturating_add(plane.cells.saturating_mul(2))
            },
        )
    }

    pub(super) fn local_work(self) -> usize {
        self.planes.into_iter().fold(
            self.legacy_fields()
                .saturating_add(self.balance.zero_union)
                .saturating_add(self.nonce.zero_union),
            |work, plane| {
                work.saturating_add(plane.default.zero_union)
                    .saturating_add(plane.cell_unions)
            },
        )
    }
}

impl<N: WordElement> Plane<N> {
    fn cleanup_footprint(
        &self,
        address: Address,
        union_work: impl Fn(usize) -> usize + Copy,
    ) -> PlaneCleanupFootprint {
        let root = self.roots.get(&address);
        let default = self.defaults.get(&address);
        let mut footprint = PlaneCleanupFootprint {
            root_work: root.unwrap_or(&self.base).work_size(),
            new_root: root.is_none(),
            default: CleanupValue::new(
                default.unwrap_or(&self.global_default),
                default.is_none(),
                union_work,
            ),
            cells: 0,
            cell_work: 0,
            cell_unions: 0,
        };
        for (_, value) in self
            .slots
            .range((address, U256::ZERO)..=(address, U256::MAX))
        {
            let value = CleanupValue::new(value, false, union_work);
            footprint.cells = footprint.cells.saturating_add(1);
            footprint.cell_work = footprint.cell_work.saturating_add(value.work);
            footprint.cell_unions = footprint.cell_unions.saturating_add(value.zero_union);
        }
        footprint
    }

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

    fn join_deleted_numeric<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        domain: AlgebraRef<'_, D>,
    ) {
        let zero_with = EvmValue::<N>::constant(U256::ZERO);
        let default = domain.join(self.default_at(address), &zero_with);
        self.slots
            .update_range((address, U256::ZERO)..=(address, U256::MAX), |_, value| {
                *value = domain.join(value, &zero_with);
            });
        self.defaults.insert(address, default);
        // Reset's independence is true, so old AND reset is exactly old.
        // Operand facts constrain immutable historical expressions, not just
        // current cells. Deletion does not invalidate any of those facts.
    }
}

impl<N: WordElement> EvmStore<N> {
    // Finalization supplies the finite-union price during the existing scan;
    // owner execution does not depend on its sibling's pricing implementation.
    pub(super) fn owner_cleanup_footprint(
        &self,
        address: Address,
        union_work: impl Fn(usize) -> usize + Copy,
    ) -> OwnerCleanupFootprint {
        let balance = self.balances.get(&address);
        let nonce = self.nonces.get(&address);
        OwnerCleanupFootprint {
            balance: CleanupValue::new(
                balance.unwrap_or(&self.balance_default),
                balance.is_none(),
                union_work,
            ),
            nonce: CleanupValue::new(
                nonce.unwrap_or(&EvmValue::<N>::top()),
                nonce.is_none(),
                union_work,
            ),
            new_code: self.codes.get(&address).is_none(),
            new_existence: self.existence.get(&address).is_none(),
            planes: [
                self.persistent.cleanup_footprint(address, union_work),
                self.transient.cleanup_footprint(address, union_work),
            ],
        }
    }

    fn join_deleted_fields<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        domain: AlgebraRef<'_, D>,
    ) {
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
    fn join_deleted_guarded<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        domain: AlgebraRef<'_, D>,
        selector: ExprId,
    ) -> Result<(), StoreError> {
        let limits = domain.spec().relations().expressions();
        let zero_with = EvmValue::<N>::constant(U256::ZERO);
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
        let balance = balance.join_guarded_with(&zero_with, domain, selector.clone(), limits)?;
        let nonce = nonce.join_guarded_with(&zero_with, domain, selector, limits)?;

        self.join_deleted_fields(address, domain);
        self.persistent.roots.insert(address, persistent);
        self.transient.roots.insert(address, transient);
        self.balances.insert(address, balance);
        self.nonces.insert(address, nonce);
        Ok(())
    }

    pub(super) fn maybe_delete_account_with<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        domain: AlgebraRef<'_, D>,
    ) {
        let result = ExprId::fresh()
            .ok_or(StoreError::State(StateError::IdentityExhausted))
            .and_then(|selector| self.join_deleted_guarded(address, domain, selector));
        if let Err(error) = result {
            // Preserve the unmetered API's numerical upper fallback and visible
            // failure. Checked finalization rejects it before installing state.
            let zero_with = EvmValue::<N>::constant(U256::ZERO);
            let balance = domain.join(&self.read_balance(address), &zero_with);
            let nonce = domain.join(&self.nonce(address), &zero_with);
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
