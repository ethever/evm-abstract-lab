//! Explicit, bounded RPC acquisition for a fixed analysis snapshot.
//!
//! Every state request uses [EIP-1898](https://eips.ethereum.org/EIPS/eip-1898)'s exact block hash with
//! `requireCanonical=true`. Unsupported hash selectors and missing state are
//! errors; there is no retry using a block number, moving tag, or empty fact.
//! Chain identity and the selected block are resolved once before acquisition.
//! The caller fully trusts the endpoint; account facts use ordinary state RPC.

mod failure;
mod loader;
pub use failure::{
    CodeFailure, ConfigurationReason, DelegationFailure, FailureCause, HeaderField, HeaderValue,
    ResponseReason, TransportFailure, WorldFailure,
};
mod session;
#[cfg(test)]
mod tests;
mod wire;

pub use session::Session;

use super::{World, WorldError};
use crate::{
    Fork,
    analysis::{control::Control, progress::Observer},
    bytecode::DecodeError,
};
use alloy_primitives::{Address, B256, U256};
use loader::Loader;
use reqwest::Client;
use serde::Serialize;
use std::{collections::BTreeSet, fmt, time::Duration};

/// One explicitly requested account and its observed storage slots.
#[derive(Clone, Debug)]
pub struct AccountRequest {
    /// Account whose code, balance and nonce will be acquired.
    pub address: Address,
    /// Only these slots are fetched; omitted slots remain unknown.
    pub slots: BTreeSet<U256>,
}

/// Block to resolve once before any state acquisition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RpcBlock {
    /// Pin the endpoint's latest block once, then retain its exact hash.
    #[default]
    Latest,
    /// Resolve this height to one exact block hash.
    Number(u64),
    /// Acquire facts at exactly this block hash.
    Hash(B256),
}

/// A caller-selected endpoint and snapshot selector; never constructed by execution.
#[derive(Clone, Debug)]
pub struct RpcInput {
    /// HTTP(S) JSON-RPC endpoint explicitly authorized by the caller.
    pub endpoint: String,
    /// Execution rules selected by the caller, retained in the loaded world.
    pub fork: Fork,
    /// Block resolved once; all state facts then use its exact hash.
    pub block: RpcBlock,
    /// Accounts explicitly chosen before loading.
    pub accounts: Vec<AccountRequest>,
    /// Timeout for each request, including reading its response body.
    pub timeout: Duration,
    /// Maximum JSON response bytes accepted per request.
    pub max_response_bytes: usize,
    /// Maximum total accounts, including initial and on-demand observations.
    pub max_accounts: usize,
    /// Maximum HTTP requests, including identity checks and failed requests.
    pub max_requests: usize,
}

impl RpcInput {
    /// Create bounded acquisition settings; accounts are added explicitly.
    pub fn new(endpoint: impl Into<String>, fork: Fork) -> Self {
        Self {
            endpoint: endpoint.into(),
            fork,
            block: RpcBlock::Latest,
            accounts: Vec::new(),
            timeout: Duration::from_secs(15),
            max_response_bytes: 4 * 1024 * 1024,
            max_accounts: 256,
            max_requests: 16_384,
        }
    }
}

/// Provenance retained on every acquisition failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RpcContext {
    /// Chain identifier discovered from the endpoint, if available.
    pub chain_id: Option<U256>,
    /// Exact snapshot hash, if already selected or resolved.
    pub block_hash: Option<B256>,
    /// Failing method (or `client` for local configuration failures).
    pub method: &'static str,
    /// Account being observed, if this is a state method.
    pub account: Option<Address>,
    /// Slot being observed, if this is a storage request.
    pub slot: Option<U256>,
}

impl fmt::Display for RpcContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.method)?;
        if let Some(chain_id) = self.chain_id {
            write!(formatter, " at chain {chain_id:#x}")?;
        }
        if let Some(block_hash) = self.block_hash {
            write!(formatter, " block {block_hash}")?;
        }
        if let Some(account) = self.account {
            write!(formatter, " account {account}")?;
        }
        if let Some(slot) = self.slot {
            write!(formatter, " slot {slot:#x}")?;
        }
        Ok(())
    }
}

/// Cumulative resource whose acquisition bound was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionLimit {
    /// Number of accounts retained in the fixed snapshot.
    Accounts,
    /// Number of attempted HTTP requests across the session.
    Requests,
}

/// Stable classification retained when a late acquisition failure is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcFailureKind {
    /// The caller cancelled this operation before another observation completed.
    Cancelled,
    /// Invalid local limits or duplicate initial acquisition entries.
    Configuration,
    /// A cumulative account or request bound was reached.
    AcquisitionLimit,
    /// HTTP transport, connection or timeout failure.
    Transport,
    /// Non-success HTTP status.
    Http,
    /// Per-response allocation bound was reached.
    ResponseLimit,
    /// Reading the response body failed.
    Read,
    /// Response JSON could not be decoded.
    Json,
    /// JSON-RPC returned an explicit error.
    Remote,
    /// JSON-RPC result was missing or null.
    MissingResult,
    /// Response envelope or observed facts were inconsistent.
    Response,
    /// Observed chain identity differed from the initially discovered chain.
    ChainMismatch,
    /// Observed block identity differed from the requested block.
    BlockMismatch,
    /// Code could not be decoded under the selected fork.
    Code,
    /// Account observations violated fixed-snapshot consistency.
    World,
}

/// Serializable evidence for acquisition that failed after analysis began.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// Resolved snapshot, failing method and account/slot provenance.
    pub context: RpcContext,
    /// Typed failure category, independent of the explanatory message.
    pub kind: RpcFailureKind,
    /// Structured nested cause with complete native coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<FailureCause>,
    /// Cumulative resource whose bound was reached, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<AcquisitionLimit>,
    /// Reached cumulative bound, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// HTTP status when the transport completed with a non-success response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// JSON-RPC error code, without the endpoint's untrusted message payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_code: Option<i64>,
    /// Line of malformed JSON or a typed response-schema failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_line: Option<usize>,
    /// Column of malformed JSON or a typed response-schema failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_column: Option<usize>,
    /// Public explanation without transport URLs or untrusted remote messages.
    pub message: String,
}

/// RPC failure; callers must not manufacture an observation or convergence.
///
/// Nested failures remain concrete fields on their variants. Inspect those fields
/// directly; the standard error trait does not expose an erased source chain.
#[derive(Debug)]
pub enum RpcError {
    /// Cancellation drops the in-flight request/body future before returning.
    Cancelled {
        /// Fixed request provenance known at the cancellation checkpoint.
        context: Box<RpcContext>,
    },
    /// The private asynchronous network reactor could not be created.
    Runtime {
        /// Local setup provenance.
        context: Box<RpcContext>,
        /// Original reactor setup failure.
        source: std::io::Error,
    },
    /// Invalid request limits or duplicate acquisition entries.
    Configuration {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Which local configuration invariant failed.
        reason: ConfigurationReason,
    },
    /// A cumulative session acquisition bound was reached before a request.
    AcquisitionLimit {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Cumulative resource whose configured limit was reached.
        resource: AcquisitionLimit,
        /// Configured maximum for the exhausted resource.
        limit: usize,
    },
    /// Transport failures, including timeout and connection errors.
    Transport {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Original transport error with endpoint credentials removed.
        source: reqwest::Error,
    },
    /// Non-success HTTP status; no error payload is interpreted as state.
    Http {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Returned HTTP status code.
        status: u16,
    },
    /// Response exceeded the configured allocation bound.
    ResponseLimit {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Maximum accepted response size.
        limit: usize,
    },
    /// Failure reading the bounded response body.
    Read {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Original I/O error, including body timeout.
        source: reqwest::Error,
    },
    /// The body was not a JSON document.
    Json {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Original JSON decoding error.
        source: serde_json::Error,
    },
    /// JSON-RPC explicitly rejected the pinned request.
    Remote {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Remote JSON-RPC error code.
        code: i64,
        /// Untrusted remote explanation, retained for explicit library inspection.
        /// Public display and serialized failure evidence do not copy this text.
        message: String,
    },
    /// No observation was returned; absence is never manufactured from null.
    MissingResult {
        /// Failing method and exact requested snapshot provenance.
        context: Box<RpcContext>,
    },
    /// Envelope or returned fact was missing, malformed or inconsistent.
    Response {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// The failed validation invariant.
        reason: ResponseReason,
        /// Concrete schema decoding error, retained without exposing payload text.
        source: Option<serde_json::Error>,
    },
    /// The endpoint returned a different chain identity.
    ChainMismatch {
        /// Expected chain and fixed snapshot provenance.
        context: Box<RpcContext>,
        /// Chain identifier actually returned.
        observed: U256,
    },
    /// A block lookup did not resolve the exact requested hash.
    BlockMismatch {
        /// Expected block and fixed snapshot provenance.
        context: Box<RpcContext>,
        /// Block hash actually returned.
        observed: B256,
    },
    /// Runtime code cannot be decoded under the selected fork.
    Code {
        /// Account, method and fixed snapshot provenance.
        context: Box<RpcContext>,
        /// Original fork-aware decoder error.
        source: DecodeError,
    },
    /// Returned account facts violate snapshot code/state consistency.
    World {
        /// Account, method and fixed snapshot provenance.
        context: Box<RpcContext>,
        /// Original snapshot validation error.
        source: WorldError,
    },
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled { context } => write!(formatter, "RPC cancelled ({context})"),
            Self::Runtime { context, source } => {
                write!(formatter, "RPC reactor setup failed ({context}): {source}")
            }
            Self::Configuration { context, reason } => {
                write!(formatter, "invalid RPC configuration ({context}): {reason}")
            }
            Self::AcquisitionLimit {
                context,
                resource,
                limit,
            } => write!(
                formatter,
                "RPC acquisition {resource:?} limit {limit} reached ({context})"
            ),
            Self::Transport { context, source } => {
                write!(formatter, "RPC transport failure ({context}): {source}")
            }
            Self::Http { context, status } => {
                write!(formatter, "RPC HTTP status {status} ({context})")
            }
            Self::ResponseLimit { context, limit } => {
                write!(formatter, "RPC response exceeds {limit} bytes ({context})")
            }
            Self::Read { context, source } => {
                write!(formatter, "RPC response read failure ({context}): {source}")
            }
            Self::Json { context, source } => {
                write!(formatter, "RPC JSON failure ({context}): {source}")
            }
            Self::Remote { context, code, .. } => {
                write!(formatter, "RPC error {code} ({context})")
            }
            Self::MissingResult { context } => {
                write!(formatter, "RPC result is missing or null ({context})")
            }
            Self::Response {
                context, reason, ..
            } => {
                write!(formatter, "invalid RPC response ({context}): {reason}")
            }
            Self::ChainMismatch { context, observed } => {
                write!(
                    formatter,
                    "RPC chain mismatch ({context}): returned {observed:#x}"
                )
            }
            Self::BlockMismatch { context, observed } => {
                write!(
                    formatter,
                    "RPC block mismatch ({context}): returned {observed}"
                )
            }
            Self::Code { context, source } => {
                write!(formatter, "RPC code failure ({context}): {source}")
            }
            Self::World { context, source } => {
                write!(formatter, "RPC snapshot failure ({context}): {source}")
            }
        }
    }
}

impl std::error::Error for RpcError {}

impl RpcError {
    /// Fixed provenance available without parsing a human-readable message.
    pub fn context(&self) -> &RpcContext {
        match self {
            Self::Cancelled { context }
            | Self::Runtime { context, .. }
            | Self::Configuration { context, .. }
            | Self::AcquisitionLimit { context, .. }
            | Self::Transport { context, .. }
            | Self::Http { context, .. }
            | Self::ResponseLimit { context, .. }
            | Self::Read { context, .. }
            | Self::Json { context, .. }
            | Self::Remote { context, .. }
            | Self::MissingResult { context }
            | Self::Response { context, .. }
            | Self::ChainMismatch { context, .. }
            | Self::BlockMismatch { context, .. }
            | Self::Code { context, .. }
            | Self::World { context, .. } => context,
        }
    }

    /// Classification available without parsing the displayed explanation.
    pub fn kind(&self) -> RpcFailureKind {
        match self {
            Self::Cancelled { .. } => RpcFailureKind::Cancelled,
            Self::Runtime { .. } => RpcFailureKind::Transport,
            Self::Configuration { .. } => RpcFailureKind::Configuration,
            Self::AcquisitionLimit { .. } => RpcFailureKind::AcquisitionLimit,
            Self::Transport { .. } => RpcFailureKind::Transport,
            Self::Http { .. } => RpcFailureKind::Http,
            Self::ResponseLimit { .. } => RpcFailureKind::ResponseLimit,
            Self::Read { .. } => RpcFailureKind::Read,
            Self::Json { .. } => RpcFailureKind::Json,
            Self::Remote { .. } => RpcFailureKind::Remote,
            Self::MissingResult { .. } => RpcFailureKind::MissingResult,
            Self::Response { .. } => RpcFailureKind::Response,
            Self::ChainMismatch { .. } => RpcFailureKind::ChainMismatch,
            Self::BlockMismatch { .. } => RpcFailureKind::BlockMismatch,
            Self::Code { .. } => RpcFailureKind::Code,
            Self::World { .. } => RpcFailureKind::World,
        }
    }

    /// Preserve typed provenance when an analysis retains a late failure.
    pub fn failure(&self) -> Failure {
        let (resource, limit) = match self {
            Self::AcquisitionLimit {
                resource, limit, ..
            } => (Some(*resource), Some(*limit)),
            _ => (None, None),
        };
        Failure {
            context: self.context().clone(),
            kind: self.kind(),
            cause: match self {
                Self::Configuration { reason, .. } => Some(FailureCause::Configuration(*reason)),
                Self::Response { reason, .. } => Some(FailureCause::Response(reason.clone())),
                Self::Code { source, .. } => Some(FailureCause::Code(source.into())),
                Self::World { source, .. } => Some(FailureCause::World(source.into())),
                Self::ChainMismatch { observed, .. } => {
                    Some(FailureCause::ChainMismatch(*observed))
                }
                Self::BlockMismatch { observed, .. } => {
                    Some(FailureCause::BlockMismatch(*observed))
                }
                Self::Transport { source, .. } | Self::Read { source, .. } => {
                    Some(FailureCause::Transport(source.into()))
                }
                Self::Runtime { source, .. } => Some(FailureCause::Runtime(source.raw_os_error())),
                _ => None,
            },
            resource,
            limit,
            http_status: match self {
                Self::Http { status, .. } => Some(*status),
                _ => None,
            },
            rpc_code: match self {
                Self::Remote { code, .. } => Some(*code),
                _ => None,
            },
            json_line: match self {
                Self::Json { source, .. }
                | Self::Response {
                    source: Some(source),
                    ..
                } => Some(source.line()),
                _ => None,
            },
            json_column: match self {
                Self::Json { source, .. }
                | Self::Response {
                    source: Some(source),
                    ..
                } => Some(source.column()),
                _ => None,
            },
            message: self.to_string(),
        }
    }
}

/// Acquire a complete selected input set, or return an error before analysis.
///
/// An omitted account remains absent from the observation map (unknown); an
/// omitted storage slot remains unknown. This function never follows CALL
/// targets discovered by execution and never performs implicit networking.
pub fn load(input: &RpcInput) -> Result<World, RpcError> {
    Session::load(input).map(Session::into_world)
}

/// Acquire a pinned snapshot with cancellation and typed progress observations.
pub fn load_with_control(input: &RpcInput, control: &Control) -> Result<World, RpcError> {
    Session::load_with_control(input, control).map(Session::into_world)
}

/// Observe acquisition without enabling cancellation.
pub fn load_with_observer(input: &RpcInput, observer: &Observer) -> Result<World, RpcError> {
    load_with_control(input, &Control::with_observer(observer.clone()))
}

#[cfg(test)]
fn configured_loader(input: &RpcInput) -> Result<Loader, RpcError> {
    configured_loader_with_control(input, &Control::default())
}

fn configured_loader_with_control(input: &RpcInput, control: &Control) -> Result<Loader, RpcError> {
    let client_context = context(input, "client", None, None);
    if input.timeout.is_zero()
        || input.max_response_bytes == 0
        || input.max_accounts == 0
        || input.max_requests == 0
    {
        return Err(RpcError::Configuration {
            context: Box::new(client_context),
            reason: ConfigurationReason::Limits,
        });
    }
    let mut addresses = BTreeSet::new();
    if input.accounts.len() > input.max_accounts
        || input
            .accounts
            .iter()
            .any(|request| !addresses.insert(request.address))
    {
        return Err(RpcError::Configuration {
            context: Box::new(client_context),
            reason: ConfigurationReason::InitialAccounts,
        });
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| RpcError::Runtime {
            context: Box::new(client_context.clone()),
            source,
        })?;
    let client = Client::builder()
        .timeout(input.timeout)
        .connect_timeout(input.timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|source| RpcError::Transport {
            context: Box::new(client_context),
            source: source.without_url(),
        })?;
    Ok(Loader {
        input: input.clone(),
        client,
        runtime: Some(runtime),
        control: control.clone(),
        header: None,
        environment: None,
        round: 0,
        acquired_accounts: 0,
        acquired_slots: 0,
        next_id: 0,
        chain_id: None,
        block_hash: None,
        block_number: None,
    })
}

fn context(
    input: &RpcInput,
    method: &'static str,
    account: Option<Address>,
    slot: Option<U256>,
) -> RpcContext {
    RpcContext {
        chain_id: None,
        block_hash: match input.block {
            RpcBlock::Hash(hash) => Some(hash),
            RpcBlock::Latest | RpcBlock::Number(_) => None,
        },
        method,
        account,
        slot,
    }
}

fn invalid(context: &RpcContext, reason: ResponseReason) -> RpcError {
    RpcError::Response {
        context: Box::new(context.clone()),
        reason,
        source: None,
    }
}
