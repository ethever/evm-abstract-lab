//! Bounded asynchronous task transport, independent of the server's runner.

/// Opaque task identity. Numeric values are decoded by Rust on both endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct JobId(pub u64);
impl<'de> serde::Deserialize<'de> for JobId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(<u64 as serde::Deserialize>::deserialize(
            deserializer,
        )?))
    }
}
impl std::fmt::Display for JobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
choice! {
    /// Observable work phase; counters remain cumulative across RPC refinement.
    pub enum AnalysisPhase {
        /// Waiting for a bounded worker slot.
        Queued,
        /// Validating request fields and limits.
        Validating,
        /// Resolving the chain and exact canonical block.
        Pinning,
        /// Reading accounts and storage from the fixed snapshot.
        Acquiring,
        /// Running abstract execution.
        Analyzing,
        /// Constructing and verifying SSA.
        BuildingSsa,
        /// Producing the typed display report.
        Projecting,
        /// Report is ready.
        Complete,
        /// Execution was cancelled.
        Cancelled,
    }
}
record! {
    /// Cumulative task progress; no fabricated percentage for open worklists.
    pub struct AnalysisProgress {
        /// Current phase.
        pub phase: AnalysisPhase,
        /// RPC refinement round.
        pub round: usize,
        /// Observed account count.
        pub accounts: usize,
        /// Observed storage-slot count.
        pub slots: usize,
        /// RPC requests attempted.
        pub requests: usize,
        /// Machine states created.
        pub states: usize,
        /// Transfers performed.
        pub transfers: usize,
        /// Logical execution work charged.
        pub work: usize,
    }
}
impl Default for AnalysisProgress {
    fn default() -> Self {
        Self {
            phase: AnalysisPhase::Queued,
            round: 0,
            accounts: 0,
            slots: 0,
            requests: 0,
            states: 0,
            transfers: 0,
            work: 0,
        }
    }
}
variant! {
    /// Task lifecycle. Completed reports are fetched separately, never in polls.
    pub enum JobState {
        /// Waiting for a worker.
        Queued,
        /// Work is executing.
        Running,
        /// Immutable result is available from the result endpoint.
        Completed,
        /// Terminal typed failure.
        Failed(crate::ApiError),
        /// Cancellation requested; the worker has not acknowledged cleanup.
        Cancelling,
        /// Terminal cancellation after worker cleanup.
        Cancelled,
    }
}
record! {
    /// Small status reply, independent of report size.
    pub struct JobSnapshot {
        /// Job handle.
        pub id: JobId,
        /// Lifecycle including a terminal error where applicable.
        pub state: JobState,
        /// Last cumulative progress observation.
        pub progress: AnalysisProgress,
    }
}
record! {
    /// Shared status envelope for submit, poll and cancellation.
    pub struct JobReply {
        /// Status or a typed request failure.
        pub result: Result<JobSnapshot, crate::ApiError>,
    }
}
