//! Header-local numerical invariants. Executable identities are never the
//! convergence criterion; independent roots are built only after a bound grows.

use super::{
    Domain, Entry, FrameCode, MachineError, MachinePayload, PayloadSummary, WorkBudget, form,
    instantiate, join, semantic_eq,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(in crate::analysis) struct LoopBound {
    layout: PayloadSummary,
    bound: PayloadSummary,
    // Reprojection can restore singleton/bit facts erased by the fallback.
    // Keep every later lattice operation in the same canonical coarse form.
    coarse: bool,
}

fn reserve(budget: &mut WorkBudget, cost: usize) -> Result<(), MachineError> {
    MachineError::check_cancelled()?;
    if cost == usize::MAX || !budget.charge(cost) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

fn project(
    payload: &MachinePayload,
    entry: &Entry,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Result<PayloadSummary, MachineError> {
    if payload
        .call_stack
        .iter()
        .any(|frame| frame.code == FrameCode::Unknown)
    {
        return Err(MachineError::Shape);
    }
    reserve(
        budget,
        payload
            .work_size()
            .saturating_mul(4)
            .saturating_add(super::payload::alias_form_work(payload))
            .saturating_add(super::storage_observations::form_work(payload, domain)),
    )?;
    let mut projected = form(payload, entry, domain)?;
    // These are optional guarantees, not coordinates of the loop lattice.
    // Erase them consistently before comparing or independently replaying roots.
    projected.storage_observations = Default::default();
    for frame in &mut projected.frames {
        frame.stack_aliases = super::stack_alias::StackAliases::form(&frame.stack, false)?;
    }
    projected.work = projected.measure_work();
    Ok(projected)
}

pub(super) fn restrict(
    payload: &PayloadSummary,
    layout: &PayloadSummary,
    domain: Domain,
) -> PayloadSummary {
    let mut out = payload.clone();
    out.store = out.store.project_to_layout(&layout.store, domain);
    for (frame, shape) in out.frames.iter_mut().zip(&layout.frames) {
        frame.memory = frame.memory.project_to_layout(&shape.memory, domain);
        frame.calldata = frame.calldata.project_to_layout(&shape.calldata, domain);
        frame.returndata = frame
            .returndata
            .project_to_layout(&shape.returndata, domain);
        frame.saved_store = frame
            .saved_store
            .project_to_layout(&shape.saved_store, domain);
    }
    out.work = out.measure_work();
    out
}

pub(super) fn coarse(payload: &PayloadSummary) -> PayloadSummary {
    let mut out = payload.clone();
    let scalar = |value: &mut crate::domain::AbstractValue| {
        let (lower, upper) = value.interval().unsigned_bounds();
        *value = crate::domain::AbstractValue::unsigned_range(lower, upper)
            .expect("summary bounds are ordered");
    };
    out.store = out.store.forget_loop_numeric();
    for frame in &mut out.frames {
        frame.stack.iter_mut().for_each(scalar);
        scalar(&mut frame.call_value);
        scalar(&mut frame.address_operand);
        scalar(&mut frame.caller_operand);
        frame.memory = frame.memory.forget_loop_content();
        frame.calldata = frame.calldata.forget_loop_content();
        frame.returndata = frame.returndata.forget_loop_content();
        frame.saved_store = frame.saved_store.forget_loop_numeric();
    }
    for continuation in &mut out.continuations {
        scalar(&mut continuation.output_offset);
        scalar(&mut continuation.output_size);
    }
    out.work = out.measure_work();
    out
}

fn retention_refusal(error: &MachineError) -> bool {
    use crate::{
        domain::symbolic::{ExprError, StateError},
        world::{RangeError, StoreError},
    };
    let bytes = |error: &RangeError| {
        matches!(
            error,
            RangeError::Expression(ExprError::Limit { .. })
                | RangeError::State(StateError::Limit { .. })
        )
    };
    match error {
        MachineError::Expression(ExprError::Limit { .. }) => true,
        MachineError::Bytes(error) => bytes(error),
        MachineError::Store(error) => match error {
            StoreError::Expression(ExprError::Limit { .. })
            | StoreError::State(StateError::Limit { .. }) => true,
            StoreError::Bytes(error) => bytes(error),
            _ => false,
        },
        _ => false,
    }
}

fn materialize(
    bound: PayloadSummary,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Result<(PayloadSummary, MachinePayload, bool), MachineError> {
    reserve(budget, bound.work_size().saturating_mul(4))?;
    match instantiate(&bound, domain) {
        Ok(payload) => Ok((bound, payload, false)),
        Err(error) if retention_refusal(&error) => {
            // One conservative retry changes the retained invariant itself.
            // Subsequent coverage uses this larger bound, never the failed one.
            let bound = coarse(&bound);
            reserve(budget, bound.work_size().saturating_mul(4))?;
            let payload = instantiate(&bound, domain)?;
            Ok((bound, payload, true))
        }
        Err(error) => Err(error),
    }
}

impl LoopBound {
    fn normalize(&self, summary: PayloadSummary) -> PayloadSummary {
        if self.coarse {
            coarse(&summary)
        } else {
            summary
        }
    }

    pub(in crate::analysis) fn new(
        old: &MachinePayload,
        incoming: &MachinePayload,
        entry: &Entry,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<(Self, MachinePayload), MachineError> {
        if old.key() != incoming.key() {
            return Err(MachineError::Shape);
        }
        let old = project(old, entry, domain, budget)?;
        let incoming = project(incoming, entry, domain, budget)?;
        reserve(
            budget,
            old.work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let layout = join(&old, &incoming, domain, false)?;
        // The layout retains bounded coordinates, not an unbounded exact byte
        // buffer used only to choose those coordinates on the first feedback.
        let layout = restrict(&layout, &layout, domain);
        let bound = restrict(&join(&old, &incoming, domain, true)?, &layout, domain);
        let (bound, payload, coarse) = materialize(bound, domain, budget)?;
        Ok((
            Self {
                layout,
                bound,
                coarse,
            },
            payload,
        ))
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        self.layout
            .work_size()
            .saturating_add(self.bound.work_size())
    }

    pub(in crate::analysis) fn update(
        &self,
        incoming: &MachinePayload,
        entry: &Entry,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<Option<(Self, MachinePayload)>, MachineError> {
        if incoming.key() != self.layout.location {
            return Err(MachineError::Shape);
        }
        let incoming = project(incoming, entry, domain, budget)?;
        reserve(
            budget,
            self.work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let incoming = self.normalize(restrict(&incoming, &self.layout, domain));
        let joined = self.normalize(restrict(
            &join(&self.bound, &incoming, domain, false)?,
            &self.layout,
            domain,
        ));
        if semantic_eq(&joined, &self.bound) {
            MachineError::check_cancelled()?;
            return Ok(None);
        }
        let bound = self.normalize(restrict(
            &join(&self.bound, &joined, domain, true)?,
            &self.layout,
            domain,
        ));
        let (bound, payload, coarse) = materialize(bound, domain, budget)?;
        Ok(Some((
            Self {
                layout: self.layout.clone(),
                bound,
                coarse: self.coarse || coarse,
            },
            payload,
        )))
    }
}
