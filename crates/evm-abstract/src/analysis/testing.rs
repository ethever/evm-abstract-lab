//! Local typed fixtures. Numeric choices stay separate from execution Config.

use crate::domain::{
    DomainSpec, EvmValue, NumericValue, ProductAlgebra, ValueAlgebra, relational::RelationLimits,
};
use std::num::NonZeroUsize;

pub(in crate::analysis) type TestValue = EvmValue<NumericValue>;
pub(in crate::analysis) type Account = crate::world::EvmAccount<NumericValue>;
pub(in crate::analysis) type World = crate::world::EvmWorld<NumericValue>;
pub(in crate::analysis) type Entry = crate::world::EvmEntry<NumericValue>;
pub(in crate::analysis) type ByteArray = crate::world::EvmByteArray<NumericValue>;
pub(in crate::analysis) type ByteArraySummary = crate::world::EvmByteArraySummary<NumericValue>;
pub(in crate::analysis) type Store = crate::world::EvmStore<NumericValue>;
pub(in crate::analysis) type StoreSummary = crate::world::EvmStoreSummary<NumericValue>;
pub(in crate::analysis) type Snapshot = crate::world::EvmSnapshot<NumericValue>;
pub(in crate::analysis) type EvmEnvironment = crate::world::EvmEnvironmentValues<NumericValue>;
pub(in crate::analysis) type TestValueQuery =
    crate::domain::relational::EvmValueQuery<NumericValue>;
pub(in crate::analysis) type TestScalarQuery =
    crate::domain::relational::EvmScalarQuery<NumericValue>;

/// Editable numbers used only to construct an explicit test algebra spec.
/// It has no value operations, forwarding implementation or engine entrypoint.
#[derive(Clone, Debug)]
pub(in crate::analysis) struct NumericLimits {
    pub max_constants: usize,
    pub reduction_rounds: usize,
    pub max_facts: usize,
    pub relations: RelationLimits,
}
impl Default for NumericLimits {
    fn default() -> Self {
        Self {
            max_constants: 8,
            reduction_rounds: 4,
            max_facts: 256,
            relations: RelationLimits::default(),
        }
    }
}
impl NumericLimits {
    pub fn machine() -> Self {
        Self {
            relations: RelationLimits {
                max_nodes: 8192,
                max_depth: 128,
                max_constraints: 128,
                ..RelationLimits::default()
            },
            ..Self::default()
        }
    }
    pub fn spec(&self) -> DomainSpec {
        DomainSpec::new(
            NonZeroUsize::new(self.max_constants).unwrap(),
            NonZeroUsize::new(self.reduction_rounds).unwrap(),
            NonZeroUsize::new(self.max_facts).unwrap(),
        )
        .with_relations(self.relations)
    }
}
pub(in crate::analysis) fn product(spec: DomainSpec) -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(spec))
}

/// Explicit product execution used by concrete semantic fixtures.
pub(in crate::analysis) fn execute(
    world: World,
    entry: Entry,
    config: super::ExecutionConfig,
    spec: DomainSpec,
) -> Result<
    super::EvmWorldAnalysis<NumericValue>,
    super::cooperation::CooperationError<std::convert::Infallible>,
> {
    let algebra = product(spec);
    super::AnalysisEngine::new(
        &algebra,
        &super::EnginePolicies::default(),
        &mut super::cooperation::NoExtension,
    )
    .run(
        super::AnalysisInput::new(world, entry)?,
        config,
        &super::control::Control::default(),
    )
}

pub(in crate::analysis) type AbstractLog = crate::world::EvmLog<NumericValue>;
pub(in crate::analysis) type Reduction = crate::domain::EvmReduction<NumericValue>;
