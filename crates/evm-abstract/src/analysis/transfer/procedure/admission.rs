//! Stage a complete injected relative admission before touching live slots.

use super::{EvmOutput, EvmPayloadSummary, MachineError, semantic_eq};
use crate::analysis::engine::{
    Acceleration, PolicyContext, PolicySite, ProcedureOutput, ProcedureSummaryPolicy,
};
use crate::domain::EvmNumericAlgebra;
use analysis_kernel::{Change, algebra::Prepared};

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
    cx.reserve(current.work_size().saturating_add(value.work_size()))?;
    let change = if semantic_eq(current, &value) {
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
    let summary =
        policy.admit_output(current.map(|old| &old.summary), incoming, acceleration, cx)?;
    let work = summary.work_size();
    cx.reserve(
        work.saturating_add(current.map_or(0, |old| old.work))
            .saturating_add(usize::from(invalidate)),
    )?;
    let changed = current.is_none_or(|old| old.summary != summary);
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
