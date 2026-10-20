use crate::{analysis::EvmMachinePayload, domain::WordElement};
use analysis_kernel::{Capability, Scope, algebra::PublicationPolicy};
use std::{marker::PhantomData, sync::Arc};

/// Input-producing interpreter task family. These are structural locations;
/// concrete assumptions and their lifetimes belong to [`Subject::scope`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PointKind {
    /// Ordinary transaction machine node.
    World,
    /// Relative procedure body node, including resumed summary contributions.
    Body,
}

/// Run-local interpreter point. Relative namespaces distinguish tables whose
/// node indices restart; equal PCs or numerical bounds never identify a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    /// Checked interpreter/table namespace within one kernel epoch.
    pub namespace: u64,
    /// Input-producing interpreter task family.
    pub kind: PointKind,
    /// Structural node index within that namespace and family.
    pub index: usize,
}

/// Fixed scoped analysis assumptions and reaching-execution universe for one
/// production cooperation generation. The native EVM carrier is only one bound
/// on that universe; external providers may prove additional valid guarantees.
///
/// Facts from another subject cannot be silently used as assumptions here. When
/// the global input grows, the interpreter seeds a new scope from its anchor;
/// all narrowing and jobs from the old scope are retired, not merged forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Subject {
    /// Actual interpreter point and relative-table namespace.
    pub point: Point,
    /// Kernel-issued input generation, including epoch and frozen policy.
    pub scope: Scope,
}

/// Immutable native input published by the interpreter for one [`Subject`].
///
/// Queries clone an Arc handle, not the machine or custom numeric elements.
/// Construction/retention of the snapshot is charged by the interpreter before
/// installation. Components charge their own semantic traversal through Charges.
#[derive(Debug)]
pub struct Input<N: WordElement> {
    subject: Subject,
    payload: Arc<EvmMachinePayload<N>>,
}

impl<N: WordElement> Clone for Input<N> {
    fn clone(&self) -> Self {
        Self {
            subject: self.subject,
            payload: Arc::clone(&self.payload),
        }
    }
}

impl<N: WordElement> Input<N> {
    /// Retain an already paid native snapshot. The payload keeps its actual N,
    /// frame relations, transaction Store and per-caller rollback checkpoints.
    pub fn new(subject: Subject, payload: EvmMachinePayload<N>) -> Self {
        Self {
            subject,
            payload: Arc::new(payload),
        }
    }

    /// Exact assumptions under which this snapshot and any feedback are valid.
    pub fn subject(&self) -> Subject {
        self.subject
    }

    /// Borrow the immutable native state without a conversion to a fixed domain.
    pub fn payload(&self) -> &EvmMachinePayload<N> {
        &self.payload
    }

    /// Conservative retained native-state traversal envelope. Fine-grained
    /// numerical operations can use the actual element's more precise quotes.
    pub fn work_size(&self) -> usize {
        self.payload.work_size().saturating_add(8)
    }
}

/// Read-only, single-assignment production inputs, indexed by their own scope.
/// The type marker follows the numeric element's ownership and auto traits.
pub struct Inputs<N: WordElement>(PhantomData<N>);

impl<N: WordElement> Capability for Inputs<N> {
    type Key = ();
    type Value = Input<N>;

    fn key_work(_: &()) -> usize {
        1
    }

    fn value_work(_: &Input<N>) -> usize {
        // Query/retention only copies the paid immutable snapshot handle.
        8
    }
}

/// Attempt to replace an immutable generation's seeded production input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("production input is already initialized; growth requires a fresh generation")]
pub struct InputAlreadyInitialized;

/// Single-assignment input publication policy. All valid inputs are seeded by
/// registered initialization before external jobs are admitted. Same-scope
/// authority is required even for first writes; later writes are refused.
/// This is an immutable source protocol, not an ascending state-join policy.
/// The direction flag enforces registered, same-generation initialization;
/// valid seeded sources never combine with another value.
/// The policy marker follows the numeric element's ownership and auto traits.
pub struct ImmutableInputPolicy<N: WordElement>(PhantomData<N>);

impl<N: WordElement> Default for ImmutableInputPolicy<N> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<N: WordElement> PublicationPolicy for ImmutableInputPolicy<N> {
    type Value = Input<N>;
    type Error = InputAlreadyInitialized;
    const DESCENDING: bool = true;

    fn work(&self, _: &Input<N>, _: &Input<N>) -> usize {
        1
    }

    fn combine(&self, _: &Input<N>, _: &Input<N>) -> Result<Input<N>, Self::Error> {
        Err(InputAlreadyInitialized)
    }

    fn equivalent(&self, left: &Input<N>, right: &Input<N>) -> bool {
        Arc::ptr_eq(&left.payload, &right.payload) && left.subject == right.subject
    }
}
