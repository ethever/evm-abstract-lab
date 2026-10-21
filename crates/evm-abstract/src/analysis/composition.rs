//! One borrowed static composition for every native execution carrier.

use super::{
    AnalysisInput, ContextPolicy, EnginePolicies, EvmWorldAnalysis, ExecutionConfig,
    PartitionPolicy, StatePolicy,
    control::Control,
    cooperation::{Composition, CooperationError},
    engine,
};
use crate::{
    domain::{EvmNumericAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

/// Frozen numeric and control policies plus a borrowed mutable analyzer assembly.
/// None of the policies or analyzers needs Clone, Copy or a static lifetime.
pub struct AnalysisEngine<'a, D: EvmNumericAlgebra, C, P, S, X> {
    pub(super) algebra: &'a ValueAlgebra<D>,
    pub(super) policies: &'a EnginePolicies<C, P, S>,
    pub(super) extension: &'a mut X,
}
impl<'a, D: EvmNumericAlgebra, C, P, S, X> AnalysisEngine<'a, D, C, P, S, X> {
    /// Borrow the actual policies once; execution never reconstructs a domain.
    pub fn new(
        algebra: &'a ValueAlgebra<D>,
        policies: &'a EnginePolicies<C, P, S>,
        extension: &'a mut X,
    ) -> Self {
        Self {
            algebra,
            policies,
            extension,
        }
    }
}
impl<D, C, P, S, X> AnalysisEngine<'_, D, C, P, S, X>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    /// Execute one validated root under the caller's observation/cancellation scope.
    pub fn run(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        control: &Control,
    ) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<<X as Composition<D::Element>>::Error>>
    {
        control.scope(|| {
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            let mut budget = WorkBudget::new(config.max_work);
            let mut counters = engine::Counters::with_observer(control.observer());
            let result = self.run_metered(
                input,
                config,
                &mut budget,
                &mut counters,
                super::transfer::StorageReadPolicy::Abstract,
                None,
            )?;
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            Ok(result)
        })
    }
    pub(super) fn run_metered(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        budget: &mut WorkBudget,
        counters: &mut engine::Counters,
        storage_reads: super::transfer::StorageReadPolicy,
        reuse: Option<&mut engine::EvmExecutionReuse<D::Element>>,
    ) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<<X as Composition<D::Element>>::Error>>
    {
        engine::run_reusing(
            input.world,
            input.entry,
            config,
            self.algebra.borrowed(),
            self.policies,
            budget,
            counters,
            storage_reads,
            reuse,
            (input.facts.snapshot(), input.facts.revision()),
            engine::cooperation::Cooperation::new(engine::cooperation::Borrowed(
                &mut *self.extension,
            )),
        )
    }
}
