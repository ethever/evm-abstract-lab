//! Independent numeric queries on the transfer's shared ledger.
//!
//! Complete numeric operation/refinement envelopes fund their own internals.
//! Queries before or after those operations use this boundary, and callers
//! reuse the returned answer for decisions about the same immutable element.

use crate::{
    U256,
    analysis::FrontierReason,
    domain::{WordElement, relational::QueryReason},
    resource::WorkBudget,
};

pub(super) fn singleton<N: WordElement>(
    value: &N,
    budget: &mut WorkBudget,
) -> Result<Option<U256>, FrontierReason> {
    checkpoint(budget)?;
    let answer = value.singleton_with_payment(|work| {
        // The quote itself may observe cancellation. Check again before the
        // actual query, even when the ledger belongs to an outer scope.
        checkpoint(budget)?;
        if work == usize::MAX || !budget.charge(work) {
            Err(FrontierReason::Work)
        } else {
            checkpoint(budget)
        }
    })?;
    // Opaque code may cancel either owner while executing. Keep the payment,
    // but do not let that result reach metadata decisions or executable state.
    checkpoint(budget)?;
    Ok(answer)
}

pub(super) fn checkpoint(budget: &WorkBudget) -> Result<(), FrontierReason> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    } else if budget.exhausted() {
        Err(FrontierReason::Work)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests;
