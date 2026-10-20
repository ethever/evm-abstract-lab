//! Finite transaction-end planning, independent of deployed bytecode length.
//!
//! Admission pays for discovering the plan as well as executing it. The staged
//! Store is installed only after all work is reserved and cancellation checked.
//! The ordinary Store API uses the same classification and execution order.

use super::{
    Code, EvmSnapshot, EvmStore, StoreError, lifecycle, owner_deletion::OwnerCleanupFootprint,
};
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::Domain;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::resource::WorkBudget;
#[cfg(test)]
use crate::world::Store;
use alloy_primitives::Address;

mod preparation;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FinalizationError {
    #[error("transaction finalization work exhausted")]
    Work,
    #[error("transaction finalization cancelled")]
    Cancelled,
    #[error("{0}")]
    Store(StoreError),
}

impl From<StoreError> for FinalizationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl FinalizationError {
    pub(crate) fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub(crate) fn into_store_error(self) -> Option<StoreError> {
        match self {
            Self::Store(error) => Some(error),
            Self::Work | Self::Cancelled => None,
        }
    }
}

fn check_cancelled() -> Result<(), FinalizationError> {
    if analysis_control::current_cancellation().is_cancelled() {
        Err(FinalizationError::Cancelled)
    } else {
        Ok(())
    }
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), FinalizationError> {
    check_cancelled()?;
    if budget.charge(work) {
        Ok(())
    } else {
        check_cancelled()?;
        Err(FinalizationError::Work)
    }
}

/// Upper bound on ordered-container steps, including empty containers.
fn tree_work(entries: usize) -> usize {
    entries
        .saturating_add(1)
        .saturating_mul(tree_height(entries))
}

fn tree_height(entries: usize) -> usize {
    (usize::BITS - entries.leading_zeros()) as usize + 1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deletion {
    Retain,
    Delete,
    Join,
}

impl Deletion {
    fn at<N: WordElement>(store: &EvmStore<N>, address: Address) -> Self {
        let pending = store.pending_destruction(address);
        // Pending destruction is independent of code emptiness. In particular,
        // EIP-6780 leaves created accounts' code visible until this point.
        let empty_touch = if pending == Some(true) {
            Some(false)
        } else {
            let touched = store.touched_in_transaction(address);
            if touched == Some(false) {
                Some(false)
            } else {
                lifecycle::and(touched, store.is_empty(address))
            }
        };
        match lifecycle::or(pending, empty_touch) {
            Some(false) => Self::Retain,
            Some(true) => Self::Delete,
            None => Self::Join,
        }
    }
}

pub(super) struct Plan {
    actions: Vec<(Address, Deletion)>,
    fee: PlanFee,
    removed_code_bytes: usize,
}

enum PlanFee {
    Legacy { join_fields: usize },
    NoDelete { owners: Vec<OwnerCleanupFootprint> },
}

impl Plan {
    pub(super) fn new<N: WordElement>(store: &EvmStore<N>) -> Self {
        let mut actions = Vec::new();
        let mut owners = Vec::new();
        let mut has_delete = false;
        let mut join_fields = 0usize;
        let mut removed_code_bytes = 0usize;
        for address in store.addresses() {
            let action = Deletion::at(store, address);
            if action == Deletion::Join {
                let owner = store.owner_cleanup_footprint(address, tree_work);
                join_fields = join_fields.saturating_add(owner.legacy_fields());
                owners.push(owner);
            }
            has_delete |= action == Deletion::Delete;
            if action != Deletion::Retain
                && let Some(Code::Runtime(program)) = store.code(address)
            {
                removed_code_bytes = removed_code_bytes.saturating_add(program.byte_len());
            }
            actions.push((address, action));
        }
        Self {
            actions,
            fee: if has_delete {
                PlanFee::Legacy { join_fields }
            } else {
                PlanFee::NoDelete { owners }
            },
            removed_code_bytes,
        }
    }

    fn work(&self, footprint: Footprint) -> usize {
        match &self.fee {
            PlanFee::Legacy { join_fields } => self.legacy_work(footprint, *join_fields),
            PlanFee::NoDelete { owners } => self.no_delete_work(footprint, owners).total(),
        }
    }

    fn legacy_work(&self, footprint: Footprint, join_fields: usize) -> usize {
        let deletes = self
            .actions
            .iter()
            .filter(|(_, action)| *action == Deletion::Delete)
            .count();
        let joins = self
            .actions
            .iter()
            .filter(|(_, action)| *action == Deletion::Join)
            .count();

        // Definite resets can add owner roots/defaults/independence entries.
        // MAY growth is bounded by its own old fields plus fixed ITE/key/zero
        // metadata. No other owner's defaults/scalars are materialized.
        let grown = footprint
            .fields
            .saturating_add(join_fields)
            .saturating_add(deletes.saturating_mul(footprint.reset_growth));

        // Four cleanup/drop passes; sixteen per definite reset retain the old
        // bound for its four whole-map retain loops and ordered-map updates.
        // MAY actions never clone the Store. Eight shared passes cover the
        // first MAY input validation, first-write COW node/value copies and
        // their drops. Private map versions stay live for subsequent updates.
        let passes = 4usize
            .saturating_add(deletes.saturating_mul(16))
            .saturating_add(if joins == 0 { 0 } else { 8 });
        tree_work(grown)
            .saturating_mul(passes)
            // Sixteen local passes cover old-value validation, two plane root
            // preparations, numeric cell/default joins, scalar guarded joins,
            // selected-key collection, map updates and failed-preparation drops.
            // Graph/value sizes and the grown map height bound those operations;
            // unrelated fields are paid once above, not once per MAY owner.
            .saturating_add(
                join_fields
                    .saturating_mul(tree_height(grown))
                    .saturating_mul(16),
            )
            // Only removed Program handles can release decoded code. There are
            // no whole-Store equality checks or runtime-byte comparisons here.
            .saturating_add(self.removed_code_bytes.saturating_mul(4))
    }

    fn no_delete_work(
        &self,
        footprint: Footprint,
        owners: &[OwnerCleanupFootprint],
    ) -> NoDeleteWork {
        let mut receipt = NoDeleteWork {
            // Empty-plane/flag setup and action iteration/interrupt/consumption.
            setup: 32usize.saturating_add(self.actions.len().saturating_mul(2)),
            validation: if owners.is_empty() {
                0
            } else {
                footprint.validation_work
            },
            code_drop: self.removed_code_bytes.saturating_mul(4),
            ..NoDeleteWork::default()
        };
        let mut additions = [0usize; CleanupMap::COUNT];
        let mut touched = TouchedMaps::default();
        for owner in owners {
            receipt.local = receipt
                .local
                .saturating_add(owner.local_work().saturating_mul(16));
            // A late guarded preparation may fail, then use the ordinary
            // numeric fallback. Across both paths: <= 12 gets, 8 inserts.
            for (map, gets, new_entry) in [
                (CleanupMap::Balances, 2usize, owner.balance.new_entry),
                (CleanupMap::Nonces, 2, owner.nonce.new_entry),
                (CleanupMap::Codes, 1, owner.new_code),
                (CleanupMap::Existence, 1, owner.new_existence),
                (CleanupMap::PersistentRoots, 1, owner.planes[0].new_root),
                (CleanupMap::TransientRoots, 1, owner.planes[1].new_root),
                (
                    CleanupMap::PersistentDefaults,
                    2,
                    owner.planes[0].default.new_entry,
                ),
                (
                    CleanupMap::TransientDefaults,
                    2,
                    owner.planes[1].default.new_entry,
                ),
            ] {
                let index = map as usize;
                additions[index] = additions[index].saturating_add(usize::from(new_entry));
                let entries = footprint.maps[index]
                    .entries
                    .saturating_add(additions[index]);
                receipt.maps = receipt.maps.saturating_add(
                    gets.saturating_add(1)
                        .saturating_mul(tree_height(entries))
                        .saturating_mul(4),
                );
                touched.insert(map);
            }
            for (plane, map) in owner
                .planes
                .iter()
                .zip([CleanupMap::PersistentSlots, CleanupMap::TransientSlots])
            {
                let height = tree_height(footprint.maps[map as usize].entries);
                // One old-value range and one numeric-update range, plus imbl's
                // selected-key collection/get_mut bound. This shared logical
                // upper bound keeps Work backend-independent; std may perform
                // fewer physical steps.
                let mut ranges = height.saturating_add(plane.cells).saturating_mul(2);
                ranges =
                    ranges.saturating_add(plane.cells.saturating_mul(height.saturating_add(1)));
                receipt.maps = receipt.maps.saturating_add(ranges.saturating_mul(4));
                if plane.cells != 0 {
                    touched.insert(map);
                }
            }
        }
        receipt.cow = touched.cow_work(&footprint.maps);
        receipt
    }

    pub(super) fn apply<E, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        self,
        store: &mut EvmStore<N>,
        domain: AlgebraRef<'_, D>,
        interrupt: &mut impl FnMut() -> Result<(), E>,
    ) -> Result<(), E> {
        let mut checked_join_input = false;
        for (address, action) in self.actions {
            interrupt()?;
            // Both updates affect only this owner; other classifications stay
            // valid and their roots/metadata retain their existing identities.
            match action {
                Deletion::Retain => {}
                Deletion::Delete => store.delete_account(address),
                Deletion::Join => {
                    if !checked_join_input {
                        // Preserve existing refusal of invalid old alternatives,
                        // including transient facts cleared at transaction end.
                        // Later MAY preparations validate their changed fields;
                        // final checked installation validates the complete result.
                        if let Err(error) =
                            store.check_existing(domain.spec().relations().expressions())
                        {
                            store.persistent.failure = Some(error.clone());
                            store.transient.failure = Some(error);
                        }
                        checked_join_input = true;
                    }
                    store.maybe_delete_account_with(address, domain);
                }
            }
        }
        interrupt()?;
        store.clear_transaction();
        interrupt()
    }
}

#[derive(Clone, Copy)]
enum CleanupMap {
    Balances,
    Nonces,
    Codes,
    Existence,
    PersistentRoots,
    PersistentDefaults,
    PersistentSlots,
    TransientRoots,
    TransientDefaults,
    TransientSlots,
}

impl CleanupMap {
    const COUNT: usize = 10;
}

/// Cardinality controls map height; clone_units controls key/value copying.
/// A retained DAG's expanded node count must never inflate map cardinality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MapFootprint {
    entries: usize,
    clone_units: usize,
}

impl MapFootprint {
    fn new(entries: usize, key_handles: usize) -> Self {
        Self {
            entries,
            clone_units: entries.saturating_mul(key_handles),
        }
    }
}

#[derive(Default)]
struct TouchedMaps(u16);

impl TouchedMaps {
    fn insert(&mut self, map: CleanupMap) {
        self.0 |= 1 << map as usize;
    }

    fn cow_work(&self, maps: &[MapFootprint; CleanupMap::COUNT]) -> usize {
        // Use one conservative logical policy so report Work is independent of
        // the backend. Existing staging already pays std map cloning; reserving
        // the same first-write bound here is deliberate.
        maps.iter()
            .enumerate()
            .filter(|(index, map)| self.0 & (1u16 << *index) != 0 && map.entries != 0)
            .fold(0usize, |work, (_, map)| {
                work.saturating_add(
                    tree_work(map.entries)
                        .saturating_add(map.clone_units)
                        .saturating_mul(4),
                )
            })
        // apply holds one private map version, with no intermediate snapshots:
        // each original shared node can be copied once. New-node splits/searches
        // and new-content drops belong to the per-owner local/map receipts.
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct NoDeleteWork {
    setup: usize,
    validation: usize,
    cow: usize,
    local: usize,
    maps: usize,
    code_drop: usize,
}

impl NoDeleteWork {
    fn total(&self) -> usize {
        self.setup
            .saturating_add(self.validation)
            .saturating_add(self.cow)
            .saturating_add(self.local)
            .saturating_add(self.maps)
            .saturating_add(self.code_drop)
    }
}

/// Expanded retained fields, separate from account count and raw code bytes.
/// Root/AbstractValue work sizes are cached; discovering them visits entries,
/// not their graphs. Log work_size also walks a graph, prepaid separately below.
#[derive(Clone, Copy)]
struct Footprint {
    fields: usize,
    code_bytes: usize,
    reset_growth: usize,
    maps: [MapFootprint; CleanupMap::COUNT],
    validation_work: usize,
}

impl<N: WordElement> EvmStore<N> {
    /// O(1) container lengths only. This includes every map visited by planning,
    /// cloning, deletion, joins and cleanup, not merely the account maps.
    pub(crate) fn finalization_entries(&self) -> usize {
        [&self.persistent, &self.transient].into_iter().fold(
            self.balances
                .len()
                .saturating_add(self.codes.len())
                .saturating_add(self.nonces.len())
                .saturating_add(self.existence.len())
                .saturating_add(self.created.len())
                .saturating_add(self.pending_destruction.len())
                .saturating_add(self.touched.len())
                .saturating_add(self.possible_logs.len().saturating_mul(2))
                .saturating_add(16),
            |entries, plane| {
                entries
                    .saturating_add(plane.roots.len())
                    .saturating_add(plane.slots.len())
                    .saturating_add(plane.defaults.len())
                    .saturating_add(plane.operands.len())
                    .saturating_add(plane.initial_independence.slots.len().saturating_mul(3))
                    .saturating_add(plane.initial_independence.defaults.len().saturating_mul(2))
            },
        )
    }

    /// Finish a root transaction atomically under the shared work ledger.
    /// Return uses the live Store; Revert/Failure supply the root checkpoint.
    /// Restoration precedes planning so its sticky RIPEMD touch is included.
    pub(crate) fn finalize_transaction_checked_with<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
        rollback: Option<&EvmSnapshot<N>>,
        budget: &mut WorkBudget,
    ) -> Result<(), FinalizationError> {
        let current = Footprint::measure(self, budget)?;
        let saved = match rollback {
            Some(snapshot) => Footprint::measure(snapshot.state(), budget)?,
            None => current,
        };
        // Reserve staging, checkpoint restoration, replacement and all drops,
        // including discarding the staged Store on later admission/cancellation.
        // The possible RIPEMD touch entry also needs a COW tree update.
        reserve(
            budget,
            tree_work(
                current
                    .fields
                    .saturating_add(saved.fields)
                    .saturating_add(2),
            )
            .saturating_mul(8)
            .saturating_add(if rollback.is_some() {
                current.code_bytes.saturating_mul(4)
            } else {
                0
            }),
        )?;
        let mut staged = self.clone();
        if let Some(snapshot) = rollback {
            staged.restore(snapshot.clone());
        }
        staged.prepare_unlisted_finalization_checked_with(domain, budget)?;
        let footprint = Footprint::measure(&staged, budget)?;
        // Pay address-set construction, lookup/classification and plan disposal
        // before visiting any address; values remain borrowed during emptiness.
        reserve(budget, tree_work(footprint.fields).saturating_mul(4))?;
        let plan = Plan::new(&staged);
        reserve(budget, plan.work(footprint))?;
        plan.apply(&mut staged, domain, &mut check_cancelled)?;
        // MAY deletion preserves a numerical upper fallback when retaining its
        // complete symbolic state fails. That is not a completed transaction.
        // Discover and reserve validation work before checking the staged state;
        // refusal or a typed retention error leaves the live Store unchanged.
        let validation = Footprint::measure(&staged, budget)?;
        reserve(budget, tree_work(validation.fields).saturating_mul(4))?;
        staged.check_existing(domain.spec().relations().expressions())?;
        *self = staged;
        Ok(())
    }
}

impl Footprint {
    fn measure<N: WordElement>(
        store: &EvmStore<N>,
        budget: &mut WorkBudget,
    ) -> Result<Self, FinalizationError> {
        let entries = store.finalization_entries();
        // Two passes cover outer metadata discovery and temporary traversal
        // storage. No uncharged work_size()/addresses() scan discovers this fee.
        reserve(budget, tree_work(entries).saturating_mul(2))?;
        let mut maps = [MapFootprint::default(); CleanupMap::COUNT];
        let mut validation_work = tree_work(entries);
        let mut fields = entries.saturating_add(store.balance_default.work_size());
        for (kind, map) in [
            (CleanupMap::Balances, &store.balances),
            (CleanupMap::Nonces, &store.nonces),
        ] {
            let receipt = &mut maps[kind as usize];
            *receipt = MapFootprint::new(map.len(), 1);
            for value in map.values() {
                let work = value.work_size();
                fields = fields.saturating_add(work);
                receipt.clone_units = receipt.clone_units.saturating_add(work);
            }
        }
        maps[CleanupMap::Codes as usize] = MapFootprint::new(store.codes.len(), 2);
        maps[CleanupMap::Existence as usize] = MapFootprint::new(store.existence.len(), 2);
        let mut reset_growth = 128usize;
        for (plane, roots, defaults, slots) in [
            (
                &store.persistent,
                CleanupMap::PersistentRoots,
                CleanupMap::PersistentDefaults,
                CleanupMap::PersistentSlots,
            ),
            (
                &store.transient,
                CleanupMap::TransientRoots,
                CleanupMap::TransientDefaults,
                CleanupMap::TransientSlots,
            ),
        ] {
            maps[roots as usize] = MapFootprint::new(plane.roots.len(), 2);
            maps[defaults as usize] = MapFootprint::new(plane.defaults.len(), 1);
            maps[slots as usize] = MapFootprint::new(plane.slots.len(), 2);
            reset_growth = reset_growth
                .saturating_add(plane.base.work_size())
                .saturating_add(plane.global_default.work_size());
            fields = fields
                .saturating_add(plane.base.work_size())
                .saturating_add(plane.global_default.work_size());
            // Store::visit_values calls slots::expressions separately for every
            // root occurrence, each with its own ordered visited set. Expanded
            // cached nodes bound that walk; the check itself is not O(1).
            validation_work = validation_work.saturating_add(tree_work(plane.base.work_size()));
            for root in plane.roots.values() {
                fields = fields.saturating_add(root.work_size());
                validation_work = validation_work.saturating_add(tree_work(root.work_size()));
            }
            for value in plane.slots.values() {
                let work = value.work_size();
                fields = fields.saturating_add(work);
                maps[slots as usize].clone_units =
                    maps[slots as usize].clone_units.saturating_add(work);
            }
            for value in plane.defaults.values() {
                let work = value.work_size();
                fields = fields.saturating_add(work);
                maps[defaults as usize].clone_units =
                    maps[defaults as usize].clone_units.saturating_add(work);
            }
            for (expression, value) in &plane.operands {
                fields = fields
                    .saturating_add(expression.work_size())
                    .saturating_add(value.work_size());
            }
        }
        let mut code_bytes = 0usize;
        for code in store.codes.values() {
            if let Code::Runtime(program) = code {
                code_bytes = code_bytes.saturating_add(program.byte_len());
            }
        }
        for log in store.possible_logs.values() {
            check_cancelled()?;
            let scan = log
                .data
                .metadata_len()
                .saturating_add(log.data.state_root().work_size())
                .saturating_add(log.topics.len());
            reserve(budget, tree_work(scan).saturating_mul(2))?;
            let data_work = log.data.work_size();
            fields = fields.saturating_add(data_work);
            // The already prepaid log walk supplies this bound; do not run
            // another DAG census to discover the initial validation charge.
            validation_work = validation_work
                .saturating_add(tree_work(data_work.saturating_add(log.topics.len())));
            for topic in &log.topics {
                fields = fields.saturating_add(topic.work_size());
            }
        }
        check_cancelled()?;
        Ok(Self {
            fields,
            code_bytes,
            reset_growth,
            maps,
            // Duplicate value visits, root discovery/visited sets, log checks
            // and their temporary disposal are bounded by four allowances.
            validation_work: validation_work.saturating_mul(4),
        })
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod unlisted;

#[cfg(test)]
impl EvmStore<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn finalize_transaction_checked(
        &mut self,
        domain: crate::domain::Domain,
        rollback: Option<&EvmSnapshot<crate::domain::NumericValue>>,
        budget: &mut WorkBudget,
    ) -> Result<(), FinalizationError> {
        self.finalize_transaction_checked_with(
            crate::domain::ValueAlgebra::new(domain).borrowed(),
            rollback,
            budget,
        )
    }
}
