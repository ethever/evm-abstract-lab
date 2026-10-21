//! Owned snapshot quotation uses the original ledger and both cancellation owners.

use super::WorkBudget;
use crate::domain::{EvmValue, WordElement};

#[cfg(test)]
pub(crate) mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CloneError {
    #[error("snapshot clone cancelled")]
    Cancelled,
    #[error("snapshot clone work exhausted or overflowed")]
    Work,
}

pub(crate) fn checkpoint(budget: &WorkBudget) -> Result<(), CloneError> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        Err(CloneError::Cancelled)
    } else if budget.exhausted() {
        Err(CloneError::Work)
    } else {
        Ok(())
    }
}

pub(crate) fn pay(budget: &mut WorkBudget, work: usize) -> Result<(), CloneError> {
    checkpoint(budget)?;
    if work == usize::MAX {
        return Err(CloneError::Work);
    }
    if !budget.charge(work) {
        checkpoint(budget)?;
        return Err(CloneError::Work);
    }
    checkpoint(budget)
}

/// Only declared cached copy metadata is consulted, after the owner's census.
pub(crate) fn value_work<N: WordElement>(
    value: &EvmValue<N>,
    budget: &WorkBudget,
) -> Result<usize, CloneError> {
    checkpoint(budget)?;
    let work = value.clone_work();
    finish_quote(work, budget)
}

pub(crate) fn finish_quote(work: usize, budget: &WorkBudget) -> Result<usize, CloneError> {
    checkpoint(budget)?;
    if work == usize::MAX {
        Err(CloneError::Work)
    } else {
        Ok(work)
    }
}
