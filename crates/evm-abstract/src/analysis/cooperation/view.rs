//! Semantic views and explicitly selected cooperation phases.

use crate::{
    analysis::{EvmMachinePayload, ProcessFrameKey},
    bytecode::Program,
    domain::{EvmValue, WordElement},
    world::EvmStore,
};

/// Representations accepted by a component assembly. Existing stack analyzers
/// must explicitly opt into Process inputs before observing that abstraction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputViews {
    /// Ordinary machine inputs, including the initial World admission.
    #[default]
    Native,
    /// Both machine inputs and the typed Process control/invocation/Store view.
    NativeAndProcess,
}

/// Phases in which the caller deliberately registers an extension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhaseScope {
    /// Every phase must support the supplied views; none is silently skipped.
    #[default]
    All,
    /// In Hybrid, register only in optional Native detail. Baseline has its own
    /// unextended kernel; mutable analyzers and stores are never shared with it.
    /// A direct Process run still requires Process view support.
    NativeOnly,
}

/// The genuine joined invocation operands, independent of adapter key anchors.
#[derive(Clone, Copy, Debug)]
pub struct InvocationView<'a, N: WordElement> {
    /// Logical ADDRESS, including symbolic owners. Missing means unavailable;
    /// key adapter anchors must never be substituted.
    pub address: Option<&'a EvmValue<N>>,
    /// Logical CALLER, including inherited DELEGATECALL callers. Missing means
    /// unavailable, never a key-implied fabricated value.
    pub caller: Option<&'a EvmValue<N>>,
    /// Logical CALLVALUE, including inherited DELEGATECALL values.
    pub value: &'a EvmValue<N>,
}

/// One immutable Process bound. No private stack, memory or executed PC is
/// exposed. Program is source inventory, not a reachability certificate.
#[derive(Clone, Copy, Debug)]
pub struct ProcessView<'a, N: WordElement> {
    /// Source, owner, frozen catalogue epoch and static mode.
    pub control: &'a ProcessFrameKey,
    /// Actual joined logical invocation operands.
    pub invocation: InvocationView<'a, N>,
    /// Transaction Store upper bound; rollback checkpoints remain interpreter owned.
    pub store: &'a EvmStore<N>,
    /// Captured code, when available under this source identity.
    pub program: Option<&'a Program>,
}

/// Explicit representation of the immutable input, without treating Process
/// adapter anchors as native instruction or operand facts.
#[derive(Clone, Copy, Debug)]
pub enum InputView<'a, N: WordElement> {
    /// A native execution bound with its actual stack and frame data.
    Native(&'a EvmMachinePayload<N>),
    /// A Process control/invocation/Store bound.
    Process(ProcessView<'a, N>),
}
