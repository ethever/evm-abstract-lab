choice! {
    /// Currentness of the retained execution receipt for a block.
    pub enum BlockCoverage {
        /// No transfer has executed this state.
        Unexecuted,
        /// Entry changed after the retained transfer; body is suppressed.
        Stale,
        /// Displayed instruction phases describe the current entry.
        Current,
    }
}
choice! {
    /// Observed instruction phase, separate from whole-graph closure.
    pub enum InstructionProgress {
        /// Reached opcode; operands remain unconsumed.
        Started,
        /// Consumed operands; no normal result is available.
        OperandsConsumed,
        /// Completed ordinary opcode semantics.
        Completed,
        /// Entered control dispatch; only observed edges carry results.
        Dispatched,
        /// Exceptional halt without ordinary stack transformation.
        Faulted,
    }
}
record! {
    /// An incoming SSA stack parameter on one original edge.
    pub struct PhiInput {
        /// Native edge ID, not an index into the sparse SSA transition vector.
        pub edge: usize,
        /// Native predecessor state ID.
        pub predecessor: usize,
        /// Incoming SSA value ID.
        pub value: usize,
    }
}
record! {
    /// Entry stack parameter with predecessor-specific operands.
    pub struct Phi {
        /// Defining SSA value ID.
        pub result: usize,
        /// Frame index, oldest suspended caller first.
        pub frame: usize,
        /// Stack slot, bottom to top.
        pub slot: usize,
        /// Supported incoming arguments only.
        pub inputs: Vec<PhiInput>,
    }
}
record! {
    /// One bytecode attempt with named uses, definitions and effects.
    pub struct SsaInstruction {
        /// Opcode byte offset.
        pub pc: usize,
        /// Raw opcode byte.
        pub opcode: u8,
        /// Upstream opcode mnemonic.
        pub name: String,
        /// Lossless hexadecimal PUSH value.
        pub immediate: Option<String>,
        /// SSA arguments, in EVM pop order; DUP/SWAP retain existing names.
        pub operands: Vec<usize>,
        /// New definitions only; pending calls never invent a result.
        pub results: Vec<usize>,
        /// Exceptional halt at this instruction.
        pub fault: bool,
        /// Exact observed phase.
        pub progress: InstructionProgress,
        /// Effect bundle before the attempt.
        pub effect_input: usize,
        /// Observed effect after the attempt, absent for Started.
        pub effect_result: Option<usize>,
    }
}
record! {
    /// Incoming machine-effect bundle.
    pub struct EffectInput {
        /// Original edge ID.
        pub edge: usize,
        /// Effect name carried by that edge.
        pub effect: usize,
    }
}
record! {
    /// Native SSA block with explicit incoming and execution coverage.
    pub struct SsaBlock {
        /// Native machine state ID shared with CFG nodes.
        pub state: usize,
        /// Currentness of the instruction receipt.
        pub coverage: BlockCoverage,
        /// True only if both whole graph and incoming coverage are complete.
        pub incoming_complete: bool,
        /// Stack entry parameters for every frame.
        pub phis: Vec<Phi>,
        /// Recorded instruction attempts in program order.
        pub instructions: Vec<SsaInstruction>,
        /// Observed stack names, or entry-only names when stale/unexecuted.
        pub exit_frames: Vec<Vec<usize>>,
        /// Entry effect definition.
        pub effect: usize,
        /// Supported incoming effect arguments.
        pub effect_inputs: Vec<EffectInput>,
        /// Last observed effect name.
        pub exit_effect: usize,
        /// Original incoming edges without supported SSA transitions.
        pub open_incoming: Vec<usize>,
    }
}
record! {
    /// Supported graph transition with stack and effect arguments.
    pub struct SsaTransition {
        /// Original native graph edge ID.
        pub edge: usize,
        /// Control dispatch kind.
        pub kind: crate::EdgeKind,
        /// Opcode operands used by the transition.
        pub operands: Vec<usize>,
        /// Destination frame stacks, bottom to top.
        pub stacks: Vec<Vec<usize>>,
        /// Input effect name.
        pub effect_input: usize,
        /// Output effect definition.
        pub effect_result: usize,
        /// Deferred call/creation result, when defined by this transition.
        pub result: Option<usize>,
    }
}
choice! {
    /// Why an observed edge cannot support current SSA arguments.
    pub enum DeferredReason {
        /// Source has not executed.
        SourceUnexecuted,
        /// Source entry changed after execution.
        SourceStale,
        /// Current receipt did not emit this accumulated edge.
        NotInExecutionEvidence,
        /// Source stopped in a pending instruction.
        SourcePendingInstruction,
    }
}
record! {
    /// An observed graph edge that is intentionally absent from SSA transitions.
    pub struct DeferredEdge {
        /// Original native edge ID.
        pub edge: usize,
        /// Explicit coverage reason.
        pub reason: DeferredReason,
    }
}
record! {
    /// Structurally verified SSA with partial coverage represented explicitly.
    pub struct SsaReport {
        /// True only after complete native SSA validation has succeeded.
        pub complete: bool,
        /// Number of unique stack value definitions.
        pub value_count: usize,
        /// Number of unique machine-effect definitions.
        pub effect_count: usize,
        /// Blocks in native state order.
        pub blocks: Vec<SsaBlock>,
        /// Supported transitions; IDs may be sparse.
        pub transitions: Vec<SsaTransition>,
        /// Observed edges left deliberately unsupported.
        pub deferred_edges: Vec<DeferredEdge>,
    }
}
