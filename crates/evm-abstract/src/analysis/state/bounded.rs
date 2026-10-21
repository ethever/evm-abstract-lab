//! Persistent coordinates and normalization for whole-state numerical bounds.
//!
//! Feedback bounds own their summary; Runtime retries use the node's summary.
//! Both retain this same representation policy, so reprojection cannot restore
//! erased guarantees or leave coarse mode. Callers own admission and publication.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    AlgebraRef, EvmMachinePayload, EvmPayloadSummary, EvmValue, MachineError, WorkBudget,
    instantiate, join, stack_alias::StackAliases,
};

#[derive(Clone)]
pub(in crate::analysis) struct EvmBoundedState<N: WordElement> {
    layout: EvmPayloadSummary<N>,
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

pub(super) fn erase_guarantees<N: WordElement>(
    mut summary: EvmPayloadSummary<N>,
) -> Result<EvmPayloadSummary<N>, MachineError> {
    summary.storage_observations = Default::default();
    for frame in &mut summary.frames {
        frame.stack_aliases = StackAliases::form(&frame.stack, false)?;
    }
    summary.work = summary.measure_work();
    Ok(summary)
}

fn restrict<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    mut summary: EvmPayloadSummary<N>,
    layout: &EvmPayloadSummary<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmPayloadSummary<N> {
    summary.store = summary.store.project_to_layout_with(&layout.store, domain);
    for (frame, shape) in summary.frames.iter_mut().zip(&layout.frames) {
        for (bytes, shape) in frame.byte_arrays_mut().into_iter().zip(shape.byte_arrays()) {
            *bytes = bytes.project_to_layout_with(shape, domain);
        }
        frame.saved_store = frame
            .saved_store
            .project_to_layout_with(&shape.saved_store, domain);
    }
    summary
}

fn coarse<N: WordElement>(mut summary: EvmPayloadSummary<N>) -> EvmPayloadSummary<N> {
    let scalar = |value: &mut EvmValue<N>| {
        let (lower, upper) = value.interval().unsigned_bounds();
        *value = EvmValue::unsigned_range(lower, upper).expect("summary bounds are ordered");
    };
    summary.store = summary.store.forget_loop_numeric();
    for frame in &mut summary.frames {
        frame.values_mut().for_each(scalar);
        for bytes in frame.byte_arrays_mut() {
            *bytes = bytes.forget_loop_content();
        }
        frame.saved_store = frame.saved_store.forget_loop_numeric();
    }
    for continuation in &mut summary.continuations {
        scalar(&mut continuation.output_offset);
        scalar(&mut continuation.output_size);
    }
    summary
}

impl<N: WordElement> EvmBoundedState<N> {
    /// The caller prepays construction and persistent retention. Self-projection
    /// bounds the layout itself instead of retaining an exact byte buffer solely
    /// for its coordinates. Coarse content never replaces these frozen keys.
    pub(in crate::analysis) fn freeze<D: EvmNumericAlgebra<Element = N>>(
        layout: &EvmPayloadSummary<N>,
        coarse: bool,
        domain: AlgebraRef<'_, D>,
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
    pub(in crate::analysis) fn layout(&self) -> &EvmPayloadSummary<N> {
        &self.layout
    }

    fn normalize<D: EvmNumericAlgebra<Element = N>>(
        &self,
        summary: EvmPayloadSummary<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmPayloadSummary<N>, MachineError> {
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
    pub(in crate::analysis) fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        old: &EvmPayloadSummary<N>,
        incoming: &EvmPayloadSummary<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<EvmPayloadSummary<N>, MachineError> {
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
    pub(in crate::analysis) fn materialize<D: EvmNumericAlgebra<Element = N>>(
        mut self,
        summary: EvmPayloadSummary<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<(Self, EvmPayloadSummary<N>, EvmMachinePayload<N>), MachineError> {
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
