//! Fixed-snapshot acquisition evidence, including unsuccessful observations.

choice! {
    /// Stable RPC failure category.
    pub enum RpcFailureKind {
        /// Cooperative cancellation acknowledged.
        Cancelled,
        /// Invalid acquisition settings.
        Configuration,
        /// Cumulative acquisition cap.
        AcquisitionLimit,
        /// Network transport failure.
        Transport,
        /// Non-success HTTP status.
        Http,
        /// Response byte cap.
        ResponseLimit,
        /// Response body read error.
        Read,
        /// Malformed JSON.
        Json,
        /// JSON-RPC error reply.
        Remote,
        /// Missing or null result.
        MissingResult,
        /// Invalid response facts or envelope.
        Response,
        /// Chain identity changed.
        ChainMismatch,
        /// Block hash changed.
        BlockMismatch,
        /// Unsupported or invalid observed code.
        Code,
        /// Contradictory account observations.
        World,
    }
}
choice! {
    /// Cumulative acquisition resource.
    pub enum AcquisitionResource {
        /// Account count.
        Accounts,
        /// HTTP request count.
        Requests,
    }
}
record! {
    /// Context of one failed RPC operation. Endpoint URLs are intentionally absent.
    pub struct RpcFailure {
        /// Stable category.
        pub kind: RpcFailureKind,
        /// Resolved chain ID, if available.
        pub chain_id: Option<String>,
        /// Pinned block hash, if available.
        pub block_hash: Option<String>,
        /// RPC method or local client validation phase.
        pub method: String,
        /// Affected account, if any.
        pub account: Option<String>,
        /// Affected storage key, if any.
        pub slot: Option<String>,
        /// Resource that was exhausted.
        pub resource: Option<AcquisitionResource>,
        /// Bound reached.
        pub limit: Option<usize>,
        /// HTTP response status.
        pub http_status: Option<u16>,
        /// Remote JSON-RPC error code.
        pub rpc_code: Option<i64>,
        /// Invalid JSON or schema line.
        pub json_line: Option<usize>,
        /// Invalid JSON or schema column.
        pub json_column: Option<usize>,
        /// Exact nested invariant beyond the top-level category.
        pub cause: Option<crate::RpcFailureCause>,
        /// Safe explanatory text supplementing structured evidence.
        pub message: String,
    }
}
record! {
    /// Concrete initial storage observation identity.
    pub struct StorageLocation {
        /// Storage owner.
        pub address: String,
        /// Complete 256-bit slot key.
        pub slot: String,
    }
}
record! {
    /// Cumulative evidence from fixed-hash RPC refinement.
    pub struct AcquisitionReport {
        /// Completed/attempted analysis rounds.
        pub rounds: usize,
        /// Newly discovered accounts acquired after the initial snapshot.
        pub fetched_accounts: Vec<String>,
        /// Accounts whose acquisition failed.
        pub failed_accounts: Vec<String>,
        /// Initial slots acquired during refinement.
        pub fetched_storage: Vec<StorageLocation>,
        /// Failed initial slots.
        pub failed_storage: Vec<StorageLocation>,
        /// Typed failure evidence, including account and slot context.
        pub failures: Vec<RpcFailure>,
        /// Cumulative RPC attempts.
        pub requests: usize,
        /// Cumulative state allocations across refinement rounds.
        pub states_created: usize,
    }
}
