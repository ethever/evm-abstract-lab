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
    let height = (usize::BITS - entries.leading_zeros()) as usize + 1;
    entries.saturating_add(1).saturating_mul(height)
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
}

impl Plan {
    pub(super) fn new(store: &Store) -> Self {
        Self {
            actions: store
                .addresses()
                .into_iter()
                .map(|address| (address, Deletion::at(store, address)))
                .collect(),
        }
    }

    fn work(&self, footprint: Footprint) -> usize {
        let deletes = self
            .actions
            .iter()
            .filter(|(_, action)| *action != Deletion::Retain)
            .count();
        let joins = self
            .actions
            .iter()
            .filter(|(_, action)| *action == Deletion::Join)
            .count();

        // No address or slot key is invented. One whole-Store join can make
        // every known address explicit in the account maps, including copies of
        // balance_default. Deleting a previously implicit owner can add a root,
        // default and independence default per plane. A joined root is the old
        // root (or account-scoped base) plus zero, selector and ITE; balance and
        // nonce add at most four expression nodes and one zero candidate each.
        // 128 covers those fixed fields/nodes, keys and zero scalar metadata.
        // Existing slots gain at most one zero candidate and Constant origin,
        // once per owner; operand intersections and log joins add no fields.
        let materialized = if joins == 0 {
            0
        } else {
            self.actions
                .len()
                .saturating_mul(footprint.balance_default.saturating_add(64))
                .saturating_add(footprint.slots.saturating_mul(2))
        };
        let grown = footprint
            .fields
            .saturating_add(materialized)
            .saturating_add(deletes.saturating_mul(footprint.reset_growth));

        // Four cleanup/drop passes; sixteen passes per reset cover four retain
        // loops (slots and independence slots in both planes), ten ordered-map
        // updates and their COW/drop overhead. A MAY branch additionally pays
        // 64 complete passes: branch clone/drop, repeated numeric-cache joins,
        // existing-value/graph checks, ordered key unions, guarded plane/account
        // joins and unchanged log checks/joins. Numeric joins enumerate unions,
        // not candidate Cartesian products; candidate and graph sizes are in F.
        // Runtime equality can inspect bytes in four Store/code comparisons per
        // MAY branch. Code is Arc-backed in clones and is never an account count.
        let passes = 4usize
            .saturating_add(deletes.saturating_mul(16))
            .saturating_add(joins.saturating_mul(64));
        tree_work(grown)
            .saturating_mul(passes)
            .saturating_add(footprint.code_bytes.saturating_mul(joins).saturating_mul(4))
            // A final Program Arc may release decoded blocks/jumpdest entries;
            // their count is bounded by byte length. Clones alone cannot do so.
            .saturating_add(if deletes == 0 {
                0
            } else {
                footprint.code_bytes.saturating_mul(4)
            })
    }

    pub(super) fn apply(
        self,
        store: &mut Store,
        domain: Domain,
        interrupt: &mut impl FnMut() -> Result<(), FinalizationError>,
    ) -> Result<(), FinalizationError> {
        for (address, action) in self.actions {
            interrupt()?;
            // A deletion changes only this address's numeric/lifecycle facts.
            // Joins can materialize other owners' defaults but preserve those
            // facts, so classifications of the remaining addresses stay valid.
            match action {
                Deletion::Retain => {}
                Deletion::Delete => store.delete_account(address),
                Deletion::Join => {
                    let mut deleted = store.clone();
                    deleted.delete_account(address);
                    *store = store.join(&deleted, domain);
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
    slots: usize,
    balance_default: usize,
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
        // Store::join preserves a numerical upper fallback when retaining its
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
        let mut slots = 0usize;
        let mut reset_growth = 128usize;
        for plane in [&store.persistent, &store.transient] {
            slots = slots.saturating_add(plane.slots.len());
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
            slots,
            balance_default: store.balance_default.work_size(),
            reset_growth,
        })
    }
}

#[cfg(test)]
mod tests;
