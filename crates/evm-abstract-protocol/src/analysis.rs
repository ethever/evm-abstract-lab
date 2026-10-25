choice! {
    /// Input assumptions recorded with the result.
    pub enum AnalysisScope {
        /// One runtime program with explicit or unknown transaction inputs.
        SingleProgram,
        /// Pinned trusted RPC world and explicit root execution.
        RpcWorld,
    }
}
choice! {
    /// Worklist completion, not a proof of program safety or exactness.
    pub enum AnalysisStatus {
        /// The configured abstract model reached closure.
        Converged,
        /// Explicit frontiers remain unexpanded.
        Incomplete,
    }
}
record! {
    /// One decoded opcode, independently of execution reachability.
    pub struct DisasmInstruction {
        /// Byte offset of the opcode.
        pub pc: usize,
        /// Numeric opcode byte.
        pub opcode: u8,
        /// Upstream opcode mnemonic.
        pub name: String,
        /// Lossless hexadecimal PUSH value, including right zero padding.
        pub immediate: Option<String>,
        /// Declared encoded width, including truncated PUSH padding.
        pub size: usize,
        /// Whether the opcode exists under the selected fork.
        pub valid: bool,
        /// Required input count, including DUP/SWAP depth requirements.
        pub stack_inputs: usize,
        /// Stack output count.
        pub stack_outputs: usize,
    }
}
record! {
    /// A syntactic basic block of the submitted root program.
    pub struct DisasmBlock {
        /// Program-local block index.
        pub id: usize,
        /// First opcode byte offset.
        pub start_pc: usize,
        /// All decoded instructions, including unexecuted instructions.
        pub instructions: Vec<DisasmInstruction>,
    }
}
record! {
    /// A native machine state; contexts and suspended frames distinguish nodes.
    pub struct CfgBlock {
        /// Explicit process identity; native-only anchor fields are absent when present.
        pub process: Option<crate::ProcessFrameKey>,
        /// Native state ID shared with SSA, edges, diagnostics and frontiers.
        pub id: usize,
        /// Program-local syntactic block index for this active frame.
        pub basic_block: Option<usize>,
        /// First instruction offset, absent for a no-code frame.
        pub start_pc: Option<usize>,
        /// Recent internal jump source offsets.
        pub context: Vec<usize>,
        /// Number of active and suspended frames.
        pub frame_depth: usize,
        /// Executable account identity, distinct from the storage account.
        pub code_address: Option<String>,
        /// Active persistent/transient storage owner.
        pub storage_address: Option<String>,
        /// Active source program in the report directory.
        pub program: Option<usize>,
        /// Active stack on entry, bottom to top; each entry is an abstract value summary.
        pub entry_stack: Vec<String>,
        /// Last observed active exit stack, bottom to top.
        pub exit_stack: Vec<String>,
        /// Offsets actually visited by the latest transfer.
        pub executed_pcs: Vec<usize>,
    }
}
impl CfgBlock {
    /// Actual owner class or recorded native storage address, without a representative.
    pub fn storage_label(&self) -> &str {
        match self.process.as_ref().map(|key| &key.owner) {
            Some(crate::ProcessOwner::RootNamespace) => "Root input owner",
            Some(crate::ProcessOwner::Named(address)) => address,
            Some(crate::ProcessOwner::Residual) => "Residual owner class",
            None => self.storage_address.as_deref().unwrap_or("Not recorded"),
        }
    }
    /// Actual executable source, without exposing private process anchors.
    pub fn code_label(&self) -> &str {
        match self.process.as_ref().map(|key| &key.source) {
            Some(crate::ProcessSource::InputProgram(_)) => "Supplied input program",
            Some(crate::ProcessSource::InputDelegation(source)) => &source.target,
            Some(
                crate::ProcessSource::Runtime(source)
                | crate::ProcessSource::InvalidDelegation(source),
            ) => &source.source,
            Some(crate::ProcessSource::InitCode(_) | crate::ProcessSource::AnyInitCode(_)) => {
                "Constructor code"
            }
            Some(crate::ProcessSource::AnyRuntime(_)) => "Unobserved runtime",
            Some(crate::ProcessSource::Empty(_)) => "Empty execution",
            Some(crate::ProcessSource::Precompile(source)) => &source.address,
            None => self.code_address.as_deref().unwrap_or("Not recorded"),
        }
    }
}
variant! {
    /// Typed control-transfer category, including native call dispatch.
    #[derive(Copy)]
    pub enum EdgeKind {
        /// Local primitive process recurrence, without an executed instruction claim.
        Process(crate::ProcessAction),
        /// Adjacent block entry.
        Fallthrough,
        /// Unconditional internal jump.
        Jump,
        /// Conditional jump taken.
        BranchTrue,
        /// Conditional jump not taken.
        BranchFalse,
        /// Caller suspension and child entry.
        Call,
        /// Successful child completion.
        Return,
        /// Call rejection or exceptional child completion.
        Failure,
        /// Child revert and caller resumption.
        Revert,
        /// Successful caller resumption established by an abstract summary.
        SummaryReturn,
        /// Reverting caller resumption established by an abstract summary.
        SummaryRevert,
        /// Exceptional caller resumption established by an abstract summary.
        SummaryFailure,
    }
}
record! {
    /// An observed edge; a partial SSA may deliberately defer its arguments.
    pub struct CfgEdge {
        /// Original native edge index.
        pub id: usize,
        /// Native source state ID.
        pub from: usize,
        /// Native destination state ID.
        pub to: usize,
        /// Control-transfer category.
        pub kind: EdgeKind,
    }
}
choice! {
    /// Diagnostic category; these do not by themselves imply incompleteness.
    pub enum DiagnosticKind {
        /// Unknown jump conservatively expands legal destinations.
        UnknownJump,
        /// Illegal internal jump destination.
        InvalidJump,
        /// Insufficient stack operands.
        StackUnderflow,
        /// Stack exceeds 1024 slots.
        StackOverflow,
        /// Opcode invalid under the selected fork.
        InvalidOpcode,
        /// Conservative memory, environment or state summary.
        OpaqueResult,
        /// Optional value query failed; conservative complete propagation continues.
        RelationalPrecisionLost,
        /// Precision exchange reached its configured bound.
        FactExchangeLimited,
    }
}
variant! {
    /// Why an ordinary instruction is invalid under the selected execution rules.
    pub enum InvalidOpcodeReason {
        /// The explicit INVALID instruction exceptionally halts execution.
        ExplicitInvalid,
        /// No ordinary bytecode instruction is defined for this byte.
        UndefinedOpcode,
        /// The instruction has not been enabled in the recorded fork.
        UnavailableInFork(crate::Fork),
    }
}
record! {
    /// An invalid instruction actually attempted by an abstract execution path.
    pub struct InvalidInstruction {
        /// Original byte from the state's active executable program.
        pub opcode: u8,
        /// Defined instruction name, absent for undefined opcode bytes.
        pub mnemonic: Option<String>,
        /// Validity reason from the same rules used by execution.
        pub reason: InvalidOpcodeReason,
    }
}
record! {
    /// Exceptional delegation entry, which has no ordinary decoded instruction.
    pub struct InvalidDelegationEntry {
        /// First captured code byte, when it can be obtained from the active account.
        pub opcode: Option<u8>,
        /// Why delegation resolution rejected this entry.
        pub detail: String,
        /// Why a code byte is unavailable, present whenever opcode is absent.
        pub byte_unavailable: Option<String>,
    }
}
variant! {
    /// Invalid-opcode evidence distinguishes real instructions from delegation entry.
    pub enum InvalidOpcodeDiagnostic {
        /// Real instruction in the state's active captured program.
        Instruction(InvalidInstruction),
        /// Invalid delegation resolution at a synthetic entry location.
        DelegationEntry(InvalidDelegationEntry),
    }
}
record! {
    /// Diagnostic with a stable graph location.
    pub struct Diagnostic {
        /// Native state ID.
        pub state: usize,
        /// Opcode offset.
        pub pc: usize,
        /// Typed diagnostic category.
        pub kind: DiagnosticKind,
        /// Typed precision exchange reason, when this is a fact-exchange diagnostic.
        pub reduction: Option<crate::ReductionReason>,
        /// Exact optional relational projection reason, independent of a frontier.
        pub precision: Option<crate::QueryBoundary>,
        /// Original opcode and typed cause, when the category is InvalidOpcode.
        pub invalid_opcode: Option<InvalidOpcodeDiagnostic>,
        /// Detailed explanation, including engine-specific precision limits.
        pub detail: String,
    }
}
choice! {
    /// Stable incomplete-coverage categories; detailed facts remain in the message.
    pub enum FrontierKind {
        /// State creation cap.
        States,
        /// Transfer cap.
        Transfers,
        /// Cumulative execution work cap.
        Work,
        /// External frame depth cap.
        CallDepth,
        /// Memory modeling cap.
        Memory,
        /// Symbolic representation or local path reasoning bound.
        Relations,
        /// Call destination is not covered by supplied code.
        UnknownTarget,
        /// Known account has no observed code.
        MissingCode,
        /// Persistent slot has no observation.
        MissingStorage,
        /// RPC acquisition failed.
        RpcAcquisition,
        /// Direct precompile entry is outside this model.
        Precompile,
        /// Precompile input could not be modeled.
        PrecompileInput,
        /// Account creation boundary.
        Creation,
        /// Unsupported semantics.
        UnsupportedOpcode,
        /// A byte-range representation rejected the original operands.
        ByteRange,
        /// Complete state alternative could not be joined.
        StateJoin,
        /// Other explicit model boundary.
        Model,
    }
}
record! {
    /// An unexpanded path; its absence from the graph never proves infeasibility.
    pub struct Frontier {
        /// Source native state, when one exists.
        pub from: Option<usize>,
        /// Instruction offset where expansion stopped.
        pub pc: Option<usize>,
        /// Stable boundary category.
        pub kind: FrontierKind,
        /// Authoritative boundary facts; the message is supplemental.
        pub reason: crate::FrontierDetails,
        /// Full engine boundary detail, including account/slot when available.
        pub detail: String,
        /// Conditional range boundary before applying effects.
        pub range: Option<crate::RangeWitness>,
        /// Index in pending_machines for an unmerged complete incoming state.
        pub payload: Option<usize>,
    }
}
record! {
    /// One graph namespace with independent machine, SSA and pool identities.
    pub struct AnalysisGraph {
        /// Explicit input/model scope.
        pub scope: AnalysisScope,
        /// Execution rules fixed during decoding.
        pub fork: crate::Fork,
        /// Resolved root executable source size in bytes.
        pub byte_len: usize,
        /// Model closure, independent of SSA display selection.
        pub status: AnalysisStatus,
        /// Executed transfers, including revisits.
        pub transfers: u64,
        /// Reachable native machine-state nodes.
        pub cfg: Vec<CfgBlock>,
        /// Observed native graph edges.
        pub edges: Vec<CfgEdge>,
        /// Verified names and explicit coverage for the recorded graph.
        pub ssa: crate::SsaReport,
        /// Precision and exceptional-halt diagnostics.
        pub diagnostics: Vec<Diagnostic>,
        /// Every unexpanded model or resource frontier.
        pub frontiers: Vec<Frontier>,
        /// Pinned identity and effective root environment.
        pub metadata: crate::ReportMetadata,
        /// Original input rows before effects; Namespace root binding qualifies
        /// its coordinate as a supplied template rather than a physical account.
        pub accounts: Vec<crate::AccountState>,
        /// Entry and retained exit payloads keyed by native state.
        pub states: Vec<crate::StateDetails>,
        /// Shared dependency-ordered state/expression/path DAG.
        pub symbolic: crate::SymbolicGraph,
        /// Shared unexpanded pre-effect or incoming machine states.
        pub pending_machines: Vec<crate::MachineSnapshot>,
        /// Shared typed byte arrays referenced by frames, logs and outcomes.
        pub byte_arrays: Vec<crate::ByteArraySnapshot>,
        /// Shared transaction and rollback stores.
        pub stores: Vec<crate::StoreSnapshot>,
        /// Terminal root results and state changes.
        pub outcomes: Vec<crate::Outcome>,
        /// Native all-input terminal relation, independent of detailed coverage.
        /// None means no certificate was attached, including when requested.
        pub root_terminal_coverage: Option<crate::RootTerminalCoverage>,
        /// Known-call, recursion and environment relations, separate from native CFG/SSA.
        pub abstract_summaries: Vec<crate::AbstractSummary>,
    }
}

record! {
    /// Immutable publication with fixed graph placement and one source catalog.
    /// Numeric machine/SSA/pool identities belong to their graph namespace.
    pub struct AnalysisReport {
        /// Must equal the consumer's supported protocol version.
        pub schema_version: u16,
        /// Explicit initial display choice; selecting a view never moves graphs.
        pub selected_view: crate::AnalysisNamespace,
        /// Global model closure, independent of optional detail and display choice.
        pub status: AnalysisStatus,
        /// Cumulative execution work across all phases and fact revisions.
        pub work: u64,
        /// Cumulative transfers across all phases and fact revisions.
        pub transfers: u64,
        /// Work charged to this publication's shared postprocessing ledger.
        pub postprocessing_work: u64,
        /// Processing and queue durations, separate from semantic graph identity.
        pub timings: crate::AnalysisTimings,
        /// Cumulative fixed-snapshot acquisition, separate from immutable graph identity.
        pub acquisition: Option<crate::AcquisitionReport>,
        /// Complete immutable source directory shared by both graph namespaces.
        pub programs: Vec<std::sync::Arc<crate::ProgramInfo>>,
        /// Conservative global graph, when global analysis was requested.
        /// Shared fixed slots retain the exact projected graph across publications.
        pub global: Option<std::sync::Arc<AnalysisGraph>>,
        /// Native graph, including honest pending detail when available.
        pub native: Option<std::sync::Arc<AnalysisGraph>>,
        /// Global/native qualifications, receipts and optional RPC revision chain.
        pub hybrid: Option<crate::HybridReport>,
    }
}
record! {
    /// Source coordinate, independent of graph namespace and execution reachability.
    #[derive(Copy)]
    pub struct SourceBlockRef {
        /// Report-local immutable program identity.
        pub program: usize,
        /// Program-local syntactic block identity.
        pub basic_block: usize,
    }
}
impl CfgBlock {
    /// Captured source block, without asserting that any opcode was executed.
    pub fn source_block(&self) -> Option<SourceBlockRef> {
        Some(SourceBlockRef {
            program: self.program?,
            basic_block: self.basic_block?,
        })
    }
}
impl AnalysisReport {
    /// Follow numeric state/SSA/pool IDs only within this namespace.
    pub fn graph(&self, namespace: crate::AnalysisNamespace) -> Option<&AnalysisGraph> {
        match namespace {
            crate::AnalysisNamespace::Baseline => self.global.as_deref(),
            crate::AnalysisNamespace::Native => self.native.as_deref(),
        }
    }
    /// Explicitly selected graph; absent on a structurally invalid publication.
    pub fn selected_graph(&self) -> Option<&AnalysisGraph> {
        self.graph(self.selected_view)
    }
    /// Resolve a captured source coordinate from the one immutable catalog.
    pub fn source_block(&self, source: SourceBlockRef) -> Option<&DisasmBlock> {
        self.programs
            .iter()
            .find(|program| program.id == source.program)?
            .blocks
            .iter()
            .find(|block| block.id == source.basic_block)
    }
    /// Root source disassembly in a particular namespace, including unreachable code.
    pub fn disassembly(&self, namespace: crate::AnalysisNamespace) -> Option<&[DisasmBlock]> {
        let id = self.graph(namespace)?.metadata.root_program?;
        Some(
            &self
                .programs
                .iter()
                .find(|program| program.id == id)?
                .blocks,
        )
    }
}
