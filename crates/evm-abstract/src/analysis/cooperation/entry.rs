use super::{CooperationError, EvmContext, Extension, ExtraStores, InputTasks};
use crate::{
    analysis::{
        ContextPolicy, EnginePolicies, EvmWorldAnalysis, ExecutionConfig, PartitionPolicy,
        control::Control, engine, progress::Observer,
    },
    domain::{EvmNumericAlgebra, ValueAlgebra},
    world::{EvmEntry, EvmWorld},
};
use analysis_kernel::Analyzer;

/// Jointly analyze EVM execution and independently authored static components.
///
/// Components implement the same Analyzer/Read/Accepts/Registered contracts as
/// the kernel. The interpreter publishes immutable production inputs, consumes
/// entailed local stack facts and automatically replaces their generations when
/// global inputs grow. All participants share dependency scheduling, closure,
/// cancellation and the transaction work ledger. The returned native graph
/// retains the actual `D::Element`; the numeric algebra is independent of this
/// program-level extension assembly.
pub fn analyze_world_cooperating<D, C, P, A, S, F, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P>,
    extension: &mut Extension<A, S, F>,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: InputTasks,
    F: ExtraStores,
    for<'r, 'k, 'm> A:
        Analyzer<EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores>, Task = S::Task, Error = E>,
{
    analyze_world_cooperating_with_observer(
        world,
        entry,
        config,
        algebra,
        policies,
        extension,
        &Observer::default(),
    )
}

/// Joint composition under the existing bounded execution observer contract.
/// The observation handle does not carry semantic analyzer feedback.
pub fn analyze_world_cooperating_with_observer<D, C, P, A, S, F, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P>,
    extension: &mut Extension<A, S, F>,
    observer: &Observer,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: InputTasks,
    F: ExtraStores,
    for<'r, 'k, 'm> A:
        Analyzer<EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores>, Task = S::Task, Error = E>,
{
    engine::run_world_cooperating(
        world,
        entry,
        config,
        algebra.borrowed(),
        policies,
        extension,
        observer,
    )
}

/// Joint composition with explicit application cancellation. Pre/post checks
/// preserve the default controlled API's lifetime and no-partial-report contract.
pub fn analyze_world_cooperating_with_control<D, C, P, A, S, F, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P>,
    extension: &mut Extension<A, S, F>,
    control: &Control,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: InputTasks,
    F: ExtraStores,
    for<'r, 'k, 'm> A:
        Analyzer<EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores>, Task = S::Task, Error = E>,
{
    control
        .checkpoint()
        .map_err(|_| CooperationError::Cancelled)?;
    let result = control.scope(|| {
        analyze_world_cooperating_with_observer(
            world,
            entry,
            config,
            algebra,
            policies,
            extension,
            control.observer(),
        )
    });
    control
        .checkpoint()
        .map_err(|_| CooperationError::Cancelled)?;
    result
}
