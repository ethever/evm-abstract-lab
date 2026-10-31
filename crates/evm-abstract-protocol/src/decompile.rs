//! Independent local decompilation tasks and immutable transformation artifacts.
//!
//! Network publication identities are separate from IR stage/entity identities.
//! The shared model contains no recovery runtime or native execution machinery.

pub use evm_decompile_model as decompile_model;
pub use evm_decompile_model::{
    Artifact as DecompileArtifact, Config as DecompileConfig, Limits as DecompileLimits,
    Strategy as DecompileStrategy,
};

/// Same-origin task endpoint for local bytecode decompilation.
pub const DECOMPILE_API_PATH: &str = "/api/decompile/tasks";

record! {
    /// Local runtime bytecode and its explicit recovery/structuring policy.
    pub struct DecompileRequest {
        /// Hexadecimal runtime code; no execution world is required.
        pub bytecode: String,
        /// Instruction rules shared with bytecode inspection.
        pub fork: crate::Fork,
        /// Selected handling of irreducible control flow.
        pub strategy: DecompileStrategy,
        /// Bounds owned by this independent decompilation attempt.
        pub limits: DecompileLimits,
    }
}

impl Default for DecompileRequest {
    fn default() -> Self {
        Self {
            bytecode: "600260030100".into(),
            fork: crate::Fork::Osaka,
            strategy: DecompileStrategy::Dispatcher,
            limits: DecompileLimits::default(),
        }
    }
}

record! {
    /// Small lifecycle snapshot; recovery coverage belongs to the artifact.
    pub struct DecompileJobSnapshot {
        /// Identity from the same process-wide scheduler as analysis tasks.
        pub id: crate::JobId,
        /// Cancellation is acknowledged after worker-owned resources are released.
        pub state: crate::JobState,
        /// Actual recovery and transformation observations.
        pub progress: decompile_model::Progress,
        /// Queue residence, encoded without browser integer loss.
        pub queue_ns: crate::DecimalU64,
        /// Worker duration, frozen after completion.
        pub processing_ns: crate::DecimalU64,
        /// Installed artifact revision, unrelated to its transformation stages.
        pub publication: Option<crate::PublicationVersion>,
    }
}

record! {
    /// Submit, status and cancellation envelope for decompiler tasks.
    pub struct DecompileJobReply {
        /// Typed lifecycle or admission failure.
        pub result: Result<DecompileJobSnapshot, crate::ApiError>,
    }
}

record! {
    /// An immutable decompiler artifact with its exact task publication identity.
    pub struct DecompileJobResult {
        /// Task that owns this result.
        pub id: crate::JobId,
        /// Publication version used to reject stale transport responses.
        pub publication: crate::PublicationVersion,
        /// Baseline, validated stages, explicit limitations and transformation origins.
        pub artifact: DecompileArtifact,
    }
}

record! {
    /// Result envelope, including not-ready, failed and expired tasks.
    pub struct DecompileJobResultReply {
        /// Published artifact or a typed task failure.
        pub result: Result<DecompileJobResult, crate::ApiError>,
    }
}
