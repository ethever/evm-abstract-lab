//! Recoverable evidence for a byte-range subdomain left before its effect.

use super::MachinePayload;
use crate::domain::AbstractValue;
use serde::Serialize;
use std::sync::Arc;

/// Which mathematical bound partitions this instruction's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RangeKind {
    /// A nonempty access must fit the configured word-rounded memory footprint.
    MemoryPolicy,
    /// A returndata read must stay within the complete returned sequence.
    ReturnData,
}

/// Original operands retained independently of a host-sized representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RangeRequest {
    /// Meaning of the bound and of an excluded input.
    pub kind: RangeKind,
    /// Start of the requested range in EVM word space.
    pub offset: AbstractValue,
    /// Requested byte count, including zero.
    pub size: AbstractValue,
    /// Logical memory policy or actual returndata length.
    pub limit: AbstractValue,
}

/// An uncovered portion of the input, before modifying memory or calling code.
/// The snapshot is taken after operands are popped; `request` retains them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RangeWitness {
    /// The checked access, without enumerating or truncating its length.
    pub request: RangeRequest,
    /// EVM word predicate whose nonzero valuations satisfy the bound.
    pub condition: AbstractValue,
    /// Selected side, or the entire pre-effect input if retaining a guard failed.
    pub nonzero: Option<bool>,
    /// Immutable machine state, including the surviving branch assumptions.
    pub payload: Arc<MachinePayload>,
}
