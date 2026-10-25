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

/// Publication sequence within one job, encoded losslessly in browser transports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct PublicationVersion(pub crate::DecimalU64);

impl<'de> serde::Deserialize<'de> for PublicationVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(serde::Deserialize::deserialize(deserializer)?))
    }
}

record! {
    /// Immutable result identity, independent of the job's execution lifecycle.
    pub struct JobPublication {
        /// Monotonic overall publication sequence within this job.
        pub version: PublicationVersion,
        /// Canonical Global model identity, independent of optional rich graphs.
        pub minimum_version: Option<PublicationVersion>,
        /// Rich conservative graph identity; absent until that graph is exported.
        pub global_version: Option<PublicationVersion>,
        /// Version of the Native graph, when one is retained.
        pub native_version: Option<PublicationVersion>,
        /// A closed covering model exists under the report's explicit input qualification.
        pub model_closed: bool,
    }
}

record! {
    /// One version-qualified immutable report; available independently of job completion.
    pub struct JobResult {
        /// Job that produced this publication.
        pub id: JobId,
        /// Exact publication returned, permitting stale-response rejection.
        pub publication: JobPublication,
        /// Source/revision/root-qualified graphs and coverage.
        pub report: crate::AnalysisReport,
    }
}
record! {
    /// Result endpoint response, including typed not-ready and scheduling failures.
    pub struct JobResultReply {
        /// The retained publication or its exact typed failure.
        pub result: Result<JobResult, crate::ApiError>,
    }
}
choice! {
    /// Observable work phase; counters remain cumulative across RPC refinement.
    pub enum AnalysisPhase {
        /// Waiting for a bounded worker slot.
        Queued,
        /// Validating request fields and configuration.
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
        pub transfers: u64,
        /// Logical execution work charged.
        pub work: u64,
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
record! {
    /// Monotonic wall-clock durations in nanoseconds, frozen at task completion.
    /// Processing excludes HTTP report serialization, transmission and browser work.
    #[derive(Default)]
    pub struct AnalysisTimings {
        /// Time from queue admission until the worker actually starts; direct analysis has no queue.
        pub queue_ns: u64,
        /// Worker processing time, including the currently active phase while running.
        pub processing_ns: u64,
        /// Input and configuration validation time.
        pub validating_ns: u64,
        /// Chain identity and canonical block resolution time.
        pub pinning_ns: u64,
        /// Account and storage acquisition time across all refinement rounds.
        pub acquiring_ns: u64,
        /// Abstract execution time across all refinement rounds.
        pub analyzing_ns: u64,
        /// SSA construction and verification time.
        pub building_ssa_ns: u64,
        /// Typed display-report projection time.
        pub projecting_ns: u64,
    }
}
variant! {
    /// Task lifecycle. Completed reports are fetched separately, never in polls.
    pub enum JobState {
        /// Waiting for a worker.
        Queued,
        /// Work is executing.
        Running,
        /// Worker finished successfully; its final immutable result is available.
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
        /// Authoritative cumulative durations, independent of progress-event delivery.
        pub timings: AnalysisTimings,
        /// Latest immutable result, also available while Running or after cancellation.
        pub publication: Option<JobPublication>,
    }
}
record! {
    /// Shared status envelope for submit, poll and cancellation.
    pub struct JobReply {
        /// Status or a typed request failure.
        pub result: Result<JobSnapshot, crate::ApiError>,
    }
}
