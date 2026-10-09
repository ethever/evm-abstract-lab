//! CLI policy defaults match the browser's `AnalysisLimits` declaration.
//!
//! Keep each numeric default in one place across raw bytecode, world, RPC and
//! explain arguments. Integration tests compare these effective policies with
//! the protocol defaults; the core library retains its independent defaults.

pub(crate) const MAX_STATES: usize = 100_000;
pub(crate) const MAX_TRANSFERS: usize = 10_000_000;
pub(crate) const CONTEXT_DEPTH: usize = 128;
pub(crate) const MAX_CONSTANTS: usize = 512;
pub(crate) const MAX_WORK: usize = 1_000_000_000_000;
pub(crate) const MAX_CALL_DEPTH: usize = 1025;
pub(crate) const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const REDUCTION_ROUNDS: usize = 16;
pub(crate) const MAX_FACTS: usize = 4096;
pub(crate) const MAX_CONSTRAINTS: usize = 2048;
pub(crate) const MAX_EXPRESSION_NODES: usize = 16_384;
pub(crate) const MAX_EXPRESSION_DEPTH: usize = 256;
pub(crate) const SMT_RLIMIT: u32 = 10_000_000;
pub(crate) const MAX_RPC_ACCOUNTS: usize = 4096;
pub(crate) const MAX_RPC_REQUESTS: usize = 1_000_000;
pub(crate) const MAX_RPC_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const RPC_TIMEOUT_MS: u64 = 120_000;
