//! One persistent whole-entry retention promotion per real Runtime node.
//!
//! The bound lives in `EvmNode<N>.payload`; `EvmBoundedState<N>` owns its persistent policy.
//! This adapter admits retries and reconciles complete execution phases.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    AlgebraRef, EvmExecution, EvmPayloadSummary, FrameCode, FrontierReason, MachineError,
    WorkBudget,
};
use crate::analysis::state::bounded::{EvmBoundedState, reserve};

/// The injected summary strategy owns the admitted result. A persistent retry
/// can execute it only when its frozen coordinate/guarantee closure leaves it
/// unchanged. Refuse incompatibility instead of replacing policy output.
pub(super) fn check_closed<D: EvmNumericAlgebra>(
    representation: &EvmBoundedState<D::Element>,
    bound: &EvmPayloadSummary<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<(), MachineError> {
    let normalized = representation.join(bound, bound, domain, budget, false)?;
    reserve(
        budget,
        bound.work_size().saturating_add(normalized.work_size()),
    )?;
    if !super::semantic_eq(bound, &normalized) {
        return Err(MachineError::Shape);
    }
    MachineError::check_cancelled()
}

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) enum EvmRuntimeRetry<N: WordElement> {
    #[default]
    Fresh,
    Attempted,
    Coarse(Box<EvmBoundedState<N>>),
}

/// Use the same raw producer policy as post-execute retention receipts, before
/// MachineError::frontier_reason can erase identity sentinels and other causes.
pub(super) fn retention_refusal(error: &MachineError) -> bool {
    use super::super::errors;
    match error {
        MachineError::Expression(error) => errors::retention_expression(error).is_some(),
        MachineError::Bytes(error) => errors::retention_bytes(error).is_some(),
        MachineError::Store(error) => errors::retention_store(error).is_some(),
        _ => false,
    }
}

pub(super) fn interruption(reason: &FrontierReason) -> bool {
    matches!(
        reason,
        FrontierReason::Work
            | FrontierReason::Budget(_)
            | FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
    )
}

/// The caller admits only executions whose every frontier is a raw retention
/// refusal. A complete retry replaces that whole phase; a failed retry cannot
/// invalidate its legal alternatives or receipts.
pub(super) fn reconcile<N: WordElement>(
    mut first: EvmExecution<N>,
    mut retry: EvmExecution<N>,
) -> EvmExecution<N> {
    if retry.frontiers.is_empty() {
        retry.diagnostics.extend(first.diagnostics);
        retry
    } else {
        // Keep the original input/receipts/effects as one complete phase. Only
        // an actual interruption needs an additional original-entry boundary;
        // the original retention frontier already witnesses incompleteness.
        for frontier in retry.frontiers {
            if interruption(&frontier.reason) {
                super::super::boundary(&mut first, frontier.pc, frontier.reason);
            }
        }
        first
    }
}

pub(super) fn supports<N: WordElement>(original: &EvmPayloadSummary<N>) -> bool {
    original.active().code == FrameCode::Runtime
        && original.active().program.is_some()
        && !original
            .frames
            .iter()
            .any(|frame| frame.code == FrameCode::Unknown)
}

pub(super) struct Prepared<N: WordElement> {
    pub representation: Box<EvmBoundedState<N>>,
    pub bound: EvmPayloadSummary<N>,
}

/// Fully admitted and reified input, before the retry consumes a transfer or
/// publishes its retained representation. Tests quote this same production step.
pub(super) struct Admitted<N: WordElement> {
    pub representation: Box<EvmBoundedState<N>>,
    pub preparation: analysis_kernel::algebra::Prepared<EvmPayloadSummary<N>, usize>,
    pub payload: crate::analysis::EvmMachinePayload<N>,
}

pub(in crate::analysis) fn prepare_with<D: EvmNumericAlgebra>(
    original: &EvmPayloadSummary<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<Prepared<D::Element>, MachineError> {
    if !supports(original) {
        return Err(MachineError::Shape);
    }
    // Pay for cloning, guarantee erasure, layout projection and the full
    // coarse pass before construction. Layout coordinates come only from
    // the original summary; two additional passes own its persistent copy
    // and eventual release. No partial bound escapes refusal.
    let preparation_work = original.work_size().saturating_mul(8);
    let layout_lifetime_work = original.work_size().saturating_mul(2);
    // One Box allocation and its eventual release are fixed actions;
    // the layout passes already cover its variable field work.
    let box_lifetime_work = 2;
    reserve(
        budget,
        preparation_work
            .saturating_add(layout_lifetime_work)
            .saturating_add(box_lifetime_work),
    )?;
    let representation = EvmBoundedState::freeze(original, true, domain)?;
    // Produce a covering closed candidate without instantiating it first. The
    // injected admission may still change this representation or refuse it;
    // only its accepted result owns a fresh executable input.
    let bound = representation.join(original, original, domain, budget, false)?;
    Ok(Prepared {
        representation: Box::new(representation),
        bound,
    })
}

#[cfg(test)]
type RuntimeRetry = EvmRuntimeRetry<crate::domain::NumericValue>;

#[cfg(test)]
pub(in crate::analysis) fn prepare(
    original: &crate::analysis::state::PayloadSummary,
    domain: crate::domain::ProductAlgebra,
    budget: &mut WorkBudget,
) -> Result<
    (
        Box<crate::analysis::state::bounded::EvmBoundedState<crate::domain::NumericValue>>,
        crate::analysis::state::PayloadSummary,
        crate::analysis::MachinePayload,
    ),
    MachineError,
> {
    let algebra = crate::domain::ValueAlgebra::new(domain);
    let prepared = prepare_with(original, algebra.borrowed(), budget)?;
    reserve(budget, prepared.bound.work_size().saturating_mul(4))?;
    let payload = super::instantiate(&prepared.bound, algebra.borrowed())?;
    MachineError::check_cancelled()?;
    Ok((prepared.representation, prepared.bound, payload))
}

#[cfg(test)]
use super::{Execution, ProductAlgebra};
