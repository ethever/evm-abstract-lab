//! Intervals with arbitrary excluded words: executable data, not an observation wrapper.
use alloy_primitives::U256;
use evm_abstract::domain::{
    WordElement, WordObservation,
    facts::{FactError, Symbol},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Serialize)]
pub struct ExcludingWord {
    pub lower: U256,
    pub upper: U256,
    pub excluded: BTreeSet<U256>,
    #[serde(skip)]
    pub clone_counter: Option<Arc<AtomicUsize>>,
}
impl Clone for ExcludingWord {
    fn clone(&self) -> Self {
        if let Some(counter) = &self.clone_counter {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            lower: self.lower,
            upper: self.upper,
            excluded: self.excluded.clone(),
            clone_counter: self.clone_counter.as_ref().map(Arc::clone),
        }
    }
}
impl PartialEq for ExcludingWord {
    fn eq(&self, other: &Self) -> bool {
        (self.lower, self.upper, &self.excluded) == (other.lower, other.upper, &other.excluded)
    }
}
impl Eq for ExcludingWord {}
impl fmt::Display for ExcludingWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:#x},{:#x}] excluding {:?}",
            self.lower, self.upper, self.excluded
        )
    }
}
impl ExcludingWord {
    pub fn range(lower: U256, upper: U256) -> Self {
        assert!(lower <= upper);
        Self {
            lower,
            upper,
            excluded: BTreeSet::new(),
            clone_counter: None,
        }
    }
    pub fn except(word: U256) -> Self {
        let mut value = Self::top();
        value.excluded.insert(word);
        value
    }
    pub fn covering(left: &Self, right: &Self) -> Self {
        let mut joined = Self::range(left.lower.min(right.lower), left.upper.max(right.upper));
        joined.excluded = left
            .excluded
            .union(&right.excluded)
            .copied()
            .filter(|word| !left.contains(*word) && !right.contains(*word))
            .collect();
        joined
    }
}
impl WordElement for ExcludingWord {
    fn top() -> Self {
        Self::range(U256::ZERO, U256::MAX)
    }
    fn constant(value: U256) -> Self {
        Self::range(value, value)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        if let Some(word) = observation.singleton() {
            return Self::constant(word);
        }
        let (lower, upper) = observation.interval().unsigned_bounds();
        Self::range(lower, upper)
    }
    fn observation(&self) -> WordObservation {
        // The explicit source-export boundary forgets the interior exclusions.
        self.singleton().map_or_else(
            || WordObservation::unsigned_range(self.lower, self.upper).unwrap(),
            WordObservation::constant,
        )
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        let contradiction = || FactError::Contradiction {
            subject: Symbol::THIS,
        };
        if self
            .singleton()
            .is_some_and(|word| !observation.contains(word))
        {
            return Err(contradiction());
        }
        let mut next = self.clone();
        let (lower, upper) = observation.interval().unsigned_bounds();
        next.lower = next.lower.max(lower);
        next.upper = next.upper.min(upper);
        if let Some(candidates) = observation.constants() {
            let possible: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|word| self.contains(*word))
                .collect();
            let Some(first) = possible.first() else {
                return Err(contradiction());
            };
            next.lower = next.lower.max(*first);
            next.upper = next.upper.min(*possible.last().unwrap());
        }
        while next.lower <= next.upper && next.excluded.contains(&next.lower) {
            if next.lower == U256::MAX {
                return Err(contradiction());
            }
            next.lower += U256::ONE;
        }
        while next.lower <= next.upper && next.excluded.contains(&next.upper) {
            if next.upper == U256::ZERO {
                return Err(contradiction());
            }
            next.upper -= U256::ONE;
        }
        if next.lower > next.upper {
            return Err(contradiction());
        }
        Ok(next)
    }
    // Deliberately no finite-set view even for exact constants. Target/index
    // consumers must use semantic singleton, not assume this representation.
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        None
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && !self.excluded.contains(&word)
    }
    fn singleton(&self) -> Option<U256> {
        (self.lower == self.upper && !self.excluded.contains(&self.lower)).then_some(self.lower)
    }
    fn singleton_work(&self) -> usize {
        // The native query compares inline bounds and performs at most one
        // exclusion-tree lookup; it never exports or searches Word256 values.
        self.work_size()
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_empty()
    }
    fn work_size(&self) -> usize {
        self.excluded.len().saturating_mul(8).saturating_add(512)
    }
}
