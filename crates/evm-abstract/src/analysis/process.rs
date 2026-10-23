//! Coarse EVM process semantics, independent of captured executable identity.
//!
//! A process receipt describes conservative actions of a whole captured region.
//! It never witnesses execution of an instruction PC. Native Body/Resume slots
//! retain input, output and continuation currency; these types add no scheduler.

use alloy_primitives::Address;
use serde::Serialize;

/// Frozen selection for captured runtime bodies; ordinary execution is default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeExecution {
    /// Execute captured instructions using the existing precise provider.
    #[default]
    Native,
    /// Cover a captured runtime with a conservative action/effect process.
    Process,
}

/// Transfer interpretation, separately from a frame's original code kind/hash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum TransferSemantics {
    /// Actual captured-instruction, empty-code or native-precompile execution.
    #[default]
    Native,
    /// An unobserved implementation's operational environment process.
    Environment,
    /// A conservative process for the original captured runtime Program.
    KnownProcess,
}

/// EVM action kind; `FrameKey::mode` instead identifies the kind of code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProcessAction {
    /// Ordinary CALL, changing the child's state owner to its target.
    Call,
    /// STATICCALL, changing owner and propagating read-only execution.
    StaticCall,
    /// DELEGATECALL, preserving owner, inherited caller and call value.
    DelegateCall,
    /// CALLCODE, preserving owner and supplying the current owner as caller.
    CallCode,
    /// CREATE with constructor and deployment completion obligations.
    Create,
    /// CREATE2 with salt, constructor and deployment obligations.
    Create2,
}

/// Target class of a conservative action; residual is not an observed address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProcessOwner {
    /// An exact owner retained in this invocation's current named vocabulary.
    Named(Address),
    /// Every remaining owner, including unresolved aliases and future owners.
    Residual,
}

/// A semantic rule not yet represented by this native process checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ProcessBoundary {
    /// No covering residual-owner Store/alias representation was admitted.
    ResidualOwner(ProcessAction),
    /// Constructor/deployment semantics require the allocation implementation.
    Creation(ProcessAction),
    /// Foreign analyzers cannot yet consume the process input representation.
    CooperativeInputs,
    /// A symbolic logical owner needs a covering owner/alias representation.
    SymbolicOwner,
    /// A process quotient cannot be reinterpreted as a native machine input.
    NativeInstantiation,
}

/// Actual coarse transfer receipt, not instruction or SSA evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmProcessTransfer {
    /// Local process actions may repeat before any represented exit.
    pub repeat: bool,
    /// Primitive action rules selected by the paid decoded capability scan.
    pub actions: Vec<ProcessAction>,
    /// Every uncovered rule also remains an explicit incomplete frontier.
    pub unresolved: Vec<ProcessBoundary>,
}
