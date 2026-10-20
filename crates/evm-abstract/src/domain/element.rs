//! Policy-free EVM numeric elements and their conservative query vocabulary.

use super::{
    FiniteConstantSet, NumericValue, WordObservation, congruence::Congruence, facts::FactError,
    interval::Interval, known_bits::KnownBits,
};
use alloy_primitives::U256;
use serde::Serialize;
use std::{collections::BTreeSet, fmt, sync::LazyLock};

static FINITE_TOP: LazyLock<FiniteConstantSet> = LazyLock::new(FiniteConstantSet::top);
static BITS_TOP: LazyLock<KnownBits> = LazyLock::new(KnownBits::top);
static INTERVAL_TOP: LazyLock<Interval> = LazyLock::new(Interval::top);
static CONGRUENCE_TOP: LazyLock<Congruence> = LazyLock::new(Congruence::top);

/// An upper approximation of Word256, without mutable run policy.
///
/// Literals must be exact. Query guarantees may be weaker than the element;
/// unsupported views return Top rather than fabricating a witness. Export to
/// `WordObservation` is an explicit sound projection, not a lossless round-trip
/// promise for arbitrary custom predicates. Executable state retains `Self`.
pub trait WordElement: Clone + fmt::Debug + Eq + fmt::Display + Serialize {
    /// No constraint on the represented word.
    fn top() -> Self;
    /// Exactly this literal, independently of policy/capacity.
    fn constant(value: U256) -> Self;
    /// Import an observation conservatively; unsupported constraints may be lost.
    fn from_observation(observation: &WordObservation) -> Self;
    /// Export guarantees about this same word.
    fn observation(&self) -> WordObservation;
    /// Refine this element by sound guarantees, preserving custom constraints.
    /// A default refusal to gain precision is sound and leaves the element intact.
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        if self
            .singleton()
            .is_some_and(|word| !observation.contains(word))
        {
            return Err(FactError::Contradiction {
                subject: super::facts::Symbol::THIS,
            });
        }
        Ok(self.clone())
    }
    /// Numeric format of one unobserved byte.
    fn unknown_byte() -> Self {
        Self::from_observation(&WordObservation::unknown_byte())
    }
    /// Numeric format of one unobserved address.
    fn unknown_address() -> Self {
        Self::from_observation(&WordObservation::unknown_address())
    }
    /// Ordered unsigned range, conservatively imported.
    fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        WordObservation::unsigned_range(lower, upper).map(|facts| Self::from_observation(&facts))
    }
    /// Complete retained finite candidates, never a partial enumeration.
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.finite_constants().as_values()
    }
    /// Conservative finite guarantee view. Top need not mean whole-element Top.
    fn finite_constants(&self) -> &FiniteConstantSet {
        &FINITE_TOP
    }
    /// Conservative fixed-bit guarantee view.
    fn known_bits(&self) -> &KnownBits {
        &BITS_TOP
    }
    /// Conservative unsigned/signed range guarantee view.
    fn interval(&self) -> &Interval {
        &INTERVAL_TOP
    }
    /// Conservative integer-congruence guarantee view.
    fn congruence(&self) -> &Congruence {
        &CONGRUENCE_TOP
    }
    /// False proves exclusion; true is not a concrete reachability witness.
    fn contains(&self, word: U256) -> bool {
        self.observation().contains(word)
    }
    /// Exact singleton if proved; the answer must agree with all own constraints.
    fn singleton(&self) -> Option<U256> {
        self.observation().singleton()
    }
    /// Zero has not been excluded by sound numeric knowledge.
    fn may_be_zero(&self) -> bool {
        self.contains(U256::ZERO)
    }
    /// Numeric knowledge does not establish exact zero.
    fn may_be_nonzero(&self) -> bool {
        self.singleton() != Some(U256::ZERO)
    }
    /// Whether this element has no numeric constraints. A conservative false is valid.
    fn is_top(&self) -> bool;
    /// Retained/copy/query work bound, including observation construction and
    /// retention. A custom export cannot inspect or allocate more than this quote.
    fn work_size(&self) -> usize;
    /// Cost of copying this numeric payload, quoted without doing the copy.
    /// Deep custom storage must be included; metadata wrappers are separate.
    fn clone_work(&self) -> usize {
        self.work_size()
    }
    /// Cost of singleton discovery and validation, without running that query.
    /// Overrides may use cached shape/cardinality, never unpaid enumeration.
    fn singleton_work(&self) -> usize {
        self.work_size().saturating_mul(256)
    }
    /// Optional scalar normalization at a paid summary boundary. Extra guarantees
    /// of nonsingletons survive; canonicalizing a true singleton is sound.
    fn canonical_for_summary(&self) -> Self {
        self.clone()
    }
    /// Optional representation compaction preserving concretization exactly.
    fn compact_for_retention(&self) -> Self {
        self.clone()
    }
    /// Restore only presentation data proved redundant by this current value.
    fn restore_redundant_finite_from(&self, _previous: &Self) -> Self {
        self.clone()
    }
}

impl WordElement for NumericValue {
    fn top() -> Self {
        Self::top()
    }
    fn constant(value: U256) -> Self {
        Self::constant(value)
    }
    fn unknown_byte() -> Self {
        Self::unknown_byte()
    }
    fn unknown_address() -> Self {
        Self::unknown_address()
    }
    fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        Self::unsigned_range(lower, upper)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        observation.numeric()
    }
    fn observation(&self) -> WordObservation {
        self.into()
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        Ok(self.observation().meet(observation)?.numeric())
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.constants()
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        self.finite_constants()
    }
    fn known_bits(&self) -> &KnownBits {
        self.known_bits()
    }
    fn interval(&self) -> &Interval {
        self.interval()
    }
    fn congruence(&self) -> &Congruence {
        self.congruence()
    }
    fn contains(&self, value: U256) -> bool {
        self.contains(value)
    }
    fn singleton(&self) -> Option<U256> {
        self.singleton()
    }
    fn may_be_zero(&self) -> bool {
        self.may_be_zero()
    }
    fn may_be_nonzero(&self) -> bool {
        self.may_be_nonzero()
    }
    fn is_top(&self) -> bool {
        self.is_top()
    }
    fn work_size(&self) -> usize {
        self.work_size()
    }
    fn clone_work(&self) -> usize {
        // Inline component words and the shared finite-allocation handle.
        12
    }
    fn singleton_work(&self) -> usize {
        let limbs = 4;
        let finite = self.finite_constants().cardinality().map_or(0, |len| {
            // Conservative height/key bound of the existing finite BTreeSet.
            let levels = if len < 11 {
                1
            } else {
                1 + (len / 2 + len % 2).ilog2() as usize / 2
            };
            let first = if len == 1 { 8 * limbs } else { 0 };
            let contains = levels * (len.min(11) * (6 * limbs + 8) + 8 * limbs);
            first + contains
        });
        let modulo = !self.congruence().is_top() && self.congruence().modulus_residue().is_some();
        128 * limbs + finite + usize::from(modulo) * 256 * limbs * limbs
    }
    fn canonical_for_summary(&self) -> Self {
        self.singleton()
            .map_or_else(|| self.clone(), Self::constant)
    }
    fn compact_for_retention(&self) -> Self {
        self.compact_for_retention()
    }
    fn restore_redundant_finite_from(&self, previous: &Self) -> Self {
        self.restore_redundant_finite_from(previous)
    }
}
