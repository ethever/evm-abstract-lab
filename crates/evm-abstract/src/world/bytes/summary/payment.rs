//! Native summary copies use the caller's original ledger and cancellation.

use super::{Content, EvmByteArraySummary, EvmValue};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement},
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, finish_quote, pay, value_work},
    },
};

mod content;
mod coverage;
mod equality;
mod join;

#[cfg(test)]
mod metering_tests;
#[cfg(test)]
mod tests;

/// BTree coordinate lookup/insertion, independent of opaque numeric payloads.
pub(super) fn coordinate_work(entries: usize) -> usize {
    (entries.saturating_add(1).ilog2() as usize)
        .saturating_add(2)
        .saturating_mul(16)
}

/// The unknown-byte sentinel comparison deliberately ignores value metadata.
/// Paying full-value equality and then using its answer would change this law.
pub(super) fn numeric_equal<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &EvmValue<N>,
    right: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    let work = domain
        .numeric_algebra()
        .summary_equal_work(left.numeric(), right.numeric());
    pay(budget, work)?;
    let equal = left.numeric() == right.numeric();
    checkpoint(budget)?;
    Ok(equal)
}

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Cached container counts fund the census before any carrier quote.
    pub(crate) fn clone_prefix_work(&self) -> usize {
        let cells = match &self.content {
            Content::Cells(cells) => cells.len(),
            Content::Zero | Content::Exact(_) => 0,
        };
        cells.saturating_add(self.words.len()).saturating_add(2)
    }

    /// Pay for the census and quote the remaining complete derived Clone.
    /// Exact byte buffers remain shared handles; native cells and words do not.
    pub(crate) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        pay(budget, self.clone_prefix_work())?;
        let cells = match &self.content {
            Content::Cells(cells) => Some(cells),
            Content::Zero | Content::Exact(_) => None,
        };
        let entries = cells
            .map_or(0, |cells| cells.len())
            .saturating_add(self.words.len());
        let mut work = 8usize.saturating_add(entries.saturating_mul(4));
        for value in cells
            .into_iter()
            .flat_map(|cells| cells.values())
            .chain(self.words.values())
            .chain([&self.length])
        {
            work = work.saturating_add(value_work(value, budget)?);
        }
        finish_quote(work, budget)
    }

    pub(crate) fn pay_clone_with(&self, budget: &mut WorkBudget) -> Result<(), CloneError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)
    }

    /// Refusal never copies a carrier or publishes a partially copied summary.
    pub(crate) fn clone_with_payment(&self, budget: &mut WorkBudget) -> Result<Self, CloneError> {
        self.pay_clone_with(budget)?;
        let out = self.clone();
        checkpoint(budget)?;
        Ok(out)
    }
}
