//! Stage a complete injected relative admission before touching live slots.

use super::{EvmOutput, EvmPayloadSummary, MachineError};
use crate::analysis::engine::{
    Acceleration, PolicyContext, PolicySite, ProcedureOutput, ProcedureSummaryPolicy,
};
use crate::domain::EvmNumericAlgebra;
use analysis_kernel::{Change, algebra::Prepared};

#[cfg(test)]
mod tests;

pub(super) fn prepare<D: EvmNumericAlgebra, S: ProcedureSummaryPolicy<D>>(
    policy: &S,
    current: &EvmPayloadSummary<D::Element>,
    incoming: &EvmPayloadSummary<D::Element>,
    site: PolicySite,
    acceleration: Acceleration,
    updates: usize,
    cx: &mut PolicyContext<'_, D>,
) -> Result<Prepared<EvmPayloadSummary<D::Element>, usize>, MachineError> {
    let value = policy.admit_relative(current, incoming, site, acceleration, cx)?;
    // Policy implementations meter their preparation. The adapter owns equality
    // and the detached value/evidence installed by the kernel publication.
    // The complete paid comparator owns its structural traversal and native
    // predicates; do not retain the obsolete enclosing traversal charge.
    let change = if current.eq_with_payment(&value, cx.domain, cx.budget)? {
        Change::Unchanged
    } else {
        Change::Changed
    };
    let metadata = if change == Change::Changed {
        updates
            .checked_add(1)
            .ok_or(MachineError::IdentityExhausted)?
    } else {
        updates
    };
    let install_work = current
        .work_size()
        .saturating_add(value.work_size())
        .saturating_add(1);
    Ok(Prepared {
        value,
        metadata,
        change,
        install_work,
    })
}

/// Feedback invalidates delivery even when the admitted bound stays equal.
/// Semantic updates alone drive acceleration; generations also track that
/// scheduling change. Neither counter escapes before the table publication.
pub(super) fn prepare_output<D: EvmNumericAlgebra, S: ProcedureSummaryPolicy<D>>(
    policy: &S,
    current: Option<&EvmOutput<D::Element>>,
    incoming: &ProcedureOutput<D::Element>,
    acceleration: Acceleration,
    invalidate: bool,
    cx: &mut PolicyContext<'_, D>,
) -> Result<Option<EvmOutput<D::Element>>, MachineError> {
    let (summary, unchanged) = match policy.admit_output_change(
        current.map(|old| &old.summary),
        incoming,
        acceleration,
        cx,
    )? {
        Some(summary) => (summary, false),
        None => {
            // The policy has paid for its complete semantic proof. There is no
            // detached value to retain or compare again on a duplicate output.
            cx.reserve(8)?;
            let old = current.ok_or(MachineError::Shape)?;
            if !invalidate {
                return Ok(None);
            }
            // Delivery invalidation still installs a new output generation.
            // Its owned summary is retained, so the actual native copy remains.
            (old.summary.clone_with_payment(cx.budget)?, true)
        }
    };
    let work = summary.work_size();
    cx.reserve(
        work.saturating_add(current.map_or(0, |old| old.work))
            .saturating_add(usize::from(invalidate)),
    )?;
    let changed = !unchanged
        && match current {
            Some(old) => !old
                .summary
                .eq_with_payment(&summary, cx.domain, cx.budget)?,
            None => true,
        };
    if !changed && !invalidate {
        return Ok(None);
    }
    let generation = current
        .map_or(0, |old| old.generation)
        .checked_add(1)
        .ok_or(MachineError::IdentityExhausted)?;
    let updates = current
        .map_or(0, |old| old.updates)
        .checked_add(usize::from(current.is_some() && changed))
        .ok_or(MachineError::IdentityExhausted)?;
    Ok(Some(EvmOutput {
        summary,
        work,
        generation,
        updates,
    }))
}
