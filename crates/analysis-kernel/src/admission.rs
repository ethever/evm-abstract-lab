//! Pure covering preparation followed by one metered publication.
//!
//! Language adapters supply complete state/partition/acceleration semantics. The
//! kernel owns the shared state machine: pay preparation, build a detached bound,
//! reserve installation and all notifications, then commit value and evidence.

use crate::Change;

/// A prepared covering state together with its admission evidence.
pub struct Prepared<V, T> {
    /// Complete candidate that covers both the old state and incoming behavior.
    pub value: V,
    /// Detached coverage history, frozen mode, or other admission evidence.
    pub metadata: T,
    /// Semantic growth, independent of presentation or bookkeeping changes.
    pub change: Change,
    /// Upper bound for retaining/replacing value and metadata, including drops.
    pub install_work: usize,
}

/// Language-specific covering join/acceleration for an already admitted state.
///
/// `prepare` reads immutable live inputs and must never mutate live graph state.
/// Its result covers both inputs and all relevant checkpoints/correlations; an
/// acceleration has that same covering obligation. `Unchanged` asserts semantic
/// equivalence, not just matching identity. Metadata may change independently.
pub trait AdmissionPolicy {
    /// Whole correlated state, not an isolated numeric field.
    type Value;
    /// Admission evidence retained alongside the value.
    type Metadata;
    /// Typed refusal, distinct from budget/cancellation.
    type Error;
    /// Allocation-free conservative quotation for preparation and comparison.
    fn work(
        &self,
        current: &Self::Value,
        incoming: &Self::Value,
        metadata: &Self::Metadata,
    ) -> usize;
    /// Prepare a complete candidate and detached evidence after work is reserved.
    fn prepare(
        &self,
        current: &Self::Value,
        incoming: &Self::Value,
        metadata: &Self::Metadata,
    ) -> Result<Prepared<Self::Value, Self::Metadata>, Self::Error>;
}
