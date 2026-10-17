//! Finite transaction-end planning, independent of deployed bytecode length.
//!
//! Admission pays for discovering the plan as well as executing it. The staged
//! Store is installed only after all work is reserved and cancellation checked.
//! The ordinary Store API uses the same classification and execution order.

use super::{Code, Snapshot, Store, StoreError, lifecycle};
use crate::{domain::Domain, resource::WorkBudget};
use alloy_primitives::Address;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FinalizationError {
    #[error("transaction finalization work exhausted")]
    Work,
    #[error("transaction finalization cancelled")]
    Cancelled,
    #[error(transparent)]
    Store(#[from] StoreError),
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
    fn at(store: &Store, address: Address) -> Self {
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
    join_fields: usize,
    removed_code_bytes: usize,
}

impl Plan {
    pub(super) fn new(store: &Store) -> Self {
        let mut actions = Vec::new();
        let mut join_fields = 0usize;
        let mut removed_code_bytes = 0usize;
        for address in store.addresses() {
            let action = Deletion::at(store, address);
            if action == Deletion::Join {
                join_fields = join_fields.saturating_add(store.maybe_delete_fields(address));
            }
            if action != Deletion::Retain
                && let Some(Code::Runtime(program)) = store.code(address)
            {
                removed_code_bytes = removed_code_bytes.saturating_add(program.byte_len());
            }
            actions.push((address, action));
        }
        Self {
            actions,
            join_fields,
            removed_code_bytes,
        }
    }

    fn work(&self, footprint: Footprint) -> usize {
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
            .saturating_add(self.join_fields)
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
                self.join_fields
                    .saturating_mul(tree_height(grown))
                    .saturating_mul(16),
            )
            // Only removed Program handles can release decoded code. There are
            // no whole-Store equality checks or runtime-byte comparisons here.
            .saturating_add(self.removed_code_bytes.saturating_mul(4))
    }

    pub(super) fn apply(
        self,
        store: &mut Store,
        domain: Domain,
        interrupt: &mut impl FnMut() -> Result<(), FinalizationError>,
    ) -> Result<(), FinalizationError> {
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
                    store.maybe_delete_account(address, domain);
                }
            }
        }
        interrupt()?;
        store.clear_transaction();
        interrupt()
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
}

impl Store {
    /// O(1) container lengths only. This includes every map visited by planning,
    /// cloning, deletion, joins and cleanup, not merely the account maps.
    fn finalization_entries(&self) -> usize {
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
    pub(crate) fn finalize_transaction_checked(
        &mut self,
        domain: Domain,
        rollback: Option<&Snapshot>,
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
    fn measure(store: &Store, budget: &mut WorkBudget) -> Result<Self, FinalizationError> {
        let entries = store.finalization_entries();
        // Two passes cover outer metadata discovery and temporary traversal
        // storage. No uncharged work_size()/addresses() scan discovers this fee.
        reserve(budget, tree_work(entries).saturating_mul(2))?;
        let mut fields = entries.saturating_add(store.balance_default.work_size());
        for value in store.balances.values().chain(store.nonces.values()) {
            fields = fields.saturating_add(value.work_size());
        }
        let mut reset_growth = 128usize;
        for plane in [&store.persistent, &store.transient] {
            reset_growth = reset_growth
                .saturating_add(plane.base.work_size())
                .saturating_add(plane.global_default.work_size());
            fields = fields
                .saturating_add(plane.base.work_size())
                .saturating_add(plane.global_default.work_size());
            for root in plane.roots.values() {
                fields = fields.saturating_add(root.work_size());
            }
            for value in plane.slots.values().chain(plane.defaults.values()) {
                fields = fields.saturating_add(value.work_size());
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
            fields = fields.saturating_add(log.data.work_size());
            for topic in &log.topics {
                fields = fields.saturating_add(topic.work_size());
            }
        }
        check_cancelled()?;
        Ok(Self {
            fields,
            code_bytes,
            reset_growth,
        })
    }
}

#[cfg(test)]
mod tests;
