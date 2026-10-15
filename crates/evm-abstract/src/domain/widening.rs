//! Acceleration of canonical numerical procedure bounds.
//!
//! Moving interval prefixes must not undo widening during the next reduction.
//! Bits independent of those prefixes, such as a fixed low-160-bit address,
//! remain guarantees of the widened input.

use super::{
    AbstractValue, Domain, FiniteConstantSet, NumericValue, Profile, known_bits::KnownBits,
};

#[cfg(test)]
mod tests;

impl Domain {
    pub(crate) fn widen_procedure(
        self,
        old: &AbstractValue,
        joined: &AbstractValue,
    ) -> AbstractValue {
        if old.numeric() == joined.numeric()
            || joined.constants().is_some_and(|values| {
                values.len() <= self.spec().widening_after_updates().saturating_add(1)
            })
        {
            return joined.clone();
        }
        if self.spec().profile() == Profile::ConstantsOnly {
            return AbstractValue::from_numeric(NumericValue::top());
        }
        let previous_bounds = old.interval().unsigned_bounds();
        let joined_bounds = joined.interval().unsigned_bounds();
        if previous_bounds == joined_bounds {
            return joined.clone();
        }
        let mut widened = self.widen(old, joined);
        let widened_bounds = widened.interval().unsigned_bounds();
        if joined_bounds != widened_bounds {
            let implied = KnownBits::from_unsigned_bounds(joined_bounds.0, joined_bounds.1);
            let retained = KnownBits::from_unsigned_bounds(widened_bounds.0, widened_bounds.1);
            let discard = (implied.zero() | implied.one()) & !(retained.zero() | retained.one());
            let bits = joined.known_bits();
            widened.numeric.bits = KnownBits::new(bits.zero() & !discard, bits.one() & !discard)
                .expect("removing guarantees preserves disjoint bit masks");
            widened.numeric.finite = FiniteConstantSet::top();
        }
        widened
    }
}
