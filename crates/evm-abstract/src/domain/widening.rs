//! Acceleration of canonical numerical procedure bounds.
//!
//! Moving interval prefixes must not undo widening during the next reduction.
//! Bits independent of those prefixes, such as a fixed low-160-bit address,
//! remain guarantees of the widened input.

#[cfg(test)]
use super::{AbstractValue, Domain};

#[cfg(test)]
mod tests;

#[cfg(test)]
impl Domain {
    pub(crate) fn widen_procedure(
        self,
        old: &AbstractValue,
        next: &AbstractValue,
    ) -> AbstractValue {
        self.algebra().widen_procedure(old, next)
    }
}
