//! Shared application policy for CLI, HTTP host and browser requests.
//!
//! These controls intentionally remain independent of core library defaults.

/// Maximum graph states.
pub const MAX_STATES: usize = 100_000;
/// Maximum cumulative transfers.
pub const MAX_TRANSFERS: usize = 10_000_000;
/// Retained internal jump history.
pub const CONTEXT_DEPTH: usize = 128;
/// Constants per abstract value.
pub const MAX_CONSTANTS: usize = 512;
/// Maximum cumulative execution work.
pub const MAX_WORK: usize = 2_000_000_000;
/// Captured source regions retaining optional Native context; zero disables detail.
pub const MAX_DETAIL_REGIONS: usize = 1;
/// Independent allowance shared by local projection, SSA and requested rendering.
pub const DEFAULT_POSTPROCESS_WORK: usize = 2_000_000_000;
/// Maximum simultaneously retained frames.
pub const MAX_CALL_DEPTH: usize = 1025;
/// Maximum modeled byte-array span.
pub const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Fact reduction rounds.
pub const REDUCTION_ROUNDS: usize = 16;
/// Maximum retained scalar facts.
pub const MAX_FACTS: usize = 4096;
/// Maximum relational constraints.
pub const MAX_CONSTRAINTS: usize = 2048;
/// Maximum expression nodes.
pub const MAX_EXPRESSION_NODES: usize = 16_384;
/// Maximum expression depth.
pub const MAX_EXPRESSION_DEPTH: usize = 256;
/// Maximum acquired accounts.
pub const MAX_RPC_ACCOUNTS: usize = 4096;
/// Maximum cumulative RPC requests.
pub const MAX_RPC_REQUESTS: usize = 1_000_000;
/// Maximum accepted bytes per RPC response.
pub const MAX_RPC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// Per-request timeout in milliseconds.
pub const RPC_TIMEOUT_MS: u64 = 120_000;
