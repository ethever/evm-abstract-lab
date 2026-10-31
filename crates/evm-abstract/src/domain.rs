//! Shared numeric and bounded symbolic semantics used by the execution adapters.

pub use evm_domain::{
    AlgebraRef, BuiltinWork, DomainSpec, DomainSpecError, EmptyFiniteConstantSet,
    EvmNumericAlgebra, EvmReduction, EvmValue, FiniteConstantSet, FiniteSetAlgebra, InputIdentity,
    NumericReduction, NumericValue, ProductAlgebra, ReductionStatus, RuntimeIdentity,
    SummaryConstruction, ValueAlgebra, ValueIdentity, WordElement, WordObservation, WorkExhausted,
    congruence, cooperation, facts, finite_constant_set, identity, interval, known_bits,
    provenance, relational, symbolic,
};
pub(crate) use evm_domain::{SingletonProof, concrete, summary};

#[cfg(test)]
mod extraction_tests;
