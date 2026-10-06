//! Explicit, bounded RPC acquisition for a fixed analysis snapshot.
//!
//! Every state request uses EIP-1898's exact block hash with
//! `requireCanonical=true`. Unsupported hash selectors and missing state are
//! errors; there is no retry using a block number, moving tag, or empty fact.
//! The chosen endpoint is a trusted observation source: code hashes and
//! duplicate state fields are checked, but Merkle proofs are not verified.

mod session;
#[cfg(test)]
mod tests;

pub use session::Session;

use super::{Account, Existence, World, WorldError};
use crate::{Fork, bytecode::DecodeError, domain::Value};
use alloy_primitives::{Address, B256, U256, hex};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{Value as Json, json};
use std::{collections::BTreeSet, fmt, io::Read, time::Duration};

/// One explicitly requested account and its observed storage slots.
#[derive(Clone, Debug)]
pub struct AccountRequest {
    /// Account whose code, balance, nonce and account hash will be acquired.
    pub address: Address,
    /// Only these slots are fetched; omitted slots remain unknown.
    pub slots: BTreeSet<U256>,
}

/// A caller-selected endpoint and fixed snapshot; never constructed by execution.
#[derive(Clone, Debug)]
pub struct RpcInput {
    /// HTTP(S) JSON-RPC endpoint explicitly authorized by the caller.
    pub endpoint: String,
    /// Execution rules selected by the caller, retained in the loaded world.
    pub fork: Fork,
    /// Expected EIP-155 chain identifier.
    pub chain_id: U256,
    /// Expected block hash; all facts are queried at exactly this hash.
    pub block_hash: B256,
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
    pub fn new(endpoint: impl Into<String>, fork: Fork, chain_id: U256, block_hash: B256) -> Self {
        Self {
            endpoint: endpoint.into(),
            fork,
            chain_id,
            block_hash,
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
    /// Chain identifier requested by the caller.
    pub chain_id: U256,
    /// Exact snapshot hash requested by the caller.
    pub block_hash: B256,
    /// Failing method (or `client` for local configuration failures).
    pub method: &'static str,
    /// Account being observed, if this is a state method.
    pub account: Option<Address>,
    /// Slot being observed, if this is a storage request.
    pub slot: Option<U256>,
}

impl fmt::Display for RpcContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at chain {:#x} block {}",
            self.method, self.chain_id, self.block_hash
        )?;
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
    /// Observed chain identity differed from the requested chain.
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
    /// Requested snapshot, failing method and account/slot provenance.
    pub context: RpcContext,
    /// Typed failure category, independent of the explanatory message.
    pub kind: RpcFailureKind,
    /// Cumulative resource whose bound was reached, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<AcquisitionLimit>,
    /// Reached cumulative bound, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Public explanation without transport URLs or untrusted remote messages.
    pub message: String,
}

/// RPC failure; callers must not manufacture an observation or convergence.
///
/// Nested failures remain concrete fields on their variants. Inspect those fields
/// directly; the standard error trait does not expose an erased source chain.
#[derive(Debug)]
pub enum RpcError {
    /// Invalid request limits or duplicate acquisition entries.
    Configuration {
        /// Fixed snapshot and method provenance.
        context: Box<RpcContext>,
        /// Which local configuration invariant failed.
        reason: &'static str,
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
        source: std::io::Error,
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
        reason: &'static str,
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
            Self::Response { context, reason } => {
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
            Self::Configuration { context, .. }
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
            resource,
            limit,
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

fn configured_loader(input: &RpcInput) -> Result<Loader, RpcError> {
    let client_context = context(input, "client", None, None);
    if input.timeout.is_zero()
        || input.max_response_bytes == 0
        || input.max_response_bytes > 64 * 1024 * 1024
        || input.max_accounts == 0
        || input.max_accounts > 4096
        || input.max_requests == 0
    {
        return Err(RpcError::Configuration {
            context: Box::new(client_context),
            reason: "timeout and request limit must be positive, response limit 1..=64 MiB, and account limit 1..=4096",
        });
    }
    let mut addresses = BTreeSet::new();
    if input.accounts.len() > input.max_accounts
        || input
            .accounts
            .iter()
            .any(|request| request.slots.len() > 1024 || !addresses.insert(request.address))
    {
        return Err(RpcError::Configuration {
            context: Box::new(client_context),
            reason: "initial accounts must be unique, fit the account limit, and have at most 1024 slots each",
        });
    }
    let client = Client::builder()
        .timeout(input.timeout)
        .connect_timeout(input.timeout.min(Duration::from_secs(5)))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|source| RpcError::Transport {
            context: Box::new(client_context),
            source: source.without_url(),
        })?;
    Ok(Loader {
        input: input.clone(),
        client,
        next_id: 0,
    })
}

fn context(
    input: &RpcInput,
    method: &'static str,
    account: Option<Address>,
    slot: Option<U256>,
) -> RpcContext {
    RpcContext {
        chain_id: input.chain_id,
        block_hash: input.block_hash,
        method,
        account,
        slot,
    }
}

fn invalid(context: &RpcContext, reason: &'static str) -> RpcError {
    RpcError::Response {
        context: Box::new(context.clone()),
        reason,
    }
}

#[derive(Debug)]
struct Loader {
    input: RpcInput,
    client: Client,
    next_id: usize,
}

impl Loader {
    fn selector(&self) -> Json {
        json!({"blockHash": self.input.block_hash, "requireCanonical": true})
    }

    fn call(&mut self, context: &RpcContext, params: Json) -> Result<Json, RpcError> {
        if self.next_id >= self.input.max_requests {
            return Err(RpcError::AcquisitionLimit {
                context: Box::new(context.clone()),
                resource: AcquisitionLimit::Requests,
                limit: self.input.max_requests,
            });
        }
        self.next_id += 1;
        let id = self.next_id as u64;
        let response = self
            .client
            .post(&self.input.endpoint)
            // A request timeout also reaches reqwest's async body deadline;
            // the blocking client's timeout alone only bounds each read wait.
            .timeout(self.input.timeout)
            .json(&json!({"jsonrpc":"2.0", "id":id, "method":context.method, "params":params}))
            .send()
            .map_err(|source| RpcError::Transport {
                context: Box::new(context.clone()),
                source: source.without_url(),
            })?;
        if !response.status().is_success() {
            return Err(RpcError::Http {
                context: Box::new(context.clone()),
                status: response.status().as_u16(),
            });
        }
        let limit = self.input.max_response_bytes;
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(RpcError::ResponseLimit {
                context: Box::new(context.clone()),
                limit,
            });
        }
        let mut bytes = Vec::new();
        response
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| RpcError::Read {
                context: Box::new(context.clone()),
                source,
            })?;
        if bytes.len() > limit {
            return Err(RpcError::ResponseLimit {
                context: Box::new(context.clone()),
                limit,
            });
        }
        let value: Json = serde_json::from_slice(&bytes).map_err(|source| RpcError::Json {
            context: Box::new(context.clone()),
            source,
        })?;
        let envelope = value
            .as_object()
            .ok_or_else(|| invalid(context, "expected JSON-RPC object"))?;
        if envelope.get("jsonrpc").and_then(Json::as_str) != Some("2.0")
            || envelope.get("id").and_then(Json::as_u64) != Some(id)
        {
            return Err(invalid(context, "JSON-RPC version or response id mismatch"));
        }
        if let Some(error) = envelope.get("error") {
            if envelope.contains_key("result") {
                return Err(invalid(context, "result and error both supplied"));
            }
            let code = error
                .get("code")
                .and_then(Json::as_i64)
                .ok_or_else(|| invalid(context, "missing JSON-RPC error code"))?;
            let message = error
                .get("message")
                .and_then(Json::as_str)
                .ok_or_else(|| invalid(context, "missing JSON-RPC error message"))?;
            return Err(RpcError::Remote {
                context: Box::new(context.clone()),
                code,
                message: message.to_owned(),
            });
        }
        envelope
            .get("result")
            .filter(|result| !result.is_null())
            .cloned()
            .ok_or_else(|| RpcError::MissingResult {
                context: Box::new(context.clone()),
            })
    }

    fn check_chain(&mut self) -> Result<(), RpcError> {
        let context = context(&self.input, "eth_chainId", None, None);
        let value = self.call(&context, json!([]))?;
        let observed = quantity(&value, &context)?;
        if observed != self.input.chain_id {
            return Err(RpcError::ChainMismatch {
                context: Box::new(context),
                observed,
            });
        }
        Ok(())
    }

    fn check_block(&mut self) -> Result<(), RpcError> {
        let context = context(&self.input, "eth_getBlockByHash", None, None);
        let value = self.call(&context, json!([self.input.block_hash, false]))?;
        let observed = hash(&value["hash"], &context)?;
        if observed != self.input.block_hash {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed,
            });
        }
        Ok(())
    }

    fn account(&mut self, world: &mut World, request: &AccountRequest) -> Result<(), RpcError> {
        let address = request.address;
        let code_context = context(&self.input, "eth_getCode", Some(address), None);
        let code = self.call(&code_context, json!([address, self.selector()]))?;
        let bytes = data(&code, &code_context)?;
        let mut account =
            Account::from_hex(&hex::encode(bytes), self.input.fork).map_err(|source| {
                RpcError::Code {
                    context: Box::new(code_context),
                    source,
                }
            })?;
        account.storage_unknown = true;
        let balance_context = context(&self.input, "eth_getBalance", Some(address), None);
        let balance = quantity(
            &self.call(&balance_context, json!([address, self.selector()]))?,
            &balance_context,
        )?;
        account.balance = Value::constant(balance);
        let nonce_context = context(&self.input, "eth_getTransactionCount", Some(address), None);
        let nonce = quantity(
            &self.call(&nonce_context, json!([address, self.selector()]))?,
            &nonce_context,
        )?;
        account.nonce = Value::constant(nonce);
        let proof_context = context(&self.input, "eth_getProof", Some(address), None);
        let keys: Vec<_> = request
            .slots
            .iter()
            .map(|slot| format!("0x{}", hex::encode(slot.to_be_bytes::<32>())))
            .collect();
        let proof = self.call(&proof_context, json!([address, keys, self.selector()]))?;
        let proof_address = proof["address"]
            .as_str()
            .and_then(|text| text.parse::<Address>().ok());
        if proof_address != Some(address)
            || quantity(&proof["balance"], &proof_context)? != balance
            || quantity(&proof["nonce"], &proof_context)? != nonce
        {
            return Err(invalid(
                &proof_context,
                "account address, balance or nonce disagrees with pinned observations",
            ));
        }
        let code_hash = hash(&proof["codeHash"], &proof_context)?;
        hash(&proof["storageHash"], &proof_context)?;
        proof["accountProof"]
            .as_array()
            .ok_or_else(|| invalid(&proof_context, "missing account proof array"))?;
        account.existence = if code_hash == B256::ZERO {
            Existence::Absent
        } else {
            Existence::Present
        };
        if account.existence == Existence::Absent {
            account.storage_unknown = false;
        }
        let storage_proof = proof["storageProof"]
            .as_array()
            .ok_or_else(|| invalid(&proof_context, "missing storage proof array"))?;
        if storage_proof.len() != request.slots.len() {
            return Err(invalid(
                &proof_context,
                "storage proof set differs from requested slots",
            ));
        }
        let mut proof_slots = BTreeSet::new();
        for observation in storage_proof {
            // Some clients preserve 32-byte DATA keys instead of QUANTITY keys.
            let key = word(&observation["key"], &proof_context)?;
            if !request.slots.contains(&key) || !proof_slots.insert(key) {
                return Err(invalid(
                    &proof_context,
                    "unknown or duplicate storage proof key",
                ));
            }
            observation["proof"]
                .as_array()
                .ok_or_else(|| invalid(&proof_context, "missing storage proof nodes"))?;
            let proof_value = quantity(&observation["value"], &proof_context)?;
            let slot_context = context(&self.input, "eth_getStorageAt", Some(address), Some(key));
            let value = self.call(
                &slot_context,
                json!([address, format!("{key:#x}"), self.selector()]),
            )?;
            let bytes = data(&value, &slot_context)?;
            if bytes.len() != 32 {
                return Err(invalid(
                    &slot_context,
                    "storage result must be exactly 32 bytes",
                ));
            }
            let observed = U256::from_be_slice(&bytes);
            if observed != proof_value {
                return Err(invalid(
                    &slot_context,
                    "storage value disagrees with pinned account proof",
                ));
            }
            account.storage.insert(key, Value::constant(observed));
        }
        world
            .insert_with_code_hash(address, account, code_hash)
            .map_err(|source| RpcError::World {
                context: Box::new(proof_context),
                source,
            })?;
        Ok(())
    }
}

fn word(value: &Json, context: &RpcContext) -> Result<U256, RpcError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid(context, "expected hex word"))?;
    let digits = text
        .strip_prefix("0x")
        .filter(|digits| !digits.is_empty() && digits.len() <= 64)
        .ok_or_else(|| invalid(context, "hex word must contain 1..=64 digits after 0x"))?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(context, "invalid hex word digits"));
    }
    U256::from_str_radix(digits, 16).map_err(|_| invalid(context, "invalid 256-bit word"))
}

fn quantity(value: &Json, context: &RpcContext) -> Result<U256, RpcError> {
    let result = word(value, context)?;
    let text = value.as_str().expect("word checked string");
    if text.len() > 3 && text.as_bytes()[2] == b'0' {
        return Err(invalid(context, "noncanonical JSON-RPC quantity"));
    }
    Ok(result)
}

fn data(value: &Json, context: &RpcContext) -> Result<Vec<u8>, RpcError> {
    let digits = value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .ok_or_else(|| invalid(context, "expected 0x-prefixed byte data"))?;
    hex::decode(digits).map_err(|_| invalid(context, "invalid byte data"))
}

fn hash(value: &Json, context: &RpcContext) -> Result<B256, RpcError> {
    let bytes = data(value, context)?;
    B256::try_from(bytes.as_slice()).map_err(|_| invalid(context, "hash must be exactly 32 bytes"))
}
