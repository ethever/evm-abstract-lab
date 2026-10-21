//! Stage a complete injected relative admission before touching live slots.

use super::{EvmPayloadSummary, MachineError, semantic_eq};
use crate::analysis::engine::{Acceleration, PolicyContext, PolicySite, ProcedureSummaryPolicy};
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
