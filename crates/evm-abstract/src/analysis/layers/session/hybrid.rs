//! Hybrid execution projects the same immutable facts and retains the session payer.

use super::{AnalysisSession, SessionError, project_input, session_clone_error};
use crate::{
    analysis::{
        ContextPolicy, HybridAnalysis, HybridConfig, PartitionPolicy, StatePolicy,
        control::Control, cooperation::Composition, transfer::StorageReadPolicy,
    },
    domain::EvmNumericAlgebra,
    resource::WorkBudget,
};

type HybridRunResult<N, E> = Result<HybridAnalysis<N>, SessionError<E>>;

impl<D, C, P, S, X> AnalysisSession<'_, D, C, P, S, X>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    pub(in crate::analysis) fn run_hybrid_uncached(
        &mut self,
        hybrid: HybridConfig,
        storage_reads: StorageReadPolicy,
        control: &Control,
    ) -> HybridRunResult<D::Element, <X as Composition<D::Element>>::Error> {
        control.scope(|| {
            control.checkpoint()?;
            let entry_code = self.entry_code();
            let budget = self
                .budget
                .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
            // Unlike charge(0), this rejects a sticky work refusal before even
            // quoting another projection or admitting a second interpretation.
            crate::resource::clone_payment::checkpoint(budget).map_err(session_clone_error)?;
            self.counters.set_observer(control.observer());
            // Preserve the original Entry.binding and exact fact interpretation.
            // Only the hybrid owner's paid detached native copy may materialize
            // an additional namespace condition; this retained input never does.
            let input = project_input(
                &self.catalog,
                &self.input,
                entry_code,
                self.engine.algebra.borrowed(),
                budget,
            )?;
            let report = self.engine.run_hybrid_metered(
                input,
                self.config.clone(),
                hybrid,
                budget,
                &mut self.counters,
                storage_reads,
            )?;
            if budget.cancellation().is_cancelled() {
                return Err(SessionError::Cancelled(crate::analysis::control::Cancelled));
            }
            control.checkpoint()?;
            self.has_executed = true;
            self.executed_revision = Some(self.input.revision());
            self.pending_changes.clear();
            self.latest = None;
            Ok(report)
        })
    }

    pub(in crate::analysis) fn limit_precision_work(
        &mut self,
        allowance: usize,
    ) -> Result<(), SessionError<<X as Composition<D::Element>>::Error>> {
        self.budget
            .get_or_insert_with(|| WorkBudget::new(self.config.max_work))
            .limit_remaining(allowance)
            .map_err(session_clone_error)
    }

    pub(in crate::analysis) fn observation_count(&self) -> usize {
        self.input.facts.observation_count()
    }

    pub(in crate::analysis) fn payer_cancelled(&self) -> bool {
        self.budget
            .as_ref()
            .is_some_and(|budget| budget.cancellation().is_cancelled())
    }
}
