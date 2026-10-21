//! A fixed whole-state representation with separately scheduled widening.
//!
//! The layout and coarse fallback persist across Join/Widen decisions. Reusing
//! independent roots is never the coverage criterion; only the retained bound
//! establishes that the complete incoming state is covered.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    AlgebraRef, EvmEntry, EvmMachinePayload, EvmPayloadSummary, FrameCode, MachineError,
    WorkBudget,
    bounded::{EvmBoundedState, erase_guarantees, reserve},
    form_normalized, join, semantic_eq,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(in crate::analysis) struct EvmLoopBound<N: WordElement> {
    representation: EvmBoundedState<N>,
    bound: EvmPayloadSummary<N>,
}

fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    payload: &EvmMachinePayload<N>,
    entry: &EvmEntry<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmPayloadSummary<N>, MachineError> {
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
    erase_guarantees(form_normalized(payload, entry, domain)?)
}

impl<N: WordElement> EvmLoopBound<N> {
    /// Compatibility entry point for an explicitly accelerated admission.
    pub(in crate::analysis) fn new<D: EvmNumericAlgebra<Element = N>>(
        old: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        entry: &EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<(Self, EvmMachinePayload<N>), MachineError> {
        Self::new_scheduled(old, incoming, entry, domain, budget, true)
    }

    pub(in crate::analysis) fn new_scheduled<D: EvmNumericAlgebra<Element = N>>(
        old: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        entry: &EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<(Self, EvmMachinePayload<N>), MachineError> {
        reserve(budget, old.work_size().saturating_add(incoming.work_size()))?;
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
        let representation = EvmBoundedState::freeze(&layout, false, domain)?;
        let bound = if widen {
            join(&old, &incoming, domain, true)?
        } else {
            layout
        };
        let (representation, bound, payload) = representation.materialize(bound, domain, budget)?;
        Ok((
            Self {
                representation,
                bound,
            },
            payload,
        ))
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        self.representation
            .work_size()
            .saturating_add(self.bound.work_size())
    }

    /// Compatibility update for an explicitly accelerated admission.
    pub(in crate::analysis) fn update<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &EvmMachinePayload<N>,
        entry: &EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<(Self, EvmMachinePayload<N>)>, MachineError> {
        self.update_scheduled(incoming, entry, domain, budget, true)
    }

    pub(in crate::analysis) fn update_scheduled<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &EvmMachinePayload<N>,
        entry: &EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<Option<(Self, EvmMachinePayload<N>)>, MachineError> {
        reserve(budget, incoming.work_size())?;
        if incoming.key() != *self.representation.location() {
            return Err(MachineError::Shape);
        }
        let incoming = project(incoming, entry, domain, budget)?;
        let joined = self
            .representation
            .join(&self.bound, &incoming, domain, budget, false)?;
        if semantic_eq(&joined, &self.bound) {
            MachineError::check_cancelled()?;
            return Ok(None);
        }
        let bound = if widen {
            join(&self.bound, &joined, domain, true)?
        } else {
            joined
        };
        let (representation, bound, payload) = self
            .representation
            .clone()
            .materialize(bound, domain, budget)?;
        Ok(Some((
            Self {
                representation,
                bound,
            },
            payload,
        )))
    }
}

#[cfg(test)]
type LoopBound = EvmLoopBound<crate::domain::NumericValue>;
