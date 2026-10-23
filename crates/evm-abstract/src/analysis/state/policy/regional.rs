//! Frozen relative coordinates with retained scalar control facts.
//!
//! Each admitted summary anchors the next layout. Projection may discard
//! coordinates but cannot add byte, word, store, lifecycle or log coordinates
//! outside that anchor. Scalar bounds cover growing inputs independently;
//! stack must-equalities can only split, and storage observations stay erased.

use super::{
    Acceleration, EvmBoundedState, EvmNumericAlgebra, EvmPayloadSummary, MachineError,
    PolicyContext, ProcedureOutput, WordElement,
};

#[cfg(test)]
mod tariff_tests;
#[cfg(test)]
mod tests;

impl<N: WordElement> EvmPayloadSummary<N> {
    /// Cover both relative inputs in frozen coordinates using the run's native
    /// scalar algebra. Preserve only stack aliases common to both inputs;
    /// independent instantiation owns fresh roots and storage snapshots.
    /// Incompatible execution shapes and resource refusals remain typed errors.
    pub fn regional_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let representation =
            EvmBoundedState::freeze_with_payment(self, false, cx.domain, cx.budget)?;
        let mut joined = representation.join_with_payment(
            self,
            incoming,
            cx.domain,
            cx.budget,
            acceleration == Acceleration::Widen,
        )?;
        // Bounded normalization erases aliases. Restore the actual inputs'
        // intersection only after the join proves their shapes compatible;
        // matching scalar bounds alone never justify an alias.
        let aliases =
            self.frames
                .iter()
                .zip(&incoming.frames)
                .fold(0usize, |work, (previous, next)| {
                    work.saturating_add(previous.stack_aliases.work_size())
                        .saturating_add(next.stack_aliases.work_size())
                });
        cx.reserve(aliases)?;
        for ((frame, previous), next) in joined
            .frames
            .iter_mut()
            .zip(&self.frames)
            .zip(&incoming.frames)
        {
            frame.stack_aliases = previous.stack_aliases.intersect(&next.stack_aliases)?;
        }
        joined.work = joined.measure_work();
        cx.reserve(0)?;
        Ok(joined)
    }
}

impl<N: WordElement> ProcedureOutput<N> {
    /// Cover return/revert data and transaction effects in the current output's
    /// frozen coordinates, retaining their native scalar bounds. Omitted bytes
    /// remain arbitrary and omitted store cells flow into conservative defaults.
    /// The procedure keeps a separate retained anchor for each outcome kind.
    pub fn regional_join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        cx.reserve(self.work_size().saturating_add(incoming.work_size()))?;
        let widen = acceleration == Acceleration::Widen;
        // Self-projection bounds an initially Exact buffer too; otherwise the
        // retained layout itself would keep the entire original buffer alive.
        let data = self
            .data
            .project_to_layout_with_payment(&self.data, cx.domain, cx.budget)?;
        let incoming_data = incoming
            .data
            .project_to_layout_with_payment(&self.data, cx.domain, cx.budget)?;
        let data = data.join_with_payment(&incoming_data, cx.domain, widen, cx.budget)?;
        let store = self
            .store
            .project_to_layout_with_payment(&self.store, cx.domain, cx.budget)?;
        let incoming_store =
            incoming
                .store
                .project_to_layout_with_payment(&self.store, cx.domain, cx.budget)?;
        let store = store.join_with_payment(&incoming_store, cx.domain, widen, cx.budget)?;
        Self::from_summaries(data, store, cx)
    }
}
