//! Finite process coordinates, separate from native instruction coordinates.

use super::{ProcessAction, ProcessOwner};
use crate::Fork;
use crate::domain::{EvmValue, WordElement};
use alloy_primitives::{Address, B256};
use serde::Serialize;

/// Actual joined process invocation bounds; anchors never supply these values.
#[derive(Clone, Debug, Serialize)]
pub struct EvmProcessInvocation<N: WordElement> {
    /// Logical ADDRESS, including a symbolic root or independent target.
    pub address: EvmValue<N>,
    /// Primitive caller or inherited DELEGATECALL caller.
    pub caller: EvmValue<N>,
    /// Primitive call value or inherited DELEGATECALL value.
    pub value: EvmValue<N>,
}

/// Executable provenance; an abstraction never changes captured code identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProcessSource {
    /// The original captured runtime and decoder fork.
    Runtime {
        /// Account supplying the captured executable bytes.
        source: Address,
        /// Complete hash of those executable bytes.
        hash: B256,
        /// Decoder and execution rules captured with this source.
        fork: Fork,
    },
    /// Captured constructor bytes, distinct from account runtime code.
    InitCode {
        /// Complete hash of the constructor bytes.
        hash: B256,
        /// Decoder and execution rules captured with this constructor.
        fork: Fork,
    },
    /// Unobserved runtime, including future deployed code in the complement.
    AnyRuntime {
        /// Frozen execution rules for this opaque runtime class.
        fork: Fork,
    },
    /// Arbitrary constructor bytes; no Program or source hash is invented.
    AnyInitCode {
        /// Frozen execution rules for this opaque constructor class.
        fork: Fork,
    },
    /// Actual empty account execution.
    Empty {
        /// Frozen execution rules for empty account code.
        fork: Fork,
    },
    /// Native precompile interpretation under the frozen fork.
    Precompile {
        /// Actual native precompile selected by account resolution.
        address: Address,
        /// Frozen precompile set and execution rules.
        fork: Fork,
    },
    /// A second delegation marker executes an invalid EF instruction.
    InvalidDelegation {
        /// Account supplying the second delegation marker.
        source: Address,
        /// Complete hash of that captured invalid marker.
        hash: B256,
        /// Frozen rules under which the marker faults.
        fork: Fork,
    },
}

/// A process has no instruction cursor or concrete residual storage address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessFrameKey {
    /// Digest of all frozen named owners and captured code versions.
    pub epoch: B256,
    /// Physical state class, independent of logical ADDRESS operands.
    pub owner: ProcessOwner,
    /// Original executable identity and origin.
    pub source: ProcessSource,
    /// Read-only execution propagates through every nested action.
    pub is_static: bool,
}

/// Constructor continuation coordinates; nonce/endowment stay joined inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessCreation {
    /// CREATE and CREATE2 use the same completion contract but distinct actions.
    pub action: ProcessAction,
    /// Execution/storage owner that performed allocation, never code owner.
    pub creator: ProcessOwner,
    /// Complete named-plus-residual destination class.
    pub destination: ProcessOwner,
    /// Full catalogue currency, never its truncated generation descriptor.
    pub epoch: B256,
}

/// Native deployment addresses and process address classes cannot be confused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CreationContinuation {
    /// Ordinary native initcode deployment.
    Native {
        /// Concrete account allocated by the native CREATE/CREATE2 rule.
        destination: Address,
    },
    /// Native Body/Resume process constructor dependency.
    Process(ProcessCreation),
}

impl CreationContinuation {
    /// A concrete destination exists only for the ordinary native adapter.
    pub fn native_address(self) -> Option<Address> {
        match self {
            Self::Native { destination } => Some(destination),
            Self::Process(_) => None,
        }
    }
}
