//! Prepay omitted-owner persistent cleanup without weakening explicit owners.

use super::{FinalizationError, reserve, tree_height, tree_work};
use crate::{
    domain::{AbstractValue, Domain},
    resource::WorkBudget,
    world::store::{Plane, Store, StoreError},
};
use alloy_primitives::Address;
use std::collections::BTreeSet;

/// Copy/drop of owned scalar metadata; expression/source handles stay shared.
fn value_clone_units(value: &AbstractValue) -> usize {
    value
        .numeric()
        .work_size()
        .saturating_add(value.provenance().origins().work_size())
        .saturating_add(4)
}

#[derive(Debug, PartialEq, Eq)]
struct PreparationWork {
    setup: usize,
    maps: usize,
    values: usize,
    scoping: usize,
}

impl PreparationWork {
    fn discovery(entries: usize) -> usize {
        // Two allowances build/dispose addresses(); four admit the quote's
        // owner/root/default lookups, membership tests and scalar metadata scan.
        tree_work(entries).saturating_mul(6)
    }

    fn measure(plane: &Plane, protected: &BTreeSet<Address>) -> Self {
        let owners = protected.len();
        let owner_height = tree_height(owners);
        let slots = plane.slots.len();
        let independent_slots = plane.initial_independence.slots.len();
        let mut scoped = 0usize;
        let mut values = 0usize;
        for owner in protected {
            scoped = scoped.saturating_add(usize::from(plane.roots.get(owner).is_none()));
            values = values.saturating_add(value_clone_units(plane.default_at(*owner)));
        }
        for ((owner, _), value) in &plane.slots {
            if protected.contains(owner) {
                values = values.saturating_add(value_clone_units(value));
            }
        }
        for value in plane.operands.values() {
            values = values.saturating_add(value_clone_units(value));
        }
        // Three old-owner queries; roots/defaults/independence each have one
        // new insertion and one disposal. Sparse cells add membership + update
        // + disposal, and independence.set_slot also queries its new default.
        let maps = owners
            .saturating_mul(
                tree_height(plane.roots.len())
                    .saturating_add(tree_height(plane.defaults.len()))
                    .saturating_add(tree_height(plane.initial_independence.defaults.len()))
                    .saturating_add(owner_height.saturating_mul(6)),
            )
            .saturating_add(
                slots.saturating_mul(
                    owner_height.saturating_add(tree_height(slots).saturating_mul(2)),
                ),
            )
            .saturating_add(
                independent_slots.saturating_mul(
                    owner_height
                        .saturating_mul(2)
                        .saturating_add(tree_height(independent_slots).saturating_mul(2)),
                ),
            )
            // BTreeMap clone/drop includes its expression keys as handles.
            .saturating_add(tree_work(plane.operands.len()).saturating_mul(2));
        Self {
            // Fresh identity, checked input root, Top and empty containers.
            setup: 32,
            maps,
            // One owned copy and one success/refusal disposal. Prior staging
            // already funds destruction of the old private Plane's containers.
            values: values.saturating_mul(2),
            // root_at recursively scopes the base for EACH missing owner.
            // Source/ITE construction, checked joins, local interning and
            // temporary drops cost <=16 per expanded node; shared root-handle
            // cloning/comparison never traverses an existing owner DAG.
            scoping: scoped
                .saturating_mul(plane.base.work_size())
                .saturating_mul(16),
        }
    }

    fn total(&self) -> usize {
        self.setup
            .saturating_add(self.maps)
            .saturating_add(self.values)
            .saturating_add(self.scoping)
    }
}

impl Store {
    fn needs_unlisted_finalization(&self) -> bool {
        self.touched_default != Some(false) || self.pending_destruction_default != Some(false)
    }

    fn prepare_unlisted_owners(
        &mut self,
        domain: Domain,
        protected: &BTreeSet<Address>,
    ) -> Result<(), StoreError> {
        // Preserve the complete explicit set, including owners found only in
        // another plane, lifecycle metadata or either log-site address role.
        // Historical operands retain their original sources; only the current
        // omitted-owner persistent base becomes independent. Transient storage
        // is cleared by the original transaction cleanup instead.
        self.persistent
            .havoc_except_checked(protected, domain.spec().relations().expressions())
    }

    pub(in crate::world::store) fn prepare_unlisted_finalization(
        &mut self,
        domain: Domain,
    ) -> Result<(), StoreError> {
        if self.needs_unlisted_finalization() {
            let protected = self.addresses();
            self.prepare_unlisted_owners(domain, &protected)?;
        }
        Ok(())
    }

    pub(super) fn prepare_unlisted_finalization_checked(
        &mut self,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<(), FinalizationError> {
        if !self.needs_unlisted_finalization() {
            return Ok(());
        }
        reserve(
            budget,
            PreparationWork::discovery(self.finalization_entries()),
        )?;
        let protected = self.addresses();
        let work = PreparationWork::measure(&self.persistent, &protected);
        reserve(budget, work.total())?;
        self.prepare_unlisted_owners(domain, &protected)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
