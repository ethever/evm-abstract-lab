//! Native fixture assembly shared by presentation unit tests.

use crate::{
    analysis::{
        AnalysisEngine, AnalysisInput, Config, EnginePolicies, EvmAnalysis, EvmWorldAnalysis,
        ExecutionConfig, ProjectionError,
        control::Control,
        cooperation::{CooperationError, NoExtension},
    },
    bytecode::Program,
    domain::{DomainSpec, EvmNumericAlgebra, EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
    world::{EvmAccount, EvmBlobHashes, EvmByteArray, EvmEntry, EvmEnvironmentValues, EvmWorld},
};
use std::convert::Infallible;

pub(crate) type TestValue = EvmValue<NumericValue>;
pub(crate) type TestWorld = EvmWorld<NumericValue>;
pub(crate) type TestAccount = EvmAccount<NumericValue>;
pub(crate) type TestEntry = EvmEntry<NumericValue>;
pub(crate) type TestBytes = EvmByteArray<NumericValue>;
pub(crate) type TestEnvironment = EvmEnvironmentValues<NumericValue>;
pub(crate) type TestBlobHashes = EvmBlobHashes<NumericValue>;
pub(crate) type TestWorldAnalysis = EvmWorldAnalysis<NumericValue>;
pub(crate) type TestAnalysis = EvmAnalysis<NumericValue>;
pub(crate) type TestNormalizedSsa = super::NormalizedSsa<NumericValue>;

pub(crate) fn product() -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()))
}

pub(crate) fn postprocessing() -> WorkBudget {
    WorkBudget::new(2_000_000_000)
}

pub(crate) fn run_native<D: EvmNumericAlgebra>(
    algebra: &ValueAlgebra<D>,
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<Infallible>> {
    let input = AnalysisInput::new(world, entry).expect("valid native fixture input");
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    AnalysisEngine::new(algebra, &policies, &mut extension).run(input, config, &Control::default())
}

pub(crate) fn run_world(
    world: TestWorld,
    entry: TestEntry,
    config: ExecutionConfig,
) -> Result<TestWorldAnalysis, CooperationError<Infallible>> {
    run_native(&product(), world, entry, config)
}

pub(crate) fn run_program(
    program: Program,
    config: Config,
) -> Result<TestAnalysis, ProjectionError> {
    run_program_with_environment(program, config, TestEnvironment::default())
}

pub(crate) fn run_program_with_environment(
    program: Program,
    config: Config,
    environment: TestEnvironment,
) -> Result<TestAnalysis, ProjectionError> {
    let input =
        AnalysisInput::from_program(program, environment).expect("valid native program fixture");
    let algebra = product();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let native = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run(
            input,
            ExecutionConfig {
                analysis: config,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .expect("native fixture execution");
    EvmAnalysis::from_execution(native, &mut postprocessing())
}

pub(crate) fn product_with_relations(
    limits: crate::domain::relational::RelationLimits,
) -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::default().with_relations(limits),
    ))
}
pub(crate) fn product_without_relations() -> ValueAlgebra<ProductAlgebra> {
    product_with_relations(crate::domain::relational::RelationLimits {
        enabled: false,
        ..Default::default()
    })
}
