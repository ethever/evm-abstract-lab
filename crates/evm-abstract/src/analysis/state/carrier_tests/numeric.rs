//! Independent range-plus-excluded-word carrier. It never stores NumericValue.

use super::{FactError, U256, WordElement, WordObservation, fmt};
use crate::domain::{
    FiniteConstantSet, congruence::Congruence, facts::Symbol, interval::Interval,
    known_bits::KnownBits,
};
use serde::Serialize;
use std::{collections::BTreeSet, sync::LazyLock};

static FINITE: LazyLock<FiniteConstantSet> = LazyLock::new(FiniteConstantSet::top);
static BITS: LazyLock<KnownBits> = LazyLock::new(KnownBits::top);
static CONGRUENCE: LazyLock<Congruence> = LazyLock::new(Congruence::top);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct ExcludingWord {
    bounds: Interval,
    excluded: Option<U256>,
}

impl ExcludingWord {
    pub(super) fn excluding(word: u64) -> Self {
        Self {
            bounds: Interval::new_unsigned(U256::ZERO, U256::from(255)).unwrap(),
            excluded: Some(U256::from(word)),
        }
    }
    pub(super) fn excluded(&self) -> Option<U256> {
        self.excluded
    }
    pub(super) fn join(&self, other: &Self) -> Self {
        Self {
            bounds: self.bounds.join(&other.bounds),
            excluded: self.excluded.filter(|_| self.excluded == other.excluded),
        }
    }
    pub(super) fn widen(&self, other: &Self) -> Self {
        Self {
            bounds: self.bounds.widen(&other.bounds),
            ..self.join(other)
        }
    }
}

impl fmt::Display for ExcludingWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} excluding {:?}", self.bounds, self.excluded)
    }
}

impl WordElement for ExcludingWord {
    fn top() -> Self {
        Self {
            bounds: Interval::top(),
            excluded: None,
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            bounds: Interval::exact(word),
            excluded: None,
        }
    }
    fn unknown_byte() -> Self {
        Self::unsigned_range(U256::ZERO, U256::from(255)).unwrap()
    }
    fn unknown_address() -> Self {
        Self::unsigned_range(U256::ZERO, U256::MAX >> 96usize).unwrap()
    }
    fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        Some(Self {
            bounds: Interval::new_unsigned(lower, upper)?,
            excluded: None,
        })
    }
    fn from_observation(facts: &WordObservation) -> Self {
        facts.singleton().map(Self::constant).unwrap_or(Self {
            bounds: *facts.interval(),
            excluded: None,
        })
    }
    fn observation(&self) -> WordObservation {
        if let Some(word) = self.singleton() {
            return WordObservation::constant(word);
        }
        let (lower, upper) = self.bounds.unsigned_bounds();
        WordObservation::unsigned_range(lower, upper).unwrap()
    }
    fn constrain(&self, facts: &WordObservation) -> Result<Self, FactError> {
        let contradiction = || FactError::Contradiction {
            subject: Symbol::THIS,
        };
        if let Some(word) = facts.singleton() {
            return self
                .contains(word)
                .then(|| Self::constant(word))
                .ok_or_else(contradiction);
        }
        Ok(Self {
            bounds: self
                .bounds
                .meet(facts.interval())
                .ok_or_else(contradiction)?,
            excluded: self.excluded,
        })
    }
    // This carrier proves exact literals without storing any finite set.
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        None
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        &FINITE
    }
    fn known_bits(&self) -> &KnownBits {
        &BITS
    }
    fn interval(&self) -> &Interval {
        &self.bounds
    }
    fn congruence(&self) -> &Congruence {
        &CONGRUENCE
    }
    fn contains(&self, word: U256) -> bool {
        self.bounds.contains(word) && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        self.bounds.singleton().filter(|word| self.contains(*word))
    }
    fn may_be_zero(&self) -> bool {
        self.contains(U256::ZERO)
    }
    fn may_be_nonzero(&self) -> bool {
        self.singleton() != Some(U256::ZERO)
    }
    fn is_top(&self) -> bool {
        self.bounds.is_top() && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        // Includes construction and retention of the exported range observation.
        32
    }
}
