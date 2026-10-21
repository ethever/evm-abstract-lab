//! Independent numeric queries on the transfer's shared ledger.
//!
//! Complete numeric operation/refinement envelopes fund their own internals.
//! Queries before or after those operations use this boundary, and callers
//! reuse the returned answer for decisions about the same immutable element.

use crate::{
    U256,
    analysis::FrontierReason,
    domain::{SingletonProof, WordElement, relational::QueryReason},
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

/// Metadata changes do not invalidate a proof about the same immutable N.
/// Reusing either answer requires no new query or payment.
pub(super) fn resolve<N: WordElement>(
    value: &N,
    proof: &mut SingletonProof,
    budget: &mut WorkBudget,
) -> Result<Option<U256>, FrontierReason> {
    checkpoint(budget)?;
    match *proof {
        SingletonProof::Answer(answer) => Ok(answer),
        SingletonProof::NotQueried => {
            let answer = singleton(value, budget)?;
            *proof = SingletonProof::Answer(answer);
            Ok(answer)
        }
    }
}

/// A standalone membership test owns the carrier's complete structural bound,
/// which explicitly includes one membership test. It buys no singleton query.
pub(super) fn contains<N: WordElement>(
    value: &N,
    word: U256,
    budget: &mut WorkBudget,
) -> Result<bool, FrontierReason> {
    checkpoint(budget)?;
    let work = value.work_size();
    checkpoint(budget)?;
    if work == usize::MAX || !budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    checkpoint(budget)?;
    let contains = value.contains(word);
    checkpoint(budget)?;
    Ok(contains)
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
