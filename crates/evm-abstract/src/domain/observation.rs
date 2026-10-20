//! Representation-independent guarantees attached to immutable EVM sources.
//!
//! An observation is a conjunction about one scoped word. It is not an
//! executable domain or a policy: no transfer, widening, or scheduling occurs
//! here. Exporting a custom element may forget predicates outside this language.

use super::{
    FiniteConstantSet, NumericValue, congruence::Congruence, facts::FactError, facts::Symbol,
    interval::Interval, known_bits::KnownBits,
};
use alloy_primitives::U256;
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{collections::BTreeSet, fmt};

/// Sound scalar guarantees for the exact expression and source cell of a binding.
/// Finite membership is a complete disjunction, never a truncated sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordObservation {
    pub(super) finite: FiniteConstantSet,
    pub(super) bits: KnownBits,
    pub(super) interval: Interval,
    pub(super) congruence: Congruence,
    pub(super) nonzero: bool,
}

impl WordObservation {
    /// No additional guarantee.
    pub fn top() -> Self {
        Self {
            finite: FiniteConstantSet::top(),
            bits: KnownBits::top(),
            interval: Interval::top(),
            congruence: Congruence::top(),
            nonzero: false,
        }
    }

    /// The observed word is exactly this literal.
    pub fn constant(value: U256) -> Self {
        Self {
            finite: FiniteConstantSet::constant(value),
            bits: KnownBits::exact(value),
            interval: Interval::exact(value),
            congruence: Congruence::exact(value),
            nonzero: value != U256::ZERO,
        }
    }

    /// A byte-format guarantee, without a claim about its source identity.
    pub fn unknown_byte() -> Self {
        Self::unsigned_range(U256::ZERO, U256::from(255))
            .expect("byte endpoints are ordered")
            .with_known_bits(KnownBits::new(U256::MAX << 8usize, U256::ZERO).unwrap())
    }

    /// An address-format guarantee, independent of account existence/code.
    pub fn unknown_address() -> Self {
        Self::unsigned_range(U256::ZERO, U256::MAX >> 96usize)
            .expect("address endpoints are ordered")
            .with_known_bits(KnownBits::new(U256::MAX << 160usize, U256::ZERO).unwrap())
    }

    /// A nonempty unsigned interval, or None for reversed endpoints.
    pub fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        Some(Self {
            interval: Interval::new_unsigned(lower, upper)?,
            ..Self::top()
        })
    }

    /// One validated mask guarantee.
    pub fn bits(bits: KnownBits) -> Self {
        Self::top().with_known_bits(bits)
    }

    /// Exclude zero without claiming a particular nonzero witness.
    pub fn nonzero_word() -> Self {
        Self {
            nonzero: true,
            ..Self::top()
        }
    }

    fn with_known_bits(mut self, bits: KnownBits) -> Self {
        self.bits = bits;
        self
    }

    /// Complete finite candidates, if represented by this observation.
    pub fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.finite.as_values()
    }

    /// Complete finite membership component.
    pub fn finite_constants(&self) -> &FiniteConstantSet {
        &self.finite
    }

    /// Guaranteed fixed bits.
    pub fn known_bits(&self) -> &KnownBits {
        &self.bits
    }

    /// Inclusive unsigned/signed bounds.
    pub fn interval(&self) -> &Interval {
        &self.interval
    }

    /// Ordinary integer congruence intersected with the Word256 universe.
    pub fn congruence(&self) -> &Congruence {
        &self.congruence
    }

    /// A definite nonzero guarantee, independent of representation redundancy.
    pub fn nonzero(&self) -> bool {
        self.nonzero
    }

    /// False excludes a concrete word; true is not an existence witness.
    pub fn contains(&self, value: U256) -> bool {
        (!self.nonzero || value != U256::ZERO)
            && self.finite.contains(value)
            && self.bits.contains(value)
            && self.interval.contains(value)
            && self.congruence.contains(value)
    }

    /// A singleton proved by one guarantee and checked against all others.
    pub fn singleton(&self) -> Option<U256> {
        let value = self
            .finite
            .singleton()
            .or_else(|| self.bits.singleton())
            .or_else(|| self.interval.singleton())
            .or_else(|| self.congruence.singleton())?;
        self.contains(value).then_some(value)
    }

    /// Zero has not been excluded.
    pub fn may_be_zero(&self) -> bool {
        self.contains(U256::ZERO)
    }

    /// This observation does not prove the word is zero.
    pub fn may_be_nonzero(&self) -> bool {
        self.singleton() != Some(U256::ZERO)
    }

    /// No numeric assertion is retained.
    pub fn is_top(&self) -> bool {
        self.finite.is_top()
            && self.bits == KnownBits::top()
            && self.interval == Interval::top()
            && self.congruence.is_top()
            && !self.nonzero
    }

    /// Conservative retained-payload work, with shared finite allocations.
    pub fn work_size(&self) -> usize {
        self.finite.work_size().saturating_add(12)
    }

    /// Intersect guarantees without running any component reduction scheduler.
    /// Contradiction is a proof; a nonempty result need not be satisfiable.
    pub fn meet(&self, other: &Self) -> Result<Self, FactError> {
        let contradiction = || FactError::Contradiction {
            subject: Symbol::THIS,
        };
        let mut out = Self {
            finite: self
                .finite
                .meet(&other.finite)
                .map_err(|_| contradiction())?,
            bits: self.bits.meet(&other.bits).ok_or_else(contradiction)?,
            interval: self
                .interval
                .meet(&other.interval)
                .ok_or_else(contradiction)?,
            congruence: self
                .congruence
                .meet(&other.congruence)
                .ok_or_else(contradiction)?,
            nonzero: self.nonzero || other.nonzero,
        };
        out.finite = out
            .finite
            .filter(|value| out.contains(value))
            .map_err(|_| contradiction())?;
        if let Some(values) = out.finite.as_values() {
            out.bits = out
                .bits
                .meet(&KnownBits::from_values(values))
                .ok_or_else(contradiction)?;
            out.interval = out
                .interval
                .meet(&Interval::from_values(values))
                .ok_or_else(contradiction)?;
            out.congruence = out
                .congruence
                .meet(&Congruence::from_values(values))
                .ok_or_else(contradiction)?;
        }
        Ok(out)
    }

    pub(super) fn from_scalar(facts: &super::facts::ScalarFacts) -> Result<Self, FactError> {
        let contradiction = || FactError::Contradiction {
            subject: Symbol::THIS,
        };
        let mut out = Self::top();
        out.finite = facts.finite_constants().clone();
        if let Some(bits) = facts.known_bits() {
            out.bits = KnownBits::new(bits.zero(), bits.one()).ok_or_else(contradiction)?;
        }
        if let Some(bounds) = facts.unsigned_bounds() {
            out.interval = out
                .interval
                .meet(
                    &Interval::new_unsigned(bounds.lower(), bounds.upper())
                        .ok_or_else(contradiction)?,
                )
                .ok_or_else(contradiction)?;
        }
        if let Some(bounds) = facts.signed_bounds() {
            out.interval = out
                .interval
                .meet(
                    &Interval::new_signed(bounds.lower(), bounds.upper())
                        .ok_or_else(contradiction)?,
                )
                .ok_or_else(contradiction)?;
        }
        if let Some(congruence) = facts.congruence() {
            out.congruence = congruence.clone();
        }
        out.nonzero = facts.nonzero();
        if facts.is_address() {
            out.bits = out
                .bits
                .meet(&KnownBits::new(U256::MAX << 160usize, U256::ZERO).unwrap())
                .ok_or_else(contradiction)?;
        }
        Ok(out)
    }

    pub(super) fn numeric(&self) -> NumericValue {
        NumericValue {
            finite: self.finite.clone(),
            bits: self.bits,
            interval: self.interval,
            congruence: self.congruence.clone(),
            nonzero: self.nonzero,
        }
    }
}

impl From<&NumericValue> for WordObservation {
    fn from(value: &NumericValue) -> Self {
        Self {
            finite: value.finite.clone(),
            bits: value.bits,
            interval: value.interval,
            congruence: value.congruence.clone(),
            nonzero: value.nonzero,
        }
    }
}
impl From<NumericValue> for WordObservation {
    fn from(value: NumericValue) -> Self {
        Self {
            finite: value.finite,
            bits: value.bits,
            interval: value.interval,
            congruence: value.congruence,
            nonzero: value.nonzero,
        }
    }
}
// Default-product callers can compare a binding's exported guarantees with the
// original numeric carrier. This compares every component, never source/value
// identity, and introduces no implicit conversion on generic executable paths.
impl PartialEq<NumericValue> for WordObservation {
    fn eq(&self, other: &NumericValue) -> bool {
        self.finite == other.finite
            && self.bits == other.bits
            && self.interval == other.interval
            && self.congruence == other.congruence
            && self.nonzero == other.nonzero
    }
}
impl PartialEq<WordObservation> for NumericValue {
    fn eq(&self, other: &WordObservation) -> bool {
        other == self
    }
}
impl Serialize for WordObservation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.is_top() {
            return serializer.serialize_str("Top");
        }
        let mut map = serializer.serialize_map(None)?;
        if let Some(values) = self.constants() {
            map.serialize_entry("Constants", values)?;
        }
        map.serialize_entry("known_bits", &self.bits)?;
        map.serialize_entry("interval", &self.interval)?;
        map.serialize_entry("congruence", &self.congruence)?;
        map.serialize_entry("nonzero", &self.nonzero)?;
        map.end()
    }
}
impl fmt::Display for WordObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.numeric().fmt(f)
    }
}
