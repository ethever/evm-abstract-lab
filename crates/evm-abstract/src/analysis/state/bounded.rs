//! Persistent coordinates and normalization for whole-state numerical bounds.
//!
//! Feedback bounds own their summary; Runtime retries use the node's summary.
//! Both retain this same representation policy, so reprojection cannot restore
//! erased guarantees or leave coarse mode. Callers own admission and publication.

use super::{
    Domain, MachineError, MachinePayload, PayloadSummary, WorkBudget, instantiate, join,
    stack_alias::StackAliases,
};

#[derive(Clone)]
pub(in crate::analysis) struct BoundedState {
    layout: PayloadSummary,
    coarse: bool,
}

pub(in crate::analysis) fn reserve(
    budget: &mut WorkBudget,
    cost: usize,
) -> Result<(), MachineError> {
    MachineError::check_cancelled()?;
    if cost == usize::MAX || !budget.charge(cost) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

pub(super) fn erase_guarantees(
    mut summary: PayloadSummary,
) -> Result<PayloadSummary, MachineError> {
    summary.storage_observations = Default::default();
    for frame in &mut summary.frames {
        frame.stack_aliases = StackAliases::form(&frame.stack, false)?;
    }
    summary.work = summary.measure_work();
    Ok(summary)
}

fn restrict(
    mut summary: PayloadSummary,
    layout: &PayloadSummary,
    domain: Domain,
) -> PayloadSummary {
    summary.store = summary.store.project_to_layout(&layout.store, domain);
    for (frame, shape) in summary.frames.iter_mut().zip(&layout.frames) {
        frame.memory = frame.memory.project_to_layout(&shape.memory, domain);
        frame.calldata = frame.calldata.project_to_layout(&shape.calldata, domain);
        frame.returndata = frame
            .returndata
            .project_to_layout(&shape.returndata, domain);
        frame.saved_store = frame
            .saved_store
            .project_to_layout(&shape.saved_store, domain);
    }
    summary
}

fn coarse(mut summary: PayloadSummary) -> PayloadSummary {
    let scalar = |value: &mut crate::domain::AbstractValue| {
        let (lower, upper) = value.interval().unsigned_bounds();
        *value = crate::domain::AbstractValue::unsigned_range(lower, upper)
            .expect("summary bounds are ordered");
    };
    summary.store = summary.store.forget_loop_numeric();
    for frame in &mut summary.frames {
        frame.stack.iter_mut().for_each(scalar);
        scalar(&mut frame.call_value);
        scalar(&mut frame.address_operand);
        scalar(&mut frame.caller_operand);
        frame.memory = frame.memory.forget_loop_content();
        frame.calldata = frame.calldata.forget_loop_content();
        frame.returndata = frame.returndata.forget_loop_content();
        frame.saved_store = frame.saved_store.forget_loop_numeric();
    }
    for continuation in &mut summary.continuations {
        scalar(&mut continuation.output_offset);
        scalar(&mut continuation.output_size);
    }
    summary
}

impl BoundedState {
    /// The caller prepays construction and persistent retention. Self-projection
    /// bounds the layout itself instead of retaining an exact byte buffer solely
    /// for its coordinates. Coarse content never replaces these frozen keys.
    pub(in crate::analysis) fn freeze(
        layout: &PayloadSummary,
        coarse: bool,
        domain: Domain,
    ) -> Result<Self, MachineError> {
        Ok(Self {
            layout: erase_guarantees(restrict(layout.clone(), layout, domain))?,
            coarse,
        })
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        self.layout.work_size()
    }

    pub(super) fn location(&self) -> &crate::analysis::MachineKey {
        &self.layout.location
    }

    #[cfg(test)]
    pub(super) fn layout(&self) -> &PayloadSummary {
        &self.layout
    }

    fn normalize(
        &self,
        summary: PayloadSummary,
        domain: Domain,
    ) -> Result<PayloadSummary, MachineError> {
        MachineError::check_cancelled()?;
        let summary = restrict(summary, &self.layout, domain);
        let summary = if self.coarse {
            coarse(summary)
        } else {
            summary
        };
        erase_guarantees(summary)
    }

    /// Reserve the incoming normalization, joins, projection, persistent copy
    /// and equality traversal together. Feedback may widen the resulting union
    /// under this same reservation before materializing it.
    pub(in crate::analysis) fn join(
        &self,
        old: &PayloadSummary,
        incoming: &PayloadSummary,
        domain: Domain,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<PayloadSummary, MachineError> {
        reserve(
            budget,
            self.work_size()
                .saturating_add(old.work_size())
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let incoming = self.normalize(incoming.clone(), domain)?;
        let joined = join(old, &incoming, domain, widen)?;
        let result = self.normalize(joined, domain)?;
        MachineError::check_cancelled()?;
        Ok(result)
    }

    /// Consuming the policy stages a mode transition until its complete summary
    /// and independent roots succeed. The caller has prepaid normalization; each
    /// instantiation is reserved separately, including the one coarse fallback.
    pub(in crate::analysis) fn materialize(
        mut self,
        summary: PayloadSummary,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<(Self, PayloadSummary, MachinePayload), MachineError> {
        let mut bound = self.normalize(summary, domain)?;
        reserve(budget, bound.work_size().saturating_mul(4))?;
        let payload = match instantiate(&bound, domain) {
            Ok(payload) => payload,
            Err(error) if !self.coarse && error.permits_bound_coarsening() => {
                // Change the invariant used by all future coverage checks.
                // A policy already in coarse mode never rebuilds the same bound.
                self.coarse = true;
                bound = self.normalize(bound, domain)?;
                reserve(budget, bound.work_size().saturating_mul(4))?;
                instantiate(&bound, domain)?
            }
            Err(error) => return Err(error),
        };
        MachineError::check_cancelled()?;
        Ok((self, bound, payload))
    }
}
