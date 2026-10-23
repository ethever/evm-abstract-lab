//! Staged native scalar payment on the original summary ledger.
//!
//! Each envelope funds exactly its next hook. Projection, canonicalization and
//! widening quote the actual intermediate element, whose tariff can differ
//! arbitrarily from either input. Pure algebra/value methods remain caller-funded.

use super::{AlgebraRef, EvmNumericAlgebra, EvmValue, SummaryConstruction, WordElement};
use crate::resource::{
    CloneError, WorkBudget,
    clone_payment::{checkpoint, pay, value_work},
};
use alloy_primitives::U256;

#[cfg(test)]
mod tests;

// Empty wrapper construction/release, identity flags and shared-expression
// handles. Native construction and query work is never inferred from this fee.
const WRAPPER_WORK: usize = 16;

pub(crate) fn clone_value<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, CloneError> {
    let work = value_work(value, budget)?;
    pay(budget, work)?;
    let out = value.clone();
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn singleton<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<Option<U256>, CloneError> {
    checkpoint(budget)?;
    let answer = value
        .numeric()
        .singleton_with_payment(|work| pay(budget, work))?;
    checkpoint(budget)?;
    Ok(answer)
}

pub(crate) fn unsigned_bounds<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<(U256, U256), CloneError> {
    let answer = singleton(value, budget)?;
    let bounds = if let Some(word) = answer {
        (word, word)
    } else {
        // Fund the independent conservative view, without a second discovery.
        pay(
            budget,
            value.numeric().work_size().saturating_add(WRAPPER_WORK),
        )?;
        value.interval().unsigned_bounds()
    };
    checkpoint(budget)?;
    Ok(bounds)
}

/// Summary literals have no executable source metadata, matching Content::at.
pub(crate) fn constant<D: EvmNumericAlgebra>(
    literal: U256,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    checkpoint(budget)?;
    pay(
        budget,
        domain
            .literal_quote_work(literal)
            .saturating_add(WRAPPER_WORK),
    )?;
    let out = EvmValue::from_numeric(D::Element::constant(literal));
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn top<D: EvmNumericAlgebra>(
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    construct(SummaryConstruction::Top, domain, budget)
}

pub(crate) fn unknown_byte<D: EvmNumericAlgebra>(
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    construct(SummaryConstruction::UnknownByte, domain, budget)
}

/// The summary invariant supplies ordered endpoints, as for the pure factory.
pub(crate) fn unsigned_range<D: EvmNumericAlgebra>(
    lower: U256,
    upper: U256,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    construct(
        SummaryConstruction::UnsignedRange { lower, upper },
        domain,
        budget,
    )
}

fn construct<D: EvmNumericAlgebra>(
    construction: SummaryConstruction,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    checkpoint(budget)?;
    let work = domain
        .numeric_algebra()
        .summary_construction_work(construction);
    pay(budget, work.saturating_add(WRAPPER_WORK))?;
    let out = match construction {
        SummaryConstruction::Top => EvmValue::top(),
        SummaryConstruction::UnknownByte => EvmValue::unknown_byte(),
        SummaryConstruction::UnsignedRange { lower, upper } => {
            EvmValue::unsigned_range(lower, upper).expect("summary range endpoints are ordered")
        }
    };
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn project<D: EvmNumericAlgebra>(
    value: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    let copy = value_work(value, budget)?;
    let work = copy.saturating_add(domain.projection_work(value));
    pay(budget, work)?;
    let out = domain.project(value);
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn canonical<D: EvmNumericAlgebra>(
    value: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    checkpoint(budget)?;
    let work = domain
        .numeric_algebra()
        .summary_canonical_work(value.numeric());
    pay(budget, work.saturating_add(WRAPPER_WORK))?;
    let out = EvmValue::from_numeric(value.numeric().canonical_for_summary());
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn scalar<D: EvmNumericAlgebra>(
    value: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    let projected = project(value, domain, budget)?;
    canonical(&projected, domain, budget)
}

// The facade unions bounded origin B-trees, copies identity flags and compares
// retained expressions. Expression comparison owns its own structural bound;
// no native equality, singleton, observation or clone is used to price it.
fn join_metadata_work<N: WordElement>(left: &EvmValue<N>, right: &EvmValue<N>) -> usize {
    let origins = left
        .provenance()
        .origins()
        .work_size()
        .saturating_add(right.provenance().origins().work_size());
    let levels = usize::BITS as usize - origins.max(1).leading_zeros() as usize;
    let expression = match (left.expression(), right.expression()) {
        (Some(left), Some(right)) => left.comparison_work(right),
        _ => 1,
    };
    WRAPPER_WORK
        .saturating_add(origins.saturating_mul(16usize.saturating_add(levels.saturating_mul(4))))
        .saturating_add(expression)
}

/// Full wrapper equality, including the separately priced actual native Eq.
pub(crate) fn equal<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    let native = domain
        .numeric_algebra()
        .summary_equal_work(left.numeric(), right.numeric());
    // The same origin/expression bound covers equality without constructing
    // any metadata union. It deliberately does not price native Eq by size.
    pay(
        budget,
        native.saturating_add(join_metadata_work(left, right)),
    )?;
    let answer = left == right;
    checkpoint(budget)?;
    Ok(answer)
}

/// Numeric Top discovery; source/identity metadata is not a numeric constraint.
pub(crate) fn is_top<D: EvmNumericAlgebra>(
    value: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    let work = domain
        .numeric_algebra()
        .summary_is_top_work(value.numeric());
    pay(budget, work)?;
    let answer = value.numeric().is_top();
    checkpoint(budget)?;
    Ok(answer)
}

pub(crate) fn join<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    checkpoint(budget)?;
    let native = domain
        .numeric_algebra()
        .summary_join_work(left.numeric(), right.numeric(), false);
    pay(
        budget,
        native.saturating_add(join_metadata_work(left, right)),
    )?;
    let joined = domain.join(left, right);
    checkpoint(budget)?;
    if !widen {
        return Ok(joined);
    }
    // A cheap input can yield an arbitrarily expensive result. The first join
    // quote supplies no price for this separately selected procedure hook.
    let native = domain
        .numeric_algebra()
        .summary_join_work(left.numeric(), joined.numeric(), true);
    pay(
        budget,
        native.saturating_add(join_metadata_work(left, &joined)),
    )?;
    let out = domain.widen_procedure(left, &joined);
    checkpoint(budget)?;
    Ok(out)
}

pub(crate) fn join_scalar<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    let joined = join(left, right, domain, widen, budget)?;
    scalar(&joined, domain, budget)
}

pub(crate) fn join_plain<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, CloneError> {
    let joined = join(left, right, domain, widen, budget)?;
    pay(
        budget,
        WRAPPER_WORK.saturating_add(joined.provenance().origins().work_size()),
    )?;
    // Move the actual carrier. Dropping facade metadata cannot justify a
    // conversion through an observation or an unnecessary native clone.
    let out = EvmValue::from_numeric(joined.numeric);
    checkpoint(budget)?;
    Ok(out)
}
