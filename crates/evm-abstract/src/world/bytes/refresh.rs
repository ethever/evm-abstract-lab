//! Reader reuse belongs only to one staged refresh with immutable read inputs.

use super::{EvmByteArray, RangeError, conditional::Context, constant_with, numeric};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::{
    domain::{
        EvmValue,
        provenance::MAX_ORIGINS,
        symbolic::{ExprError, StateError},
    },
    resource::WorkBudget,
};

#[cfg(test)]
mod tests;

// Fixed extra bookkeeping: the two admission decisions, first-cell batch
// descriptor/selection and final scope checks. The first Context/Reader and
// its cache copies/drop already belong to the original bulk fee; keeping
// them alive adds no payload clone or second capacity-sized destruction.
const EPOCH_WORK: usize = 8;

pub(super) fn charge(budget: &mut WorkBudget, work: usize) -> Result<bool, RangeError> {
    if analysis_control::current_cancellation().is_cancelled() {
        // Latch an actual cancelled charge on the original ledger as well.
        budget.charge(0);
        return Err(ExprError::Cancelled.into());
    }
    let paid = !budget.exhausted() && budget.charge(work);
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(ExprError::Cancelled.into());
    }
    Ok(paid)
}

fn compatible<N: WordElement>(
    result: Result<EvmValue<N>, RangeError>,
) -> Result<EvmValue<N>, RangeError> {
    match result {
        Err(
            error @ (RangeError::Expression(ExprError::Cancelled)
            | RangeError::State(StateError::Cancelled)),
        ) => Err(error),
        // Preserve byte_at's existing compatibility marker for checked failures.
        Err(_) => Ok(EvmValue::<N>::unknown_byte().with_symbolic_limit()),
        Ok(value) => Ok(value),
    }
}

// The original literal-index remember cannot add an informative missing key.
// Existing keys retain their cloned handles; an updated constant-key AV can
// only retain its original expression handle or lose it (join with a literal).
// Thus full map equality compares shared expression handles in O(1), not their
// expanded DAGs. Finite-set equality may scan candidates even for shared sets;
// count those, all bounded origin alternatives, and every remaining AV field.
// A constant-key join can add one candidate to a nonempty finite set:
// n + 1 <= 2 * n, including n == 1. Top stays Top. The factor of two also
// covers this literal-remember corner without scanning candidates to quote it.
fn equality_work<N: WordElement>(array: &EvmByteArray<N>) -> usize {
    array.operands.values().fold(8usize, |work, value| {
        work.saturating_add(
            value
                .numeric()
                .work_size()
                .saturating_add(MAX_ORIGINS)
                .saturating_add(8)
                .saturating_mul(2),
        )
    })
}

// Optional admission is read-only. Only this already-funded original path may
// run after a decline; a failed actual charge never reaches it. Final checking
// turns any mid-loop cancellation into an atomic abort at the write boundary.
fn original<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &mut EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, RangeError> {
    if !charge(budget, 0)? {
        return Ok(false);
    }
    array.refresh(domain);
    charge(budget, 0)
}

/// Only the caller's unpublished staged array may be mutated. Original per-cell
/// snapshots, Context construction/selection, expressions and Reader work keep
/// their original bulk owner. New census/equality/bookkeeping pays separately.
pub(super) fn run<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &mut EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, RangeError> {
    if !charge(budget, 0)? {
        return Ok(false);
    }
    if array.bytes.is_empty() {
        return Ok(true);
    }
    // Cached lengths fund the O(entries) size census before it starts. Each
    // visit reads only cached finite cardinality and inline numeric fields.
    let census = array.operands.len().saturating_mul(8).saturating_add(8);
    if !budget.can_afford(census) {
        return original(array, domain, budget);
    }
    if !charge(budget, census)? {
        return Ok(false);
    }
    let equality = equality_work(array);
    // Pay every possible comparison before retaining anything. Per-cell
    // shells own the added checks and dispatch. A second optional decline
    // retains the census debit and uses the fully paid original algorithm.
    let work =
        EPOCH_WORK.saturating_add(equality.saturating_add(8).saturating_mul(array.bytes.len()));
    if !budget.can_afford(work) {
        return original(array, domain, budget);
    }
    if !charge(budget, work)? {
        return Ok(false);
    }
    let indices = array.bytes.keys().copied().collect::<Vec<_>>();
    let (first, rest) = indices.split_first().expect("nonempty staged coordinates");
    let (first_offset, facts) = match array.prepare_byte_read(&constant_with(*first), domain) {
        Ok(prepared) => prepared,
        Err(error) => {
            array.bytes.insert(*first, compatible(Err(error))?);
            for index in rest {
                if !charge(budget, 0)? {
                    return Ok(false);
                }
                let value =
                    compatible(array.read_byte_checked_with(&constant_with(*index), domain))?;
                array.bytes.insert(*index, value);
            }
            return charge(budget, 0);
        }
    };
    // This is the FIRST cell's original Context, not an additional selection
    // or fact snapshot. Its clone shares that exact full guard environment.
    // Context::new has no demand/payer or sticky cache-refusal state; its memo
    // is unused. Only cancellation can stop it. Both its own checks and the
    // original per-index checks stay live.
    let context = Context::<N, D>::new(&facts, Some(&array.conditional), domain);
    let mut reader = numeric::Reader::<N, D>::with_context(context.clone(), domain);
    #[cfg(test)]
    tests::EPOCHS.update(|stats| stats.epochs += 1);
    let first_value =
        compatible(
            array.read_byte_prepared(&first_offset, &context, domain, || {
                #[cfg(test)]
                super::write::NUMERIC_READS.update(|calls| *calls += 1);
                #[cfg(test)]
                let before = reader.cache_stats();
                let value = reader.read(&array.root, &first_offset);
                #[cfg(test)]
                tests::observed(&reader, before, false);
                value
            }),
        )?;
    context.check()?;
    array.bytes.insert(*first, first_value);
    for index in rest {
        if !charge(budget, 0)? {
            return Ok(false);
        }
        // Never use a cached value after the shared Context has stopped.
        // The checks precede equality and Reader.read, including cache hits.
        context.check()?;
        let value = (|| {
            let (offset, current_facts) =
                array.prepare_byte_read(&constant_with(*index), domain)?;
            let current = Context::<N, D>::new(&current_facts, Some(&array.conditional), domain);
            let same = current_facts == facts;
            array.read_byte_prepared(&offset, &current, domain, || {
                #[cfg(test)]
                super::write::NUMERIC_READS.update(|calls| *calls += 1);
                if same {
                    #[cfg(test)]
                    let before = reader.cache_stats();
                    let value = reader.read(&array.root, &offset);
                    #[cfg(test)]
                    tests::observed(&reader, before, true);
                    value
                } else {
                    #[cfg(test)]
                    tests::EPOCHS.update(|stats| stats.fresh += 1);
                    numeric::Reader::<N, D>::with_context(current.clone(), domain)
                        .read(&array.root, &offset)
                }
            })
        })();
        context.check()?;
        array.bytes.insert(*index, compatible(value)?);
    }
    drop(reader);
    drop(context);
    charge(budget, 0)
}
