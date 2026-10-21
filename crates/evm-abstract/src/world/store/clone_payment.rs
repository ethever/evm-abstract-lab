//! The std OrderedMap backend owns entries; checkpoint/slot histories are shared.

use super::EvmStore;
use crate::{
    domain::WordElement,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{finish_quote, pay, value_work},
    },
};

#[cfg(test)]
mod tests;

impl<N: WordElement> EvmStore<N> {
    pub(crate) fn clone_prefix_work(&self) -> usize {
        self.finalization_entries().saturating_add(1)
    }

    /// Uses the owning maps, never visit_values: that visitor follows shared
    /// histories whose numeric contents are not copied by Store::clone.
    pub(crate) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        let entries = self.finalization_entries();
        pay(budget, self.clone_prefix_work())?;
        let mut work = 32usize.saturating_add(entries.saturating_mul(12));
        for value in self.cloned_values() {
            work = work.saturating_add(value_work(value, budget)?);
        }
        for plane in [&self.persistent, &self.transient] {
            work = work.saturating_add(plane.conditional.cloned_branches().saturating_mul(4));
        }
        for log in self.possible_logs.values() {
            // Admission to each nested container precedes its value census.
            pay(budget, log.topics.len().saturating_add(1))?;
            work = work
                .saturating_add(8)
                .saturating_add(log.topics.len().saturating_mul(2));
            for topic in &log.topics {
                work = work.saturating_add(value_work(topic, budget)?);
            }
            work = work.saturating_add(log.data.clone_work_with(budget)?);
        }
        finish_quote(work, budget)
    }

    pub(crate) fn pay_clone_with(&self, budget: &mut WorkBudget) -> Result<(), CloneError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)
    }
}
