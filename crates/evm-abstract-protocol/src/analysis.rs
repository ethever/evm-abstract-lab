choice! {
    /// Input assumptions; this version does not accept an RPC or an offline world.
    pub enum AnalysisScope {
        /// One runtime program; calldata, environment and persistent state unknown.
        SingleProgram,
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
        /// Native state ID shared with SSA, edges, diagnostics and frontiers.
        pub id: usize,
        /// Program-local syntactic block index for this active frame.
        pub basic_block: usize,
        /// First instruction offset, absent for a no-code frame.
        pub start_pc: Option<usize>,
        /// Recent internal jump source offsets.
        pub context: Vec<usize>,
        /// Number of active and suspended frames.
        pub frame_depth: usize,
        /// Executable account identity, distinct from the storage account.
        pub code_address: String,
        /// Source instructions for this frame, including generated initcode.
        pub instructions: Vec<DisasmInstruction>,
        /// Active stack on entry, bottom to top; each entry is an abstract value summary.
        pub entry_stack: Vec<String>,
        /// Last observed active exit stack, bottom to top.
        pub exit_stack: Vec<String>,
        /// Offsets actually visited by the latest transfer.
        pub executed_pcs: Vec<usize>,
    }
}
choice! {
    /// Typed control-transfer category, including native call dispatch.
    pub enum EdgeKind {
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
        /// Precision exchange reached its configured bound.
        FactExchangeLimited,
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
        /// Symbolic or SMT bound.
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
        /// Full engine boundary detail, including account/slot when available.
        pub detail: String,
    }
}
record! {
    /// Immutable structured analysis artifact consumed by all custom widgets.
    pub struct AnalysisReport {
        /// Must equal the consumer's supported protocol version.
        pub schema_version: u16,
        /// Explicit input/model scope.
        pub scope: AnalysisScope,
        /// Execution rules fixed during decoding.
        pub fork: crate::Fork,
        /// Submitted root program size in bytes.
        pub byte_len: usize,
        /// Model closure, independent of SSA display selection.
        pub status: AnalysisStatus,
        /// Executed transfers, including revisits.
        pub transfers: usize,
        /// Complete root source disassembly.
        pub disassembly: Vec<DisasmBlock>,
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
    }
}
