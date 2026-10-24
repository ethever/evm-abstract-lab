//! Policy-free EVM numeric elements and their conservative query vocabulary.

use super::{
    FiniteConstantSet, NumericValue, WordObservation, congruence::Congruence, facts::FactError,
    interval::Interval, known_bits::KnownBits,
};
use alloy_primitives::U256;
use serde::Serialize;
use std::{collections::BTreeSet, fmt, sync::OnceLock};

// Keep stable shared views without storing LazyLock's default function pointer.
static FINITE_TOP: OnceLock<FiniteConstantSet> = OnceLock::new();
static BITS_TOP: OnceLock<KnownBits> = OnceLock::new();
static INTERVAL_TOP: OnceLock<Interval> = OnceLock::new();
static CONGRUENCE_TOP: OnceLock<Congruence> = OnceLock::new();

/// An upper approximation of Word256, without mutable run policy.
///
/// Literals must be exact. Query guarantees may be weaker than the element;
/// unsupported views return Top rather than fabricating a witness. Export to
/// `WordObservation` is an explicit sound projection, not a lossless round-trip
/// promise for arbitrary custom predicates. Executable state retains `Self`.
///
/// Pure methods do no accounting. A metered caller either reserves the named
/// query/copy quote or uses a complete enclosing operation quote that explicitly
/// owns that work. Retaining or copying a value never prepays later queries.
pub trait WordElement: Clone + fmt::Debug + Eq + fmt::Display + Serialize {
    /// No constraint on the represented word.
    fn top() -> Self;
    /// Exactly this literal, independently of policy/capacity.
    fn constant(value: U256) -> Self;
    /// Import an observation conservatively; unsupported constraints may be lost.
    fn from_observation(observation: &WordObservation) -> Self;
    /// Export guarantees about this same word.
    fn observation(&self) -> WordObservation;
    /// Whether an immutable observation binding also needs this actual element.
    /// False promises that the observation expresses the complete scalar set;
    /// arbitrary custom predicates conservatively retain their executable N.
    /// This capability is constant work: no export, query, traversal or clone.
    fn requires_retained_binding(&self) -> bool {
        true
    }
    /// Refine this element by sound guarantees, preserving custom constraints.
    /// A default refusal to gain precision is sound and leaves the element intact.
    /// The caller funds the entire operation, including any internal queries,
    /// through `constrain_work` or a complete algebra refinement envelope.
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
    /// Complete standalone constraint-import quote, including internal queries
    /// and the returned copy. Cached sizes only; overrides of `constrain` must
    /// override this bound if their implementation needs more work.
    fn constrain_work(&self, observation: &WordObservation) -> usize {
        self.singleton_work()
            .saturating_add(self.clone_work())
            .saturating_add(observation.work_size().saturating_mul(256))
    }
    /// Materialize an immutable binding when this executable element belongs to
    /// the SAME binding expression. Preserve custom/path-refined guarantees and
    /// optionally import the observation. Refusal retains this element; this
    /// boundary never reports a contradiction or replaces `constrain`'s law.
    /// Without an executable element for that expression, use `from_observation`.
    fn import_binding(&self, observation: &WordObservation) -> Self {
        self.constrain(observation).unwrap_or_else(|_| self.clone())
    }
    /// Additional work beyond the reader's already-funded fixed raw-binding
    /// materialization and retention. Pay BEFORE `import_binding`. Quotation
    /// only inspects cached sizes, covering attempted constraint work and a
    /// fallback copy; override if a custom implementation needs a larger bound.
    fn import_binding_work(&self, observation: &WordObservation) -> usize {
        self.constrain_work(observation)
            .saturating_add(self.clone_work())
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
        FINITE_TOP.get_or_init(FiniteConstantSet::top)
    }
    /// Conservative fixed-bit guarantee view.
    fn known_bits(&self) -> &KnownBits {
        BITS_TOP.get_or_init(KnownBits::top)
    }
    /// Conservative unsigned/signed range guarantee view.
    fn interval(&self) -> &Interval {
        INTERVAL_TOP.get_or_init(Interval::top)
    }
    /// Conservative integer-congruence guarantee view.
    fn congruence(&self) -> &Congruence {
        CONGRUENCE_TOP.get_or_init(Congruence::top)
    }
    /// False proves exclusion; true is not a concrete reachability witness.
    fn contains(&self, word: U256) -> bool {
        self.observation().contains(word)
    }
    /// Exact singleton if proved; the answer must agree with all own constraints.
    /// Pure/caller-funded. An independent metered query reserves `singleton_work`
    /// first; a paid operation owns only the queries inside that operation.
    fn singleton(&self) -> Option<U256> {
        self.observation().singleton()
    }
    /// Pay for one independent query, then return its reusable answer. The payer
    /// must reject `usize::MAX` (unknown/overflowed work) and cancellation before
    /// returning success. Refusal never executes the query. The caller must also
    /// check cancellation AFTER this method returns, before using its answer.
    /// A new or changed element requires a new payment; callers may reuse this
    /// immutable answer. Overrides must preserve the payment-before-query order.
    fn singleton_with_payment<E>(
        &self,
        pay: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<Option<U256>, E> {
        pay(self.singleton_work())?;
        Ok(self.singleton())
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
    /// Retention/structural work bound, also covering one observation export,
    /// membership test or enabled native serialization. It is not a prepaid
    /// query allowance: singleton discovery has its own `singleton_work` quote,
    /// and copying has `clone_work`. Obtaining sizes performs none of that work.
    fn work_size(&self) -> usize;
    /// Cost of copying this numeric payload, quoted without doing the copy.
    /// Deep custom storage must be included; metadata wrappers are separate.
    /// Payment authorizes the copy alone, not queries on either copy.
    fn clone_work(&self) -> usize {
        self.work_size()
    }
    /// Cost of singleton discovery and validation, without running that query.
    /// Overrides may use cached shape/cardinality, never unpaid enumeration.
    /// Covers the complete query, including any observation export/membership
    /// checks it uses. `usize::MAX` refuses standalone metered execution.
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
    fn requires_retained_binding(&self) -> bool {
        // WordObservation retains every component of the default conjunction.
        false
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        Ok(self.observation().meet(observation)?.numeric())
    }
    fn constrain_work(&self, observation: &WordObservation) -> usize {
        // Default conjunction meet/export, including finite input traversal.
        self.work_size()
            .saturating_add(observation.work_size())
            .saturating_mul(256)
    }
    fn import_binding(&self, observation: &WordObservation) -> Self {
        // This explicit binding boundary restores the authoritative default raw
        // representation. General refinement above still preserves old facts.
        observation.numeric()
    }
    fn import_binding_work(&self, _observation: &WordObservation) -> usize {
        // Only the existing fixed inline/shared-handle materialization is used;
        // no meet, normalization, candidate scan or additional copying occurs.
        0
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
        if self.canonical_literal().is_some() {
            // Quote and query each inspect only the complete fixed-width
            // literal certificate; no tree membership or division follows.
            return 64 * limbs;
        }
        // Without a candidate, singleton() returns before finite membership or
        // modular division. These are the same cached tags and inline checks
        // as that selection chain, not a query through a public observation.
        if self.finite_constants().cardinality() != Some(1)
            && self.known_bits().singleton().is_none()
            && self.interval().singleton().is_none()
            && self.congruence().singleton().is_none()
        {
            return 32 * limbs;
        }
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

#[cfg(test)]
mod tests;
