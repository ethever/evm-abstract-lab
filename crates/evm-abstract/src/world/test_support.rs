//! Explicit built-in specializations for world fixtures.

pub(super) type TestValue = crate::domain::EvmValue<crate::domain::NumericValue>;
pub(super) type TestAlgebra = crate::domain::ValueAlgebra<crate::domain::ProductAlgebra>;
pub(super) type TestAccount = super::EvmAccount<crate::domain::NumericValue>;
pub(super) type TestWorld = super::EvmWorld<crate::domain::NumericValue>;
pub(super) type TestEntry = super::EvmEntry<crate::domain::NumericValue>;
pub(super) type TestBytes = super::EvmByteArray<crate::domain::NumericValue>;
pub(super) type TestBytesSummary = super::EvmByteArraySummary<crate::domain::NumericValue>;
pub(super) type TestStore = super::EvmStore<crate::domain::NumericValue>;
pub(super) type TestSnapshot = super::EvmSnapshot<crate::domain::NumericValue>;
pub(super) type TestLog = super::EvmLog<crate::domain::NumericValue>;
pub(super) type TestEnvironment = super::EvmEnvironmentValues<crate::domain::NumericValue>;

/// A product fixture with expression construction disabled at its source.
pub(super) fn numeric_product(algebra: TestAlgebra) -> TestAlgebra {
    let mut relations = algebra.spec().relations();
    relations.enabled = false;
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        algebra.spec().with_relations(relations),
    ))
}
