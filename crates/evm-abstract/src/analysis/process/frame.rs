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
    /// Supplied root bytes under symbolic To; no physical account is asserted.
    InputProgram {
        /// Digest of the supplied executable bytes.
        hash: B256,
        /// Decoder and execution rules for the supplied bytes.
        fork: Fork,
    },
    /// Supplied delegation marker selecting an unobserved physical executable.
    /// Its known raw marker is not reclassified as unknown runtime bytecode.
    InputDelegation {
        /// Physical pointer in the supplied marker, not a namespace coordinate.
        target: Address,
        /// Digest of the original 23-byte marker.
        hash: B256,
        /// Account-code resolution and execution rules.
        fork: Fork,
    },
    /// The original captured runtime and decoder fork.
    Runtime {
        /// Original observed physical code account.
        source: Address,
        /// Digest of its original executable bytes.
        hash: B256,
        /// Decoder and execution rules for these bytes.
        fork: Fork,
    },
    /// Captured constructor bytes, distinct from account runtime code.
    InitCode {
        /// Digest of the constructor bytes, independent of account runtime.
        hash: B256,
        /// Constructor decoder and execution rules.
        fork: Fork,
    },
    /// Unobserved runtime, including future deployed code in the complement.
    AnyRuntime {
        /// Rules for the complete unresolved account-execution remainder,
        /// including empty/native targets hidden by target abstraction.
        fork: Fork,
    },
    /// Arbitrary constructor bytes; no Program or source hash is invented.
    AnyInitCode {
        /// Rules for every represented constructor byte sequence.
        fork: Fork,
    },
    /// Actual empty account execution.
    Empty {
        /// Rules of the account-code resolution that established emptiness.
        fork: Fork,
    },
    /// Native precompile interpretation under the frozen fork.
    Precompile {
        /// Actual native precompile selected by account-code resolution.
        address: Address,
        /// Fork selecting this native implementation.
        fork: Fork,
    },
    /// A second delegation marker executes an invalid EF instruction.
    InvalidDelegation {
        /// Account supplying the second delegation marker.
        source: Address,
        /// Digest of that original marker's raw bytes.
        hash: B256,
        /// Rules under which its EF byte exceptionally fails.
        fork: Fork,
    },
}

/// A source-qualified MAY location, never evidence of Native PC execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProcessLocation {
    /// Arbitrary unobserved code, with a repeating action/effect envelope.
    #[default]
    Any,
    /// Entry to one decoded basic block under erased private values.
    Block(usize),
    /// Shared union of all legal decoded JUMPDESTs for unknown taken jumps.
    JumpTargets,
    /// Successful fallthrough beyond the captured source's final instruction.
    End,
}

/// Process coordinates retain MAY control and no concrete residual address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessFrameKey {
    /// Digest of all frozen named owners and captured code versions.
    pub epoch: B256,
    /// State namespace or physical class, independent of logical ADDRESS.
    pub owner: ProcessOwner,
    /// Original executable identity and origin.
    pub source: ProcessSource,
    /// Read-only execution propagates through every nested action.
    pub is_static: bool,
    /// Conservative control coordinate within this exact source and epoch.
    pub location: ProcessLocation,
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
        /// Actual deployment destination computed by the native CREATE path.
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
