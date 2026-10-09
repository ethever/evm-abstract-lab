//! Typed SSA contract failures. Coordinates use native state/edge IDs and
//! distinguish stack-value names from machine-effect names.

use super::SsaError;
use serde::Serialize;
use std::fmt;

/// The exact violated contract; human diagnostics do not carry hidden IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
pub enum SsaInvariantKind {
    /// CALL receipt does not defer its dispatch.
    #[error("CALL receipt does not defer its dispatch")]
    CallDispatch,
    /// DUP invented a value.
    #[error("DUP invented a value")]
    DupResult,
    /// SSA instruction differs from executing code.
    #[error("SSA instruction differs from executing code")]
    RuntimeInstructionIdentity,
    /// SSA instruction outside runtime.
    #[error("SSA instruction outside runtime")]
    InstructionOutsideRuntime,
    /// SSA names are not contiguous unique definitions.
    #[error("SSA names are not contiguous unique definitions")]
    ContiguousNames,
    /// SSA outgoing frame stacks differ from abstract execution.
    #[error("SSA outgoing frame stacks differ from abstract execution")]
    ExitFrameShape,
    /// SWAP invented a value.
    #[error("SWAP invented a value")]
    SwapResult,
    /// Analysis edge has no partial state.
    #[error("analysis edge has no partial state")]
    EdgeStateMissing,
    /// Block count mismatch.
    #[error("block count mismatch")]
    BlockCount,
    /// Block identity mismatch.
    #[error("block identity mismatch")]
    BlockIdentity,
    /// Call destination is not a child frame.
    #[error("call destination is not a child frame")]
    CallChildFrame,
    /// Call edge is not a suspension and empty callee entry.
    #[error("call edge is not a suspension and empty callee entry")]
    CallSuspension,
    /// Call/return operands or effect dependency mismatch.
    #[error("call/return operands or effect dependency mismatch")]
    TransitionDependencies,
    /// Caller resumed without CALL result.
    #[error("caller resumed without CALL result")]
    ContinuationResultMissing,
    /// Completed prefix hides an actual fault.
    #[error("completed prefix hides an actual fault")]
    CompletedFault,
    /// Completed receipt hides a bytecode/stack fault.
    #[error("completed receipt hides a bytecode/stack fault")]
    CompletedReceiptFault,
    /// Current partial receipt has no observed exit.
    #[error("current partial receipt has no observed exit")]
    CurrentExitMissing,
    /// Dispatch invented a normal instruction result.
    #[error("dispatch invented a normal instruction result")]
    DispatchResult,
    /// Dispatch receipt would invent a normal result.
    #[error("dispatch receipt would invent a normal result")]
    DispatchReceiptResult,
    /// Duplicate call result definition.
    #[error("duplicate call result definition")]
    DuplicateContinuationValue,
    /// Duplicate effect definition.
    #[error("duplicate effect definition")]
    DuplicateEffect,
    /// Duplicate partial continuation result.
    #[error("duplicate partial continuation result")]
    DuplicatePartialContinuationValue,
    /// Duplicate partial effect definition.
    #[error("duplicate partial effect definition")]
    DuplicatePartialEffect,
    /// Duplicate partial value definition.
    #[error("duplicate partial value definition")]
    DuplicatePartialValue,
    /// Duplicate stack definition.
    #[error("duplicate stack definition")]
    DuplicateStackValue,
    /// Effect phi does not cover incoming edges.
    #[error("effect phi does not cover incoming edges")]
    EffectPhiCoverage,
    /// Entry stack height mismatch.
    #[error("entry stack height mismatch")]
    EntryStackHeight,
    /// Executed instructions or effect chain mismatch.
    #[error("executed instructions or effect chain mismatch")]
    ExecutionEffectCount,
    /// Executed pc mismatch.
    #[error("executed pc mismatch")]
    ExecutedPcs,
    /// Exit stack height mismatch.
    #[error("exit stack height mismatch")]
    ExitStackHeight,
    /// Extra frame parameters.
    #[error("extra frame parameters")]
    ExtraFrameParameters,
    /// Extra partial parameters or duplicate entry effect.
    #[error("extra partial parameters or duplicate entry effect")]
    PartialParameterOrEffect,
    /// Fault invented a result.
    #[error("fault invented a result")]
    FaultResult,
    /// Fault receipt has no bytecode/stack fault.
    #[error("fault receipt has no bytecode/stack fault")]
    FaultReceipt,
    /// Faulted prefix does not match its actual fault.
    #[error("faulted prefix does not match its actual fault")]
    FaultedPrefix,
    /// Frame parameter identity or definition mismatch.
    #[error("frame parameter identity or definition mismatch")]
    FrameParameterIdentity,
    /// Frame phi inputs differ from edge arguments.
    #[error("frame phi inputs differ from edge arguments")]
    FramePhiArguments,
    /// Instruction effect chain is broken.
    #[error("instruction effect chain is broken")]
    InstructionEffectChain,
    /// Instruction fault differs from stack/bytecode.
    #[error("instruction fault differs from stack/bytecode")]
    InstructionFault,
    /// Instruction result arity mismatch.
    #[error("instruction result arity mismatch")]
    InstructionResultArity,
    /// Local edge changes call depth.
    #[error("local edge changes call depth")]
    LocalEdgeDepth,
    /// Machine state identity mismatch.
    #[error("machine state identity mismatch")]
    MachineStateIdentity,
    /// Missing covered edge parameter.
    #[error("missing covered edge parameter")]
    CoveredEdgeParameterMissing,
    /// Missing edge stack argument.
    #[error("missing edge stack argument")]
    EdgeArgumentMissing,
    /// Missing frame parameter.
    #[error("missing frame parameter")]
    FrameParameterMissing,
    /// Missing partial frame parameter.
    #[error("missing partial frame parameter")]
    PartialFrameParameterMissing,
    /// Opcode or immediate differs from the bytecode.
    #[error("opcode or immediate differs from the bytecode")]
    BytecodeIdentity,
    /// Operands violate stack identity or local definition order.
    #[error("operands violate stack identity or local definition order")]
    OperandIdentity,
    /// Outgoing stack or machine effects differ from execution.
    #[error("outgoing stack or machine effects differ from execution")]
    ExitStackOrEffect,
    /// Partial DUP invented a definition.
    #[error("partial DUP invented a definition")]
    PartialDupResult,
    /// Partial SSA definitions are not unique contiguous names.
    #[error("partial SSA definitions are not unique contiguous names")]
    PartialContiguousNames,
    /// Partial SSA lost or changed an analysis frontier.
    #[error("partial SSA lost or changed an analysis frontier")]
    FrontierCoverage,
    /// Partial SWAP invented a definition.
    #[error("partial SWAP invented a definition")]
    PartialSwapResult,
    /// Partial call is not an evidenced suspension and empty child entry.
    #[error("partial call is not an evidenced suspension and empty child entry")]
    PartialCallSuspension,
    /// Partial call operands or effect dependencies mismatch.
    #[error("partial call operands or effect dependencies mismatch")]
    PartialTransitionDependencies,
    /// Partial continuation has no caller.
    #[error("partial continuation has no caller")]
    PartialCallerMissing,
    /// Partial continuation has no result.
    #[error("partial continuation has no result")]
    PartialContinuationResultMissing,
    /// Partial deferred edges do not cover the unsupported graph edges.
    #[error("partial deferred edges do not cover the unsupported graph edges")]
    DeferredEdgeCoverage,
    /// Partial edge stack arguments differ from its covered frames.
    #[error("partial edge stack arguments differ from its covered frames")]
    PartialEdgeArguments,
    /// Partial effect claims the wrong instruction phase.
    #[error("partial effect claims the wrong instruction phase")]
    PartialEffectPhase,
    /// Partial effect phi differs from supported incoming edges.
    #[error("partial effect phi differs from supported incoming edges")]
    PartialEffectPhiCoverage,
    /// Partial frame stacks differ from the observed instruction phase.
    #[error("partial frame stacks differ from the observed instruction phase")]
    PartialExitFrameShape,
    /// Partial instruction count differs from execution receipt.
    #[error("partial instruction count differs from execution receipt")]
    PartialInstructionCount,
    /// Partial instruction effect chain is broken.
    #[error("partial instruction effect chain is broken")]
    PartialInstructionEffectChain,
    /// Partial instruction outside captured bytecode.
    #[error("partial instruction outside captured bytecode")]
    PartialInstructionOutsideRuntime,
    /// Partial instruction result arity mismatch.
    #[error("partial instruction result arity mismatch")]
    PartialInstructionArity,
    /// Partial instruction uses violate stack identity or definition order.
    #[error("partial instruction uses violate stack identity or definition order")]
    PartialOperandIdentity,
    /// Partial local edge changed frame depth or invented a result.
    #[error("partial local edge changed frame depth or invented a result")]
    PartialLocalEdgeDepth,
    /// Partial observed stacks or effects differ from its prefix.
    #[error("partial observed stacks or effects differ from its prefix")]
    PartialExitStackOrEffect,
    /// Partial parameter identity or definition mismatch.
    #[error("partial parameter identity or definition mismatch")]
    PartialParameterIdentity,
    /// Partial phase or bytecode identity differs from execution.
    #[error("partial phase or bytecode identity differs from execution")]
    PartialInstructionIdentity,
    /// Partial phi coverage claims nonexistent incoming evidence.
    #[error("partial phi coverage claims nonexistent incoming evidence")]
    PartialPhiCoverage,
    /// Partial phi differs from supported edge arguments.
    #[error("partial phi differs from supported edge arguments")]
    PartialPhiArguments,
    /// Partial prefix continued after an unfinished or dispatched step.
    #[error("partial prefix continued after an unfinished or dispatched step")]
    UnfinishedPrefix,
    /// Partial receipt does not match visited PCs.
    #[error("partial receipt does not match visited PCs")]
    ReceiptPcCount,
    /// Partial receipt is not a bytecode prefix.
    #[error("partial receipt is not a bytecode prefix")]
    ReceiptPrefix,
    /// Partial receipt is outside captured bytecode.
    #[error("partial receipt is outside captured bytecode")]
    ReceiptOutsideRuntime,
    /// Partial return does not match exactly one caller frame.
    #[error("partial return does not match exactly one caller frame")]
    PartialReturnDepth,
    /// Partial return has no caller.
    #[error("partial return has no caller")]
    PartialReturnCallerMissing,
    /// Partial return popped a root frame.
    #[error("partial return popped a root frame")]
    RootReturn,
    /// Partial state has no active frame.
    #[error("partial state has no active frame")]
    PartialActiveFrameMissing,
    /// Partial state has no frame.
    #[error("partial state has no frame")]
    PartialFrameMissing,
    /// Partial state identity or currentness mismatch.
    #[error("partial state identity or currentness mismatch")]
    PartialStateIdentity,
    /// Partial status or state coverage differs from analysis.
    #[error("partial status or state coverage differs from analysis")]
    PartialStatusOrBlockCount,
    /// Partial transition IDs are not ordered original edge IDs.
    #[error("partial transition IDs are not ordered original edge IDs")]
    PartialTransitionOrder,
    /// Partial transition crosses unrelated frame identities.
    #[error("partial transition crosses unrelated frame identities")]
    PartialFrameIdentity,
    /// Partial transition kind changed.
    #[error("partial transition kind changed")]
    PartialTransitionKind,
    /// Partial transition stack does not match destination.
    #[error("partial transition stack does not match destination")]
    PartialDestinationArguments,
    /// Pending instruction invented a normal result.
    #[error("pending instruction invented a normal result")]
    PendingResult,
    /// Pending operand consumption is invalid.
    #[error("pending operand consumption is invalid")]
    PendingOperands,
    /// Phi input must equal the predecessor's outgoing slot.
    #[error("phi input must equal the predecessor's outgoing slot")]
    PhiOutgoingSlot,
    /// Phi must have exactly one input per distinct predecessor.
    #[error("phi must have exactly one input per distinct predecessor")]
    PhiPredecessorCoverage,
    /// Phi slot identity mismatch.
    #[error("phi slot identity mismatch")]
    PhiSlotIdentity,
    /// Return does not match one caller frame.
    #[error("return does not match one caller frame")]
    ReturnDepth,
    /// Return has no caller.
    #[error("return has no caller")]
    ReturnCallerMissing,
    /// Return source is not a child frame.
    #[error("return source is not a child frame")]
    ReturnChildFrame,
    /// Return without caller.
    #[error("return without caller")]
    ReturnWithoutCaller,
    /// Same-depth partial return is not a failed call.
    #[error("same-depth partial return is not a failed call")]
    PartialSameDepthReturn,
    /// Same-frame return is not an immediate call failure.
    #[error("same-frame return is not an immediate call failure")]
    SameDepthReturn,
    /// Stale or unexecuted state received an instruction body.
    #[error("stale or unexecuted state received an instruction body")]
    UnexecutedBody,
    /// Started instruction invented a fault or result.
    #[error("started instruction invented a fault or result")]
    StartedResult,
    /// State has no active frame.
    #[error("state has no active frame")]
    ActiveFrameMissing,
    /// State without frame.
    #[error("state without frame")]
    FrameMissing,
    /// Supported partial transition is missing.
    #[error("supported partial transition is missing")]
    TransitionMissing,
    /// Transition crosses unrelated frame identities.
    #[error("transition crosses unrelated frame identities")]
    FrameIdentity,
    /// Transition identity mismatch.
    #[error("transition identity mismatch")]
    TransitionIdentity,
    /// Transition stack arguments mismatch.
    #[error("transition stack arguments mismatch")]
    TransitionArguments,
    /// Transition stack does not match destination.
    #[error("transition stack does not match destination")]
    DestinationArguments,
    /// Unreachable SSA block.
    #[error("unreachable SSA block")]
    UnreachableBlock,
    /// Unsupported historical edge received an SSA transition.
    #[error("unsupported historical edge received an SSA transition")]
    UnsupportedTransition,
    /// Value IDs must be contiguous and each defined once.
    #[error("value IDs must be contiguous and each defined once")]
    ContiguousValues,
    /// Values without blocks.
    #[error("values without blocks")]
    ValuesWithoutBlocks,
    /// World SSA graph size differs from analysis.
    #[error("world SSA graph size differs from analysis")]
    WorldGraphSize,
    /// Duplicate definition.
    #[error("duplicate definition")]
    DuplicateDefinition,
    /// Undefined value.
    #[error("undefined value")]
    UndefinedValue,
    /// Use before definition.
    #[error("use before definition")]
    UseBeforeDefinition,
    /// Definition does not dominate use.
    #[error("definition does not dominate use")]
    NonDominatingDefinition,
    /// Stack height mismatch on edge.
    #[error("stack height mismatch on edge")]
    EdgeStackHeight,
}

/// Structural failure with available native coordinates. `expected` and
/// `observed` are the count or identifier compared by `kind`, never text parsed
/// from a renderer. Missing coordinates mean that contract is graph-wide.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SsaInvariant {
    /// Exact invariant discriminant.
    pub kind: SsaInvariantKind,
    /// State being built or verified.
    pub state: Option<usize>,
    /// Original analysis edge ID.
    pub edge: Option<usize>,
    /// Bytecode program counter.
    pub pc: Option<usize>,
    /// Oldest-caller-first frame index.
    pub frame: Option<usize>,
    /// Bottom-to-top stack slot.
    pub slot: Option<usize>,
    /// SSA stack-value name.
    pub value: Option<usize>,
    /// SSA machine-effect name.
    pub effect: Option<usize>,
    /// Source of a transition or definition.
    pub source_state: Option<usize>,
    /// Destination of a transition or use.
    pub target_state: Option<usize>,
    /// Expected count or identity for this contract.
    pub expected: Option<usize>,
    /// Observed count or identity for this contract.
    pub observed: Option<usize>,
}

impl fmt::Display for SsaInvariant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.kind)?;
        for (label, coordinate) in [
            ("state", self.state),
            ("edge", self.edge),
            ("pc", self.pc),
            ("frame", self.frame),
            ("slot", self.slot),
            ("value", self.value),
            ("effect", self.effect),
            ("source_state", self.source_state),
            ("target_state", self.target_state),
            ("expected", self.expected),
            ("observed", self.observed),
        ] {
            if let Some(coordinate) = coordinate {
                write!(formatter, " {label}={coordinate}")?;
            }
        }
        Ok(())
    }
}

pub(super) fn invariant(kind: SsaInvariantKind) -> SsaError {
    SsaError::Invariant(Box::new(SsaInvariant {
        kind,
        state: None,
        edge: None,
        pc: None,
        frame: None,
        slot: None,
        value: None,
        effect: None,
        source_state: None,
        target_state: None,
        expected: None,
        observed: None,
    }))
}

impl SsaError {
    pub(crate) fn state(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.state = Some(value);
        }
        self
    }
    pub(crate) fn edge(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.edge = Some(value);
        }
        self
    }
    pub(crate) fn pc(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.pc = Some(value);
        }
        self
    }
    pub(crate) fn frame(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.frame = Some(value);
        }
        self
    }
    pub(crate) fn slot(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.slot = Some(value);
        }
        self
    }
    pub(crate) fn value(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.value = Some(value);
        }
        self
    }
    pub(crate) fn effect(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.effect = Some(value);
        }
        self
    }
    pub(crate) fn source_state(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.source_state = Some(value);
        }
        self
    }
    pub(crate) fn target_state(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.target_state = Some(value);
        }
        self
    }
    pub(crate) fn expected(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.expected = Some(value);
        }
        self
    }
    pub(crate) fn observed(mut self, value: usize) -> Self {
        if let Self::Invariant(detail) = &mut self {
            detail.observed = Some(value);
        }
        self
    }
}
