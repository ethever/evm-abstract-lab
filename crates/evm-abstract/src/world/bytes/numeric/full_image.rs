//! Domain-owned fixed-width byte-image certificates; payment remains in Reader.
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::ValueAlgebra;
use crate::domain::{EvmNumericAlgebra, WordElement};

pub(super) fn candidate_with<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &N,
    domain: AlgebraRef<'_, D>,
) -> bool {
    !domain.spec().relations().enabled && domain.byte_image_candidate(value)
}
pub(super) fn complete<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &N,
    domain: AlgebraRef<'_, D>,
) -> bool {
    domain.byte_image_complete(value)
}
#[cfg(test)]
pub(super) fn candidate(
    value: &crate::domain::NumericValue,
    domain: crate::domain::Domain,
) -> bool {
    candidate_with(value, ValueAlgebra::new(domain).borrowed())
}
#[cfg(test)]
pub(super) fn proves(value: &crate::domain::NumericValue) -> bool {
    complete(
        value,
        ValueAlgebra::new(crate::domain::Domain::default()).borrowed(),
    )
}
