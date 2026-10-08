//! Versioned data exchanged by the native analyzer and the browser workbench.
//!
//! This crate depends only on serde and contains no execution engine, networking
//! or GUI code. IDs refer to native machine states and edges throughout the
//! graph and SSA; byte offsets are never used as state identities. Word values
//! are lossless hexadecimal strings rather than JavaScript floating point numbers.

#[macro_use]
mod codec;
mod analysis;
mod ssa;

pub use analysis::*;
pub use ssa::*;

/// Same-origin analysis endpoint.
pub const API_PATH: &str = "/api/analyze";
/// Wire schema version, independent of the engine's private JSON formats.
pub const SCHEMA_VERSION: u16 = 1;
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

record! {
    /// Bounded analysis controls, validated by the native application.
    pub struct AnalysisLimits {
        /// Maximum graph states, between 1 and 4096.
        pub max_states: usize,
        /// Maximum transfers, between 1 and 100000.
        pub max_transfers: usize,
        /// Recent internal jump sources retained, between 0 and 16.
        pub context_depth: usize,
        /// Constants retained per abstract value, between 1 and 32.
        pub max_constants: usize,
    }
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_states: 512,
            max_transfers: 10_000,
            context_depth: 2,
            max_constants: 8,
        }
    }
}

record! {
    /// Analyze ordinary runtime bytecode with unknown environment and state.
    pub struct AnalyzeRequest {
        /// Hexadecimal runtime bytecode; whitespace and a `0x` prefix are accepted.
        pub bytecode: String,
        /// Execution fork, fixed for the whole report.
        pub fork: Fork,
        /// Worklist and precision bounds.
        pub limits: AnalysisLimits,
    }
}

impl Default for AnalyzeRequest {
    fn default() -> Self {
        Self {
            bytecode: "600160020100".into(),
            fork: Fork::Osaka,
            limits: AnalysisLimits::default(),
        }
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
    }
}

record! {
    /// A typed error envelope; the message supplies human-readable context.
    pub struct ApiError {
        /// Machine-readable error category.
        pub code: ApiErrorCode,
        /// Explanation suitable for display beside the input editor.
        pub message: String,
    }
}

record! {
    /// One response shape for successful analysis and all HTTP failures.
    pub struct AnalyzeReply {
        /// Successful structured report or a typed failure.
        pub result: Result<AnalysisReport, ApiError>,
    }
}
