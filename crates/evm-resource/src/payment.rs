//! Owned snapshot quotation uses the original ledger and both cancellation owners.

use crate::WorkBudget;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
/// A refused operation distinguishes cooperative cancellation from work exhaustion.
pub enum ResourceError {
    #[error("snapshot clone cancelled")]
    /// Either the original ledger owner or current caller requested cancellation.
    Cancelled,
    #[error("snapshot clone work exhausted or overflowed")]
    /// Work was refused, exhausted, or its quotation overflowed.
    Work,
}

/// Check both cancellation owners and sticky ledger refusal without charging.
pub fn checkpoint(budget: &WorkBudget) -> Result<(), ResourceError> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        Err(ResourceError::Cancelled)
    } else if budget.exhausted() {
        Err(ResourceError::Work)
    } else {
        Ok(())
    }
}

/// Reserve the complete quotation before work; `usize::MAX` denotes overflow.
pub fn pay(budget: &mut WorkBudget, work: usize) -> Result<(), ResourceError> {
    checkpoint(budget)?;
    if work == usize::MAX {
        return Err(ResourceError::Work);
    }
    if !budget.charge(work) {
        checkpoint(budget)?;
        return Err(ResourceError::Work);
    }
    checkpoint(budget)
}

/// Validate a completed quote without charging or visiting any priced payload.
pub fn finish_quote(work: usize, budget: &WorkBudget) -> Result<usize, ResourceError> {
    checkpoint(budget)?;
    if work == usize::MAX {
        Err(ResourceError::Work)
    } else {
        Ok(work)
    }
}
