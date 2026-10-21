//! Recoverable evidence for a byte-range subdomain left before its effect.

use crate::domain::WordElement;

use super::EvmMachinePayload;
use crate::domain::EvmValue;
use alloy_primitives::U256;
use serde::Serialize;
use std::sync::Arc;

/// A byte-array representation rejected a concrete or unbound range. Logical
/// memory-policy partitions remain Memory with their guarded RangeWitness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ByteRangeBoundary {
    /// No finite size or trusted symbolic size term was supplied.
    UnknownSize,
    /// A concrete destination is larger than a host index.
    OffsetTooLarge(U256),
    /// Host range arithmetic overflowed.
    Overflow {
        /// Concrete byte destination.
        offset: usize,
        /// Concrete requested byte count.
        size: usize,
    },
    /// A concrete logical or materialization range exceeds its declared cap.
    TooLarge {
        /// Requested byte count or endpoint.
        requested: U256,
        /// Declared byte limit.
        limit: usize,
    },
    /// Input binding was requested after its original source was established.
    InputAlreadyBound,
}

/// Which mathematical bound partitions this instruction's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RangeKind {
    /// A nonempty access must fit the configured word-rounded memory footprint.
    MemoryPolicy,
    /// A nonempty access must have a representable, word-rounded U256 end.
    /// Host materialization limits do not exclude its EVM-success domain.
    LogicalMemory,
    /// A returndata read must stay within the complete returned sequence.
    ReturnData,
}

/// Original operands retained independently of a host-sized representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRangeRequest<N: WordElement> {
    /// Meaning of the bound and of an excluded input.
    pub kind: RangeKind,
    /// Start of the requested range in EVM word space.
    pub offset: EvmValue<N>,
    /// Requested byte count, including zero.
    pub size: EvmValue<N>,
    /// Historical memory policy, representable logical end, or actual returndata length.
    pub limit: EvmValue<N>,
}

/// An uncovered portion of the input, before modifying memory or calling code.
/// The snapshot is taken after operands are popped; `request` retains them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRangeWitness<N: WordElement> {
    /// The checked access, without enumerating or truncating its length.
    pub request: EvmRangeRequest<N>,
    /// EVM word predicate whose nonzero valuations satisfy the bound.
    pub condition: EvmValue<N>,
    /// Selected side, or the entire pre-effect input if retaining a guard failed.
    pub nonzero: Option<bool>,
    /// Immutable machine state, including the surviving branch assumptions.
    pub payload: Arc<EvmMachinePayload<N>>,
}

// Local regression fixtures use explicit built-in carriers.
#[cfg(test)]
pub(in crate::analysis) type RangeRequest = EvmRangeRequest<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type RangeWitness = EvmRangeWitness<crate::domain::NumericValue>;
