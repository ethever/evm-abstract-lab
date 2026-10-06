//! Explicit, bounded RPC acquisition for a fixed analysis snapshot.
//!
//! Every state request uses EIP-1898's exact block hash with
//! `requireCanonical=true`. Unsupported hash selectors and missing state are
//! errors; there is no retry using a block number, moving tag, or empty fact.
//! Chain identity and the selected block are resolved once before acquisition.
//! The caller fully trusts the endpoint; account facts use ordinary state RPC.

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
    chain_id: Option<U256>,
    block_hash: Option<B256>,
    block_number: Option<U256>,
}

impl Loader {
    fn context(
        &self,
        method: &'static str,
        account: Option<Address>,
        slot: Option<U256>,
    ) -> RpcContext {
        let mut context = context(&self.input, method, account, slot);
        context.chain_id = self.chain_id;
        context.block_hash = self.block_hash.or(context.block_hash);
        context
    }

    fn selector(&self) -> Json {
        let block_hash = self.block_hash.expect("state acquisition follows pinning");
        json!({"blockHash": block_hash, "requireCanonical": true})
    }

    fn world(&self) -> World {
        World::anchored(
            self.input.fork,
            self.chain_id.expect("world construction follows pinning"),
            self.block_hash.expect("world construction follows pinning"),
            "explicit-rpc",
        )
    }

    fn pin(&mut self) -> Result<(), RpcError> {
        self.check_chain()?;
        let (method, params) = match self.input.block {
            RpcBlock::Latest => ("eth_getBlockByNumber", json!(["latest", false])),
            RpcBlock::Number(number) => (
                "eth_getBlockByNumber",
                json!([format!("{number:#x}"), false]),
            ),
            RpcBlock::Hash(hash) => ("eth_getBlockByHash", json!([hash, false])),
        };
        let context = self.context(method, None, None);
        let value = self.call(&context, params)?;
        let observed = hash(&value["hash"], &context)?;
        if let RpcBlock::Hash(expected) = self.input.block
            && observed != expected
        {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed,
            });
        }
        if let RpcBlock::Number(expected) = self.input.block
            && quantity(&value["number"], &context)? != U256::from(expected)
        {
            return Err(invalid(
                &context,
                "returned block number differs from requested height",
            ));
        }
        self.block_hash = Some(observed);
        // Minimal headers remain accepted for code-only acquisition. Storage
        // refinement requires a valid fixed height before its first request.
        self.block_number = value
            .get("number")
            .and_then(|number| quantity(number, &context).ok());
        Ok(())
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
        let context = self.context("eth_chainId", None, None);
        let value = self.call(&context, json!([]))?;
        let observed = quantity(&value, &context)?;
        if let Some(expected) = self.chain_id {
            if observed != expected {
                return Err(RpcError::ChainMismatch {
                    context: Box::new(context),
                    observed,
                });
            }
        } else {
            self.chain_id = Some(observed);
        }
        Ok(())
    }

    fn check_block(&mut self) -> Result<(), RpcError> {
        let block_hash = self.block_hash.expect("block checks follow pinning");
        let context = self.context("eth_getBlockByHash", None, None);
        let value = self.call(&context, json!([block_hash, false]))?;
        let observed = hash(&value["hash"], &context)?;
        if observed != block_hash {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed,
            });
        }
        Ok(())
    }

    fn storage_block_number(&self) -> Result<U256, RpcError> {
        self.block_number.ok_or_else(|| {
            let method = match self.input.block {
                RpcBlock::Hash(_) => "eth_getBlockByHash",
                RpcBlock::Latest | RpcBlock::Number(_) => "eth_getBlockByNumber",
            };
            invalid(
                &self.context(method, None, None),
                "resolved block header has no valid number for canonical storage checks",
            )
        })
    }

    fn check_storage_block(&mut self, number: U256) -> Result<(), RpcError> {
        let block_hash = self.block_hash.expect("block checks follow pinning");
        let context = self.context("eth_getBlockByNumber", None, None);
        let value = self.call(&context, json!([format!("{number:#x}"), false]))?;
        let observed = hash(&value["hash"], &context)?;
        if observed != block_hash {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed,
            });
        }
        if quantity(&value["number"], &context)? != number {
            return Err(invalid(
                &context,
                "returned canonical block number differs from pinned height",
            ));
        }
        Ok(())
    }

    fn account(&mut self, world: &mut World, request: &AccountRequest) -> Result<(), RpcError> {
        let address = request.address;
        let code_context = self.context("eth_getCode", Some(address), None);
        let code = self.call(&code_context, json!([address, self.selector()]))?;
        let bytes = data(&code, &code_context)?;
        let mut account =
            Account::from_hex(&hex::encode(bytes), self.input.fork).map_err(|source| {
                RpcError::Code {
                    context: Box::new(code_context.clone()),
                    source,
                }
            })?;
        account.storage_unknown = true;
        let balance_context = self.context("eth_getBalance", Some(address), None);
        let balance = quantity(
            &self.call(&balance_context, json!([address, self.selector()]))?,
            &balance_context,
        )?;
        account.balance = Value::constant(balance);
        let nonce_context = self.context("eth_getTransactionCount", Some(address), None);
        let nonce = quantity(
            &self.call(&nonce_context, json!([address, self.selector()]))?,
            &nonce_context,
        )?;
        account.nonce = Value::constant(nonce);
        // Ordinary state RPC cannot distinguish an absent account from an
        // existing account with empty code, zero balance and zero nonce.
        account.existence =
            if account.code != super::Code::Empty || balance != U256::ZERO || nonce != U256::ZERO {
                Existence::Present
            } else {
                Existence::Unknown
            };
        for &key in &request.slots {
            let observed = self.storage(address, key)?;
            if observed != U256::ZERO {
                account.existence = Existence::Present;
            }
            account.storage.insert(key, Value::constant(observed));
        }
        world
            .insert(address, account)
            .map_err(|source| RpcError::World {
                context: Box::new(code_context),
                source,
            })?;
        Ok(())
    }

    fn storage(&mut self, address: Address, key: U256) -> Result<U256, RpcError> {
        let slot_context = self.context("eth_getStorageAt", Some(address), Some(key));
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
        Ok(U256::from_be_slice(&bytes))
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
