//! Sufficient summary proofs inspect the actual carrier, never its observation.

use super::{AlgebraRef, CloneError, EvmNumericAlgebra, EvmValue, WorkBudget, checkpoint, pay};
use crate::domain::{WordElement, identity::ValueIdentity};

/// Decline an unavailable optional proof without making exhaustion sticky.
/// Completed census/predicate work remains on the original cumulative ledger;
/// cancellation and an already exhausted ledger still propagate to the caller.
pub fn try_proof_work(budget: &mut WorkBudget, work: usize) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    if work == usize::MAX || !budget.can_afford(work) {
        return Ok(false);
    }
    pay(budget, work)?;
    Ok(true)
}

fn compare<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    allow_top: bool,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    if !try_proof_work(budget, 16)? {
        return Ok(false);
    }
    // scalar()/canonical() and the numerical Process factories construct via
    // from_numeric: no provenance, identity, expression or symbolic-limit flag.
    // Check that invariant rather than extending this proof to executable
    // values. Persistent ValueIdentity::Eq intentionally ignores runtime IDs;
    // join drops them, so ordinary Eq cannot establish this precondition.
    for value in [left, right] {
        if !value.provenance().is_top()
            || value.identity().input().is_some()
            || !value.identity().runtime_eq(&ValueIdentity::none())
            || value.expression().is_some()
            || value.symbolic_limit_reached()
        {
            return Ok(false);
        }
    }
    if allow_top {
        let work = domain.numeric_algebra().summary_is_top_work(left.numeric());
        if try_proof_work(budget, work)? {
            let top = left.numeric().is_top();
            checkpoint(budget)?;
            if top {
                return Ok(true);
            }
        }
    }
    let work = domain
        .numeric_algebra()
        .summary_equal_work(left.numeric(), right.numeric());
    if !try_proof_work(budget, work)? {
        return Ok(false);
    }
    let equal = left.numeric() == right.numeric();
    checkpoint(budget)?;
    Ok(equal)
}

/// Top or exact native equality is sufficient; false also means unsupported.
pub fn covers_scalar<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    compare(left, right, domain, true, budget)
}

/// Content reuse needs actual equality, including hidden native predicates.
pub fn equal_scalar<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    compare(left, right, domain, false, budget)
}
