//! Metered public operations over opaque relative and output representations.
//!
//! Preparation drops only optional guarantees. Code, owners, rollback state and
//! continuations are retained; shape incompatibility is a typed refusal.

use crate::{
    analysis::{
        engine::policy::{Acceleration, PolicyContext},
        machine::{EvmMachinePayload, FrameKey, MachineError, MachineKey},
    },
    domain::{EvmNumericAlgebra, WordElement},
    world::{EvmByteArray, EvmByteArraySummary, EvmStore, EvmStoreSummary},
};

use super::{EvmPayloadSummary, bounded::EvmBoundedState, payload};

mod regional;

#[cfg(test)]
mod tests;

impl<N: WordElement> EvmPayloadSummary<N> {
    /// Copy the complete native summary, preserving aliases and observations.
    pub fn copy_for_policy<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        self.clone_with_payment(cx.budget)
    }

    /// Structural report projection, independent of the policy's actual key.
    pub fn key(&self) -> &MachineKey {
        &self.location
    }

    /// Shape of the active relative frame, without exposing summary internals.
    pub fn active_frame(&self) -> &FrameKey {
        &self.active().key
    }

    /// Project complete input to independent numerical templates. The caller's
    /// context histories are preserved; context normalization precedes this API.
    pub fn project<D: EvmNumericAlgebra<Element = N>>(
        input: &EvmMachinePayload<N>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let summary = payload::caller::form_normalized_with_payment(
            input,
            cx.environment,
            cx.domain,
            cx.budget,
        )?;
        cx.reserve(0)?;
        Ok(summary)
    }

    /// Instantiate independent source roots, sharing only justified aliases
    /// within this invocation. No root identity is reused between invocations.
    pub fn instantiate<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<EvmMachinePayload<N>, MachineError> {
        let input = payload::caller::instantiate_with_payment(self, cx.domain, cx.budget)?;
        cx.reserve(0)?;
        Ok(input)
    }

    /// Cover both relative inputs. Numerical widening is requested separately
    /// from the always-required covering join; different shapes are refused.
    pub fn covering_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        // Independent replays can have the same complete numerical bound.
        // Prove idempotence before rebuilding every byte/store template;
        // explicit widening remains an independent operation.
        if acceleration == Acceleration::Join
            && (std::ptr::eq(self, incoming)
                || self.eq_with_payment(incoming, cx.domain, cx.budget)?)
        {
            let joined = self.clone_with_payment(cx.budget)?;
            cx.reserve(0)?;
            return Ok(joined);
        }
        let joined = self.join_with_payment(
            incoming,
            cx.domain,
            acceleration == Acceleration::Widen,
            cx.budget,
        )?;
        cx.reserve(0)?;
        Ok(joined)
    }

    /// Erase optional jump contexts in both frames and the location projection
    /// before selecting a coarser key. Executable frame roles remain unchanged.
    pub fn forget_jump_history<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let mut prepared = self.clone_with_payment(cx.budget)?;
        for frame in &mut prepared.frames {
            frame.key.jump_history.clear();
        }
        for frame in &mut prepared.location.frames {
            frame.jump_history.clear();
        }
        prepared.work = prepared.measure_work();
        cx.reserve(0)?;
        Ok(prepared)
    }

    /// Cover this input using interval scalars, arbitrary byte contents and
    /// unknown numerical stores. Stack aliases and storage-read guarantees are
    /// erased; structural code and lifecycle facts retain their lawful meaning.
    pub fn coarsen<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        self.bounded_join(self, Acceleration::Join, cx)
    }

    /// Freeze coordinates from the retained input and admit a covering coarse
    /// union. Incoming coordinates cannot grow the retained byte/store layout;
    /// numerical widening remains controlled by `acceleration`.
    pub fn bounded_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let representation =
            EvmBoundedState::freeze_with_payment(self, true, cx.domain, cx.budget)?;
        let joined = representation.join_with_payment(
            self,
            incoming,
            cx.domain,
            cx.budget,
            acceleration == Acceleration::Widen,
        )?;
        cx.reserve(0)?;
        Ok(joined)
    }

    /// Compare semantic templates, excluding cached work and report location.
    pub fn semantic_eq<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<bool, MachineError> {
        let equal = self.eq_with_payment(other, cx.domain, cx.budget)?;
        cx.reserve(0)?;
        Ok(equal)
    }
}

/// A terminal procedure's independent numerical data and transaction effects.
/// Source identities are projected away; solver generations are not semantic
/// content and remain with the publication owner. Equality compares semantic
/// content. Derived Clone/Eq remain caller-funded; metered callers use the
/// complete native invoices and `eq_with_payment`, not structural sizes.
#[derive(Clone, Debug)]
pub struct ProcedureOutput<N: WordElement> {
    pub(in crate::analysis) data: EvmByteArraySummary<N>,
    pub(in crate::analysis) store: EvmStoreSummary<N>,
    pub(in crate::analysis) work: usize,
}

impl<N: WordElement> PartialEq for ProcedureOutput<N> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data && self.store == other.store
    }
}

impl<N: WordElement> Eq for ProcedureOutput<N> {}

impl<N: WordElement> ProcedureOutput<N> {
    /// Project executable output under payment, retaining all numerical effects.
    pub fn project<D: EvmNumericAlgebra<Element = N>>(
        data: &EvmByteArray<N>,
        store: &EvmStore<N>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let data = data.summary_projection_with_payment(cx.domain, cx.budget)?;
        let store = store.summary_projection_with_payment(cx.domain, cx.budget)?;
        Self::from_summaries(data, store, cx)
    }

    /// Construct from owned numerical projections. This does not publish or
    /// admit an output: even the first output passes through its summary policy.
    pub fn from_summaries<D: EvmNumericAlgebra<Element = N>>(
        data: EvmByteArraySummary<N>,
        store: EvmStoreSummary<N>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let work = data.work_size().saturating_add(store.work_size());
        cx.reserve(work)?;
        let output = Self { data, store, work };
        cx.reserve(0)?;
        Ok(output)
    }

    pub(in crate::analysis) fn clone_work_with(
        &self,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<usize, MachineError> {
        crate::resource::clone_payment::pay(budget, 1)?;
        let work = self
            .data
            .clone_work_with(budget)?
            .saturating_add(self.store.clone_work_with(budget)?)
            .saturating_add(4);
        Ok(crate::resource::clone_payment::finish_quote(work, budget)?)
    }

    pub(in crate::analysis) fn clone_with_payment(
        &self,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<Self, MachineError> {
        let work = self.clone_work_with(budget)?;
        crate::resource::clone_payment::pay(budget, work)?;
        let result = self.clone();
        crate::resource::clone_payment::checkpoint(budget)?;
        Ok(result)
    }

    pub(in crate::analysis) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: crate::domain::AlgebraRef<'_, D>,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<bool, MachineError> {
        Ok(self.data.eq_with_payment(&other.data, domain, budget)?
            && self.store.eq_with_payment(&other.store, domain, budget)?)
    }

    /// Independent return/revert bytes, without exposing mutable internals.
    pub fn data(&self) -> &EvmByteArraySummary<N> {
        &self.data
    }

    /// Shared transaction and lifecycle effects of this outcome.
    pub fn store(&self) -> &EvmStoreSummary<N> {
        &self.store
    }

    /// Conservative retained-data and independent-replay quote.
    pub fn work_size(&self) -> usize {
        self.work
    }

    /// Cover both terminal outputs under the independently chosen schedule.
    pub fn covering_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        if acceleration == Acceleration::Join && std::ptr::eq(self, incoming) {
            let joined = self.clone_with_payment(cx.budget)?;
            cx.reserve(0)?;
            return Ok(joined);
        }
        cx.reserve(
            self.work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(4),
        )?;
        let widen = acceleration == Acceleration::Widen;
        let data = self
            .data
            .join_with_payment(&incoming.data, cx.domain, widen, cx.budget)?;
        let store = self
            .store
            .join_with_payment(&incoming.store, cx.domain, widen, cx.budget)?;
        Self::from_summaries(data, store, cx)
    }

    /// A covering output with arbitrary content and numerical store cells.
    /// Control-relevant lifecycle, code and touch facts remain represented.
    pub fn coarsen<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        cx.reserve(self.work_size().saturating_mul(4))?;
        let data = self
            .data
            .forget_loop_content_with_payment(cx.domain, cx.budget)?;
        let store = self
            .store
            .forget_loop_numeric_with_payment(cx.domain, cx.budget)?;
        Self::from_summaries(data, store, cx)
    }

    /// Freeze retained coordinates, drop optional numerical correlations, and
    /// cover incoming effects. Forgotten contents are arbitrary, never zero.
    pub fn bounded_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        cx.reserve(
            self.work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let widen = acceleration == Acceleration::Widen;
        let data = self
            .data
            .forget_loop_content_with_payment(cx.domain, cx.budget)?;
        let incoming_data = incoming
            .data
            .project_to_layout_with_payment(&self.data, cx.domain, cx.budget)?
            .forget_loop_content_with_payment(cx.domain, cx.budget)?;
        let data = data.join_with_payment(&incoming_data, cx.domain, widen, cx.budget)?;
        let store = self
            .store
            .forget_loop_numeric_with_payment(cx.domain, cx.budget)?;
        let incoming_store = incoming
            .store
            .project_to_layout_with_payment(&self.store, cx.domain, cx.budget)?
            .forget_loop_numeric_with_payment(cx.domain, cx.budget)?;
        let store = store.join_with_payment(&incoming_store, cx.domain, widen, cx.budget)?;
        Self::from_summaries(data, store, cx)
    }
}
