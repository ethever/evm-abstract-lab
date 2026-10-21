//! Clone owns flat maps and scalar wrappers; immutable histories remain handles.

use super::EvmByteArray;
use crate::{
    domain::WordElement,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{finish_quote, pay, value_work},
    },
};

#[cfg(test)]
mod tests;

impl<N: WordElement> EvmByteArray<N> {
    /// Cached container lengths only, before looking at any key or value.
    pub(crate) fn clone_prefix_work(&self) -> usize {
        self.cloned_value_count().saturating_add(1)
    }

    /// Pays the metadata census, returning the remaining complete Clone cost.
    /// No content, expression graph or conditional child is visited to quote it.
    pub(crate) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        pay(budget, self.clone_prefix_work())?;
        let cells = self.bytes.len().saturating_add(self.operands.len());
        let mut work = 16usize // headers, root handle, flags and fixed failure
            .saturating_add(cells.saturating_mul(4))
            .saturating_add(self.operands.len().saturating_mul(2))
            .saturating_add(self.allocations.len().saturating_mul(8))
            .saturating_add(self.conditional.cloned_branches().saturating_mul(4));
        for value in self
            .bytes
            .values()
            .chain(self.operands.values())
            .chain([&self.length, &self.default])
        {
            work = work.saturating_add(value_work(value, budget)?);
        }
        finish_quote(work, budget)
    }

    pub(crate) fn pay_clone_with(&self, budget: &mut WorkBudget) -> Result<(), CloneError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)
    }
}
