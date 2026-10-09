//! Serializable nested causes retain native discriminants without error-text parsing.

use crate::{Address, Fork, U256, bytecode::DecodeError, world::WorldError};
use alloy_primitives::B256;
use revm_bytecode::eip7702::Eip7702DecodeError;
use serde::Serialize;

/// A local acquisition configuration invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
pub enum ConfigurationReason {
    /// Acquisition bounds are invalid.
    #[error("timeout, response limit, account limit, and request limit must be positive")]
    Limits,
    /// Initial observations are duplicated or exceed admission limits.
    #[error("initial accounts must be unique and fit the account limit")]
    InitialAccounts,
    /// Storage refinement needs an already observed account.
    #[error("storage acquisition requires an already observed account")]
    StorageAccountMissing,
}

/// Header field whose observation changed at the same purported hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HeaderField {
    /// Parent block hash.
    ParentHash,
    /// Block height.
    Number,
    /// Timestamp.
    Timestamp,
    /// Fee recipient.
    Miner,
    /// PREVRANDAO.
    MixHash,
    /// Block gas limit.
    GasLimit,
    /// Base fee per gas.
    BaseFee,
    /// Excess blob gas.
    ExcessBlobGas,
    /// Blob gas used.
    BlobGasUsed,
}

/// Typed header field value, including an absent optional field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HeaderValue {
    /// Optional header field was not reported.
    Absent,
    /// Full-width EVM quantity.
    Quantity(U256),
    /// Block/randomness hash.
    Hash(B256),
    /// Fee recipient address.
    Address(Address),
}

/// Exact protocol or snapshot-coherence invariant violated by a response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
pub enum ResponseReason {
    /// Returned block height differed from the requested height.
    #[error("returned block number {observed:#x} differs from requested height {expected:#x}")]
    BlockNumber {
        /// Requested height.
        expected: U256,
        /// Returned height.
        observed: U256,
    },
    /// Blob accounting fields must be reported together.
    #[error("blob header requires both excessBlobGas and blobGasUsed")]
    BlobHeaderFields {
        /// Whether excessBlobGas was reported.
        excess_blob_gas: bool,
        /// Whether blobGasUsed was reported.
        blob_gas_used: bool,
    },
    /// The one-block history did not identify exactly the pinned block and next fee.
    #[error("fee history must contain the pinned block and its next-block fee")]
    FeeHistory {
        /// Pinned height.
        expected: U256,
        /// Returned oldestBlock.
        observed: U256,
        /// Number of reported blob base fees.
        fees: usize,
    },
    /// Typed JSON decoding failed; line/column accompany this cause.
    #[error("response violates the typed RPC schema")]
    Schema,
    /// The envelope is not JSON-RPC 2.0.
    #[error("JSON-RPC version mismatch")]
    Version,
    /// The numeric response ID differed from the request ID.
    #[error("JSON-RPC response id mismatch")]
    Id {
        /// Request ID.
        expected: u64,
        /// Response ID, if present.
        observed: Option<u64>,
    },
    /// Result and error are mutually exclusive, even when one is null.
    #[error("result and error both supplied")]
    ResultAndError,
    /// An error member was present but null.
    #[error("JSON-RPC error must be an object")]
    NullError,
    /// The known header fields changed while its hash was claimed unchanged.
    #[error("pinned block header observations changed: {field:?}")]
    HeaderChanged {
        /// Changed field.
        field: HeaderField,
        /// Initial observation.
        expected: HeaderValue,
        /// Later observation.
        observed: HeaderValue,
    },
    /// A session has no pinned height for a canonical check.
    #[error("resolved block header has no valid number for canonical storage checks")]
    MissingBlockNumber,
    /// eth_getStorageAt data has the wrong byte width.
    #[error("storage result must be exactly {expected} bytes, observed {observed}")]
    StorageLength {
        /// Required byte count.
        expected: usize,
        /// Actual byte count.
        observed: usize,
    },
}

/// EIP-7702 decoding failure without erasing its native variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DelegationFailure {
    /// Indicator is not exactly 23 bytes.
    InvalidLength,
    /// Indicator magic bytes are invalid.
    InvalidMagic,
    /// Indicator version is unsupported.
    UnsupportedVersion,
}

/// Exact fork-aware bytecode decoding failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum CodeFailure {
    /// Odd count of hexadecimal digits.
    OddHexLength(usize),
    /// Invalid hexadecimal character.
    InvalidHex {
        /// Digit offset.
        index: usize,
        /// Rejected character.
        character: char,
    },
    /// EOF containers are not supported.
    UnsupportedEof,
    /// Supplied code delegates to another account.
    DelegatedCode(Address),
    /// Invalid delegation indicator.
    InvalidDelegation(DelegationFailure),
}

impl From<&DecodeError> for CodeFailure {
    fn from(error: &DecodeError) -> Self {
        match error {
            DecodeError::OddHexLength(length) => Self::OddHexLength(*length),
            DecodeError::InvalidHex { index, character } => Self::InvalidHex {
                index: *index,
                character: *character,
            },
            DecodeError::UnsupportedEof => Self::UnsupportedEof,
            DecodeError::DelegatedCode { address } => Self::DelegatedCode(*address),
            DecodeError::InvalidDelegation(reason) => Self::InvalidDelegation(match reason {
                Eip7702DecodeError::InvalidLength => DelegationFailure::InvalidLength,
                Eip7702DecodeError::InvalidMagic => DelegationFailure::InvalidMagic,
                Eip7702DecodeError::UnsupportedVersion => DelegationFailure::UnsupportedVersion,
            }),
        }
    }
}

/// Exact fixed-world consistency failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum WorldFailure {
    /// Account runtime and world use different forks.
    MixedFork {
        /// Snapshot rules.
        world: Fork,
        /// Account runtime rules.
        account: Fork,
    },
    /// Delegation is unsupported under the selected fork.
    UnsupportedDelegation(Fork),
    /// An existing observation conflicts.
    Conflict(Address),
    /// Code hash and bytes disagree.
    CodeHash {
        /// Account.
        address: Address,
        /// Declared hash.
        expected: B256,
        /// Actual hash.
        observed: B256,
    },
    /// A declared code hash has no observed bytes.
    UnknownCodeHash(Address),
    /// Confirmed absence conflicts with nonzero/unknown state.
    InvalidAbsence(Address),
}

impl From<&WorldError> for WorldFailure {
    fn from(error: &WorldError) -> Self {
        match error {
            WorldError::MixedFork { world, account } => Self::MixedFork {
                world: *world,
                account: *account,
            },
            WorldError::UnsupportedDelegation(fork) => Self::UnsupportedDelegation(*fork),
            WorldError::Conflict { address } => Self::Conflict(*address),
            WorldError::CodeHash {
                address,
                expected,
                observed,
            } => Self::CodeHash {
                address: *address,
                expected: *expected,
                observed: *observed,
            },
            WorldError::UnknownCodeHash(address) => Self::UnknownCodeHash(*address),
            WorldError::InvalidAbsence(address) => Self::InvalidAbsence(*address),
        }
    }
}

/// Reqwest's public classifications and the upstream diagnostic after URL removal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransportFailure {
    /// Deadline expired.
    pub timeout: bool,
    /// Connection establishment failed.
    pub connect: bool,
    /// Request builder failed.
    pub builder: bool,
    /// Request processing failed.
    pub request: bool,
    /// Body transfer failed.
    pub body: bool,
    /// Response decoding failed.
    pub decode: bool,
    /// Redirect handling failed.
    pub redirect: bool,
    /// Upstream cause retained as opaque diagnostic text. Every production
    /// request/build/read error removes its URL before reaching this projection.
    pub native_diagnostic: String,
}
impl From<&reqwest::Error> for TransportFailure {
    fn from(error: &reqwest::Error) -> Self {
        Self {
            timeout: error.is_timeout(),
            connect: error.is_connect(),
            builder: error.is_builder(),
            request: error.is_request(),
            body: error.is_body(),
            decode: error.is_decode(),
            redirect: error.is_redirect(),
            native_diagnostic: format!("{error:?}"),
        }
    }
}

/// Nested cause when the top-level category and standard HTTP/JSON fields do
/// not fully describe a failure. No endpoint URL or remote message is retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FailureCause {
    /// Local configuration invariant.
    Configuration(ConfigurationReason),
    /// Concrete response/snapshot invariant.
    Response(ResponseReason),
    /// Code parsing invariant.
    Code(CodeFailure),
    /// Snapshot consistency invariant.
    World(WorldFailure),
    /// Chain ID returned after pinning; expected ID is in the context.
    ChainMismatch(U256),
    /// Block hash returned after pinning; expected hash is in the context.
    BlockMismatch(B256),
    /// Concrete transport flags.
    Transport(TransportFailure),
    /// Operating-system error code while creating the HTTP reactor.
    Runtime(Option<i32>),
}
