//! Standalone numeric proof queries on the complete-state operation's ledger.
//!
//! Copy and retention fees do not fund discovery. A caller requiring a proof
//! uses this boundary once and reuses its answer for that immutable element.

use super::{EvmValue, MachineError, WorkBudget};
use crate::domain::{WordElement, relational::QueryReason};
use alloy_primitives::U256;

pub(in crate::analysis) fn singleton<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<Option<U256>, MachineError> {
    checkpoint(budget)?;
    let work = value.singleton_work();
    // Opaque quotation may cancel either scope, including an outer owner whose
    // budget is now used under a different current cancellation token.
    checkpoint(budget)?;
    if work == usize::MAX || !budget.charge(work) {
        checkpoint(budget)?;
        return Err(MachineError::Work);
    }
    checkpoint(budget)?;
    let answer = value.singleton();
    // Keep the payment, but never publish an answer obtained after cancellation.
    checkpoint(budget)?;
    Ok(answer)
}

fn checkpoint(budget: &WorkBudget) -> Result<(), MachineError> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        Err(MachineError::Relations(QueryReason::Cancelled))
    } else if budget.exhausted() {
        Err(MachineError::Work)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::analysis) mod tests;
