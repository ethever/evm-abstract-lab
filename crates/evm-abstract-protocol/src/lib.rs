//! Versioned data exchanged by the native analyzer and the browser workbench.
//!
//! This crate depends only on serde and contains no execution engine, networking
//! or GUI code. IDs refer to native machine states and edges throughout the
//! graph and SSA; byte offsets are never used as state identities. Word values
//! are lossless hexadecimal strings rather than JavaScript floating point numbers.

#[macro_use]
mod codec;
mod analysis;
mod boundaries;
mod errors;
mod input_errors;
mod jobs;
mod query_errors;
mod request;
mod rpc;
mod rpc_causes;
mod ssa;
mod ssa_errors;
mod state;

pub use analysis::*;
pub use boundaries::*;
pub use errors::*;
pub use input_errors::*;
pub use jobs::*;
pub use query_errors::*;
pub use request::*;
pub use rpc::*;
pub use rpc_causes::*;
pub use ssa::*;
pub use ssa_errors::*;
pub use state::*;

/// Same-origin analysis endpoint.
pub const API_PATH: &str = "/api/tasks";
/// Wire schema version, independent of the engine's private JSON formats.
pub const SCHEMA_VERSION: u16 = 2;
/// Maximum UTF-8 JSON request body accepted by the server.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

choice! {
    /// Explicit execution rules used for decoding and analysis.
    pub enum Fork {
        /// Cancun execution rules.
        Cancun,
        /// Prague execution rules.
        Prague,
        /// Osaka execution rules.
        Osaka,
    }
}

choice! {
    /// Stable categories for request and analysis failures.
    pub enum ApiErrorCode {
        /// JSON framing or HTTP syntax failed validation.
        InvalidRequest,
        /// Runtime bytecode could not be decoded.
        InvalidBytecode,
        /// Requested analysis limits are outside server policy.
        InvalidLimits,
        /// Input exceeded a fixed transport or bytecode bound.
        RequestTooLarge,
        /// SSA verification or another internal invariant failed.
        Internal,
        /// The route or asset does not exist.
        NotFound,
        /// This route does not accept the HTTP method.
        MethodNotAllowed,
        /// RPC acquisition failed.
        Rpc,
        /// Task identity is unknown.
        TaskNotFound,
        /// Task has not produced a result.
        TaskNotReady,
        /// Scheduling capacity is exhausted.
        QueueFull,
        /// Analysis was cancelled.
        Cancelled,
        /// Native worker failed.
        WorkerFailed,
        /// Browser or server transport failed.
        Transport,
    }
}

record! {
    /// A typed error envelope; the message supplies human-readable context.
    pub struct ApiError {
        /// Machine-readable error category.
        pub code: ApiErrorCode,
        /// Explanation suitable for display beside the input editor.
        pub message: String,
        /// Authoritative typed failure detail.
        pub details: ErrorDetails,
    }
}

record! {
    /// One response shape for successful analysis and all HTTP failures.
    pub struct AnalyzeReply {
        /// Successful structured report or a typed failure.
        pub result: Result<AnalysisReport, ApiError>,
    }
}
