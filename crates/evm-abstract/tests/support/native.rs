//! Small native execution fixture shared by integration tests.
//!
//! Each caller chooses the actual numeric algebra and supplies a validated
//! input and an execution policy. Projection is an explicit operation on the
//! returned native graph, so these helpers introduce no alternate executor.
#![allow(dead_code)]

use std::{convert::Infallible, num::NonZeroUsize};

use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, EvmWorldAnalysis, ExecutionConfig,
        control::Control,
        cooperation::{CooperationError, NoExtension},
    },
    domain::{DomainSpec, EvmNumericAlgebra, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra},
};

pub fn spec(capacity: usize, rounds: usize, facts: usize) -> DomainSpec {
    DomainSpec::new(
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(rounds).unwrap(),
        NonZeroUsize::new(facts).unwrap(),
    )
}

pub fn product() -> ValueAlgebra<ProductAlgebra> {
    product_with(spec(8, 4, 256))
}

pub fn product_with(spec: DomainSpec) -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(spec))
}

pub fn finite_with(spec: DomainSpec) -> ValueAlgebra<FiniteSetAlgebra> {
    ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec))
}

pub fn run<D: EvmNumericAlgebra>(
    values: &ValueAlgebra<D>,
    input: AnalysisInput<D::Element>,
    config: ExecutionConfig,
    control: &Control,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<Infallible>> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    AnalysisEngine::new(values, &policies, &mut extension).run(input, config, control)
}

/// Independent finite ledger for a test's explicitly selected output operation.
/// Execution budgets remain on ExecutionConfig and are never reused here.
pub fn postprocessing() -> evm_abstract::resource::WorkBudget {
    evm_abstract::resource::WorkBudget::new(2_000_000_000)
}
