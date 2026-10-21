use super::{CooperationError, EvmContext, Extension, ExtraStores, InputTasks};
use crate::{
    analysis::{
        ContextPolicy, EnginePolicies, EvmWorldAnalysis, ExecutionConfig, PartitionPolicy, StatePolicy,
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
/// valid provider stack guarantees and automatically replaces their generations when
/// global inputs grow. All participants share dependency scheduling, closure,
/// cancellation and the transaction work ledger. The returned native graph
/// retains the actual `D::Element`; the numeric algebra is independent of this
/// program-level extension assembly.
/// Providers may establish invariants absent from the EVM carrier, provided
/// they hold for every concrete execution under the same scoped assumptions.
/// Registration is a trusted semantic contract, not automatic proof of user code.
pub fn analyze_world_cooperating<D, C, P, H, A, S, F, G, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P, H>,
    extension: &mut Extension<A, S, F, G>,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    H: StatePolicy<D, P::Class>,
    S: InputTasks,
    F: ExtraStores,
    G: ExtraStores,
    G::Stores: analysis_kernel::ReclaimScope,
    for<'r, 'k, 'm> A: Analyzer<
            EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores, G::Stores>,
            Task = S::Task,
            Error = E,
        >,
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
pub fn analyze_world_cooperating_with_observer<D, C, P, H, A, S, F, G, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P, H>,
    extension: &mut Extension<A, S, F, G>,
    observer: &Observer,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    H: StatePolicy<D, P::Class>,
    S: InputTasks,
    F: ExtraStores,
    G: ExtraStores,
    G::Stores: analysis_kernel::ReclaimScope,
    for<'r, 'k, 'm> A: Analyzer<
            EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores, G::Stores>,
            Task = S::Task,
            Error = E,
        >,
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
pub fn analyze_world_cooperating_with_control<D, C, P, H, A, S, F, G, E>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    algebra: &ValueAlgebra<D>,
    policies: &EnginePolicies<C, P, H>,
    extension: &mut Extension<A, S, F, G>,
    control: &Control,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    H: StatePolicy<D, P::Class>,
    S: InputTasks,
    F: ExtraStores,
    G: ExtraStores,
    G::Stores: analysis_kernel::ReclaimScope,
    for<'r, 'k, 'm> A: Analyzer<
            EvmContext<'r, 'k, 'm, D::Element, S::Task, F::Stores, G::Stores>,
            Task = S::Task,
            Error = E,
        >,
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
