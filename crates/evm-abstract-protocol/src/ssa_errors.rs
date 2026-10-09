//! Exact structural SSA failure contracts and native coordinates.

choice! {
    /// Exact native SSA verifier contract.
    pub enum SsaInvariantKind {
    /// CALL receipt does not defer its dispatch.
    CallDispatch,
    /// DUP invented a value.
    DupResult,
    /// SSA instruction differs from executing code.
    RuntimeInstructionIdentity,
    /// SSA instruction outside runtime.
    InstructionOutsideRuntime,
    /// SSA names are not contiguous unique definitions.
    ContiguousNames,
    /// SSA outgoing frame stacks differ from abstract execution.
    ExitFrameShape,
    /// SWAP invented a value.
    SwapResult,
    /// Analysis edge has no partial state.
    EdgeStateMissing,
    /// Block count mismatch.
    BlockCount,
    /// Block identity mismatch.
    BlockIdentity,
    /// Call destination is not a child frame.
    CallChildFrame,
    /// Call edge is not a suspension and empty callee entry.
    CallSuspension,
    /// Call/return operands or effect dependency mismatch.
    TransitionDependencies,
    /// Caller resumed without CALL result.
    ContinuationResultMissing,
    /// Completed prefix hides an actual fault.
    CompletedFault,
    /// Completed receipt hides a bytecode/stack fault.
    CompletedReceiptFault,
    /// Current partial receipt has no observed exit.
    CurrentExitMissing,
    /// Dispatch invented a normal instruction result.
    DispatchResult,
    /// Dispatch receipt would invent a normal result.
    DispatchReceiptResult,
    /// Duplicate call result definition.
    DuplicateContinuationValue,
    /// Duplicate effect definition.
    DuplicateEffect,
    /// Duplicate partial continuation result.
    DuplicatePartialContinuationValue,
    /// Duplicate partial effect definition.
    DuplicatePartialEffect,
    /// Duplicate partial value definition.
    DuplicatePartialValue,
    /// Duplicate stack definition.
    DuplicateStackValue,
    /// Effect phi does not cover incoming edges.
    EffectPhiCoverage,
    /// Entry stack height mismatch.
    EntryStackHeight,
    /// Executed instructions or effect chain mismatch.
    ExecutionEffectCount,
    /// Executed pc mismatch.
    ExecutedPcs,
    /// Exit stack height mismatch.
    ExitStackHeight,
    /// Extra frame parameters.
    ExtraFrameParameters,
    /// Extra partial parameters or duplicate entry effect.
    PartialParameterOrEffect,
    /// Fault invented a result.
    FaultResult,
    /// Fault receipt has no bytecode/stack fault.
    FaultReceipt,
    /// Faulted prefix does not match its actual fault.
    FaultedPrefix,
    /// Frame parameter identity or definition mismatch.
    FrameParameterIdentity,
    /// Frame phi inputs differ from edge arguments.
    FramePhiArguments,
    /// Instruction effect chain is broken.
    InstructionEffectChain,
    /// Instruction fault differs from stack/bytecode.
    InstructionFault,
    /// Instruction result arity mismatch.
    InstructionResultArity,
    /// Local edge changes call depth.
    LocalEdgeDepth,
    /// Machine state identity mismatch.
    MachineStateIdentity,
    /// Missing covered edge parameter.
    CoveredEdgeParameterMissing,
    /// Missing edge stack argument.
    EdgeArgumentMissing,
    /// Missing frame parameter.
    FrameParameterMissing,
    /// Missing partial frame parameter.
    PartialFrameParameterMissing,
    /// Opcode or immediate differs from the bytecode.
    BytecodeIdentity,
    /// Operands violate stack identity or local definition order.
    OperandIdentity,
    /// Outgoing stack or machine effects differ from execution.
    ExitStackOrEffect,
    /// Partial DUP invented a definition.
    PartialDupResult,
    /// Partial SSA definitions are not unique contiguous names.
    PartialContiguousNames,
    /// Partial SSA lost or changed an analysis frontier.
    FrontierCoverage,
    /// Partial SWAP invented a definition.
    PartialSwapResult,
    /// Partial call is not an evidenced suspension and empty child entry.
    PartialCallSuspension,
    /// Partial call operands or effect dependencies mismatch.
    PartialTransitionDependencies,
    /// Partial continuation has no caller.
    PartialCallerMissing,
    /// Partial continuation has no result.
    PartialContinuationResultMissing,
    /// Partial deferred edges do not cover the unsupported graph edges.
    DeferredEdgeCoverage,
    /// Partial edge stack arguments differ from its covered frames.
    PartialEdgeArguments,
    /// Partial effect claims the wrong instruction phase.
    PartialEffectPhase,
    /// Partial effect phi differs from supported incoming edges.
    PartialEffectPhiCoverage,
    /// Partial frame stacks differ from the observed instruction phase.
    PartialExitFrameShape,
    /// Partial instruction count differs from execution receipt.
    PartialInstructionCount,
    /// Partial instruction effect chain is broken.
    PartialInstructionEffectChain,
    /// Partial instruction outside captured bytecode.
    PartialInstructionOutsideRuntime,
    /// Partial instruction result arity mismatch.
    PartialInstructionArity,
    /// Partial instruction uses violate stack identity or definition order.
    PartialOperandIdentity,
    /// Partial local edge changed frame depth or invented a result.
    PartialLocalEdgeDepth,
    /// Partial observed stacks or effects differ from its prefix.
    PartialExitStackOrEffect,
    /// Partial parameter identity or definition mismatch.
    PartialParameterIdentity,
    /// Partial phase or bytecode identity differs from execution.
    PartialInstructionIdentity,
    /// Partial phi coverage claims nonexistent incoming evidence.
    PartialPhiCoverage,
    /// Partial phi differs from supported edge arguments.
    PartialPhiArguments,
    /// Partial prefix continued after an unfinished or dispatched step.
    UnfinishedPrefix,
    /// Partial receipt does not match visited PCs.
    ReceiptPcCount,
    /// Partial receipt is not a bytecode prefix.
    ReceiptPrefix,
    /// Partial receipt is outside captured bytecode.
    ReceiptOutsideRuntime,
    /// Partial return does not match exactly one caller frame.
    PartialReturnDepth,
    /// Partial return has no caller.
    PartialReturnCallerMissing,
    /// Partial return popped a root frame.
    RootReturn,
    /// Partial state has no active frame.
    PartialActiveFrameMissing,
    /// Partial state has no frame.
    PartialFrameMissing,
    /// Partial state identity or currentness mismatch.
    PartialStateIdentity,
    /// Partial status or state coverage differs from analysis.
    PartialStatusOrBlockCount,
    /// Partial transition IDs are not ordered original edge IDs.
    PartialTransitionOrder,
    /// Partial transition crosses unrelated frame identities.
    PartialFrameIdentity,
    /// Partial transition kind changed.
    PartialTransitionKind,
    /// Partial transition stack does not match destination.
    PartialDestinationArguments,
    /// Pending instruction invented a normal result.
    PendingResult,
    /// Pending operand consumption is invalid.
    PendingOperands,
    /// Phi input must equal the predecessor's outgoing slot.
    PhiOutgoingSlot,
    /// Phi must have exactly one input per distinct predecessor.
    PhiPredecessorCoverage,
    /// Phi slot identity mismatch.
    PhiSlotIdentity,
    /// Return does not match one caller frame.
    ReturnDepth,
    /// Return has no caller.
    ReturnCallerMissing,
    /// Return source is not a child frame.
    ReturnChildFrame,
    /// Return without caller.
    ReturnWithoutCaller,
    /// Same-depth partial return is not a failed call.
    PartialSameDepthReturn,
    /// Same-frame return is not an immediate call failure.
    SameDepthReturn,
    /// Stale or unexecuted state received an instruction body.
    UnexecutedBody,
    /// Started instruction invented a fault or result.
    StartedResult,
    /// State has no active frame.
    ActiveFrameMissing,
    /// State without frame.
    FrameMissing,
    /// Supported partial transition is missing.
    TransitionMissing,
    /// Transition crosses unrelated frame identities.
    FrameIdentity,
    /// Transition identity mismatch.
    TransitionIdentity,
    /// Transition stack arguments mismatch.
    TransitionArguments,
    /// Transition stack does not match destination.
    DestinationArguments,
    /// Unreachable SSA block.
    UnreachableBlock,
    /// Unsupported historical edge received an SSA transition.
    UnsupportedTransition,
    /// Value IDs must be contiguous and each defined once.
    ContiguousValues,
    /// Values without blocks.
    ValuesWithoutBlocks,
    /// World SSA graph size differs from analysis.
    WorldGraphSize,
    /// Duplicate definition.
    DuplicateDefinition,
    /// Undefined value.
    UndefinedValue,
    /// Use before definition.
    UseBeforeDefinition,
    /// Definition does not dominate use.
    NonDominatingDefinition,
    /// Stack height mismatch on edge.
    EdgeStackHeight,
    }
}
record! {
    /// Native invariant failure. Absent coordinates denote graph-wide contracts.
    pub struct SsaInvariant {
        /// Exact violated contract.
        pub kind: SsaInvariantKind,
        /// Native state coordinate.
        pub state: Option<usize>,
        /// Native edge coordinate.
        pub edge: Option<usize>,
        /// Native pc coordinate.
        pub pc: Option<usize>,
        /// Native frame coordinate.
        pub frame: Option<usize>,
        /// Native slot coordinate.
        pub slot: Option<usize>,
        /// Native value coordinate.
        pub value: Option<usize>,
        /// Native effect coordinate.
        pub effect: Option<usize>,
        /// Native source state coordinate.
        pub source_state: Option<usize>,
        /// Native target state coordinate.
        pub target_state: Option<usize>,
        /// Native expected coordinate.
        pub expected: Option<usize>,
        /// Native observed coordinate.
        pub observed: Option<usize>,
    }
}
variant! {
    /// SSA conversion failure with no string-only structural reasons.
    pub enum SsaFailure {
        /// Closed-world SSA requires complete graph coverage.
        Coverage,
        /// Exact verifier invariant and contextual coordinates.
        Invariant(Box<SsaInvariant>),
    }
}
