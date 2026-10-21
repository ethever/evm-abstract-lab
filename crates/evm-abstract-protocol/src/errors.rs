//! Structured failure data. Human messages supplement these discriminants.

choice! {
    /// Invalid input categories shared by browser validation and server admission.
    pub enum ValidationErrorKind {
        /// Malformed JSON or unsupported wire representation.
        Json,
        /// Unknown or missing field.
        Field,
        /// Invalid hexadecimal bytes.
        Bytecode,
        /// Invalid Ethereum address.
        Address,
        /// Invalid 256-bit word or hash.
        Word,
        /// Invalid block selector.
        Block,
        /// Unsupported endpoint URL.
        Endpoint,
        /// Execution/environment constraints were inconsistent.
        Environment,
        /// Limit is outside the supported range.
        Limits,
        /// Unsupported input code format.
        CodeFormat,
        /// Protocol version is unsupported.
        Schema,
    }
}
record! {
    /// Specific validation failure; field and value are input coordinates.
    pub struct ValidationFailure {
        /// Failure category.
        pub kind: ValidationErrorKind,
        /// Field path, when known.
        pub field: Option<String>,
        /// Rejected non-secret value, when safe to expose.
        pub value: Option<String>,
    }
}
choice! {
    /// Task admission/lookup errors.
    pub enum TaskErrorKind {
        /// Handle does not exist or expired.
        NotFound,
        /// Result is not ready.
        NotReady,
        /// Bounded scheduling queue is full.
        QueueFull,
        /// Task was cancelled.
        Cancelled,
    }
}
record! {
    /// Task failure with its optional handle.
    pub struct TaskFailure {
        /// Failure category.
        pub kind: TaskErrorKind,
        /// Task involved, if assigned.
        pub id: Option<crate::JobId>,
    }
}
choice! {
    /// Native worker failures without erasing the source category.
    pub enum WorkerErrorKind {
        /// Worker failed to start.
        Spawn,
        /// Worker exited unexpectedly.
        Exit,
        /// Result channel closed unexpectedly.
        Channel,
        /// Worker panicked.
        Panic,
        /// Worker emitted invalid protocol data.
        Protocol,
    }
}
record! {
    /// Native worker failure evidence.
    pub struct WorkerFailure {
        /// Failure category.
        pub kind: WorkerErrorKind,
        /// Operating-system code for a worker thread setup failure, when available.
        pub os_code: Option<i32>,
    }
}
choice! {
    /// Native engine failure; contextual diagnostics remain in ApiError.message.
    pub enum ExecutionFailureKind {
        /// Shared analyzer scope, dependency, identity or resource protocol failed.
        Protocol,
        /// Required immutable input initialization failed.
        Initialization,
        /// Local facts were invalid or required refinement work was refused.
        Refinement,
        /// Required analyzer work or feedback obligations remain unsettled.
        Unfinished,
    }
}
choice! {
    /// HTTP and browser transport failures.
    pub enum TransportErrorKind {
        /// Invalid HTTP framing.
        HttpRequest,
        /// Route does not exist.
        Route,
        /// HTTP method is unsupported.
        Method,
        /// Input or response exceeded a byte bound.
        Size,
        /// Browser network transport failed.
        Network,
        /// Request serialization failed.
        Encode,
        /// Response decoding failed.
        Decode,
        /// HTTP status disagrees with a success envelope.
        HttpStatus,
    }
}
record! {
    /// Transport evidence without opaque JSON.
    pub struct TransportFailure {
        /// Failure category.
        pub kind: TransportErrorKind,
        /// HTTP status where available.
        pub status: Option<u16>,
        /// Observed or permitted byte size where relevant.
        pub bytes: Option<usize>,
    }
}
variant! {
    /// Authoritative error detail; the message is supplementary presentation.
    pub enum ErrorDetails {
        /// Exact snapshot consistency invariant.
        World(crate::WorldFailure),
        /// Exact native bytecode decoding failure.
        Bytecode(crate::BytecodeFailure),
        /// Exact environment invariant.
        Environment(crate::EnvironmentFailure),
        /// Exact native configuration invariant.
        Configuration(crate::ConfigurationFailure),
        /// Typed engine failure, separate from task and worker transport.
        Execution(ExecutionFailureKind),
        /// Typed immutable initial-input session or preparation failure.
        Session(Box<crate::SessionFailure>),
        /// Rejected server resource range.
        Limit(crate::LimitFailure),
        /// Invalid input or configuration.
        Validation(ValidationFailure),
        /// HTTP/browser boundary failure.
        Transport(TransportFailure),
        /// Task admission or cancellation failure.
        Task(TaskFailure),
        /// Worker process/thread or result-channel failure.
        Worker(WorkerFailure),
        /// RPC request or snapshot failure.
        Rpc(Box<crate::RpcFailure>),
        /// A report failed schema, graph-sort or pool-reference validation.
        Report(crate::ReportValidationFailure),
        /// Bounded report projection failed without changing analysis results.
        Export(Box<crate::ExportFailure>),
        /// Snapshot identity computation failed explicitly.
        Fingerprint(FingerprintFailure),
        /// SSA invariant failure.
        Ssa(crate::SsaFailure),
    }
}

choice! {
    /// Native snapshot fingerprint computation boundary.
    pub enum FingerprintFailureKind {
        /// Cancellation during exact fingerprint encoding.
        Cancelled,
        /// A symbolic or numeric value rejected bounded serialization.
        Encoding,
        /// A legacy length prefix cannot represent the exact length.
        LengthOverflow,
    }
}
record! {
    /// Original fingerprint failure classification and encoding coordinates.
    pub struct FingerprintFailure {
        /// Exact failure category; no digest was supplied.
        pub kind: FingerprintFailureKind,
        /// Native serializer line, when available.
        pub line: Option<usize>,
        /// Native serializer column, when available.
        pub column: Option<usize>,
    }
}
