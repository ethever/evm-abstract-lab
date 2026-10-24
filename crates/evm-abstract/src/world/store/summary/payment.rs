//! Copy invoices enumerate owned summary scalars, never executable store roots.

use super::{EvmStoreSummary, SummaryLog, SummaryPlane};
use crate::{
    domain::WordElement,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, finish_quote, pay, value_work},
    },
};

#[cfg(test)]
mod tests;

impl<N: WordElement> SummaryPlane<N> {
    pub(super) fn clone_entries(&self) -> usize {
        self.slots
            .len()
            .saturating_add(self.defaults.len())
            .saturating_add(self.initial_independence.slots.len())
            .saturating_add(self.initial_independence.defaults.len())
            .saturating_add(1)
    }

    fn numeric_clone_work(&self, budget: &WorkBudget) -> Result<usize, CloneError> {
        let mut work = 0usize;
        for value in self
            .slots
            .values()
            .chain(self.defaults.values())
            .chain([&self.global_default])
        {
            work = work.saturating_add(value_work(value, budget)?);
        }
        finish_quote(work, budget)
    }
}

impl<N: WordElement> SummaryLog<N> {
    pub(super) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        pay(budget, self.topics.len().saturating_add(1))?;
        let mut work = 8usize.saturating_add(self.topics.len().saturating_mul(2));
        for topic in &self.topics {
            work = work.saturating_add(value_work(topic, budget)?);
        }
        // Byte summaries own the content/word census and any shared buffer law.
        work = work.saturating_add(self.data.clone_work_with(budget)?);
        finish_quote(work, budget)
    }

    pub(super) fn clone_with_payment(&self, budget: &mut WorkBudget) -> Result<Self, CloneError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)?;
        let out = self.clone();
        checkpoint(budget)?;
        Ok(out)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Cached map lengths admit the scalar census before any carrier quotation.
    pub(crate) fn clone_prefix_work(&self) -> usize {
        self.persistent
            .clone_entries()
            .saturating_add(self.transient.clone_entries())
            .saturating_add(self.balances.len())
            .saturating_add(self.nonces.len())
            .saturating_add(self.codes.len())
            .saturating_add(self.existence.len())
            .saturating_add(self.created.len())
            .saturating_add(self.pending_destruction.len())
            .saturating_add(self.touched.len())
            .saturating_add(self.possible_logs.len())
            .saturating_add(2)
    }

    /// Pay the census, then quote every native payload that a deep map copy can
    /// own. Persistent maps may share entries; this same bound remains safe for
    /// both backends. No later mutation or native query is prepaid by this fee.
    pub(crate) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        let entries = self.clone_prefix_work();
        pay(budget, entries)?;
        let mut work = 48usize.saturating_add(entries.saturating_mul(12));
        work = work
            .saturating_add(self.persistent.numeric_clone_work(budget)?)
            .saturating_add(self.transient.numeric_clone_work(budget)?);
        for value in self
            .balances
            .values()
            .chain(self.nonces.values())
            .chain([&self.balance_default])
        {
            work = work.saturating_add(value_work(value, budget)?);
        }
        for log in self.possible_logs.values() {
            work = work.saturating_add(log.clone_work_with(budget)?);
        }
        finish_quote(work, budget)
    }

    pub(crate) fn pay_clone_with(&self, budget: &mut WorkBudget) -> Result<(), CloneError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)
    }

    /// Preserve native predicates, metadata identities and backend snapshot
    /// shape. Any refusal occurs before the first numeric clone is executed.
    pub(crate) fn clone_with_payment(&self, budget: &mut WorkBudget) -> Result<Self, CloneError> {
        self.pay_clone_with(budget)?;
        let out = self.clone();
        checkpoint(budget)?;
        Ok(out)
    }
}
