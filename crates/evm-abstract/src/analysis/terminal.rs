//! Separate terminal relation for one original root input valuation.
//!
//! Every alternative shares the binding's whole input assignment and saved
//! checkpoint. This record covers terminal tuples; it supplies no machine
//! states, instruction receipts, CFG edges or convergence evidence. The engine
//! alone captures the original entered root and publishes a fully paid record.

use super::{
    ExecutionConfig, FrameKey, OutcomeKind,
    layers::{FactRevision, SnapshotId},
};
use crate::{
    Fork,
    bytecode::Program,
    domain::{DomainSpec, EvmValue, NumericValue, WordElement, symbolic::ExprId},
    world::{EvmByteArray, EvmEntry, EvmSnapshot, EvmStore},
};
use serde::Serialize;

/// The relation ranges over every state satisfying the exact original inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RootTerminalScope {
    /// One whole assignment to the captured environment, saved and current roots.
    AllInputStates,
}

/// A mandatory finite representation failure, captured before public flattening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RootTerminalRetention {
    /// A required complete scalar expression exceeded its finite retention bound.
    Expression,
    /// A required complete immutable state exceeded its finite retention bound.
    State,
}

/// Actual detailed-analysis boundary which activated a prepared root relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum RootTerminalTrigger {
    /// The original shared work ledger refused mandatory detailed work.
    Work,
    /// The original ledger refused a mandatory summary operation.
    SummaryWork,
    /// The actual detailed state counter reached its configured limit.
    States,
    /// The actual detailed transfer counter reached its configured limit.
    Transfers,
    /// A raw finite mandatory retention refusal after existing local retries.
    MandatoryRetention {
        /// Native detailed state containing the interrupted instruction.
        state: usize,
        /// Actual instruction PC, never a fabricated terminal location.
        pc: usize,
        /// The original typed failure category.
        kind: RootTerminalRetention,
    },
}

/// Owned immutable assumptions for all three joint terminal alternatives.
///
/// The original saved and entered Stores preserve source/history identity even
/// when the Return envelope has independent current effects. Captured executing
/// code is independent of any later account-code choice. Snapshot/revision are
/// the actual session identities; no digest or late mutable state grants entry
/// authority. Numeric elements remain native in the captured input, rollback
/// checkpoint and terminal effects. There is no report projection during
/// execution, public constructor or deserialization path.
#[derive(Clone, Debug, Serialize)]
#[serde(rename = "RootTerminalInputBinding")]
pub struct EvmRootTerminalInputBinding<N: WordElement> {
    pub(in crate::analysis) snapshot: SnapshotId,
    pub(in crate::analysis) revision: FactRevision,
    pub(in crate::analysis) storage_source: ExprId,
    pub(in crate::analysis) input_scope: Option<u64>,
    pub(in crate::analysis) fork: Fork,
    pub(in crate::analysis) entry: EvmEntry<N>,
    pub(in crate::analysis) config: ExecutionConfig,
    pub(in crate::analysis) domain_spec: DomainSpec,
    pub(in crate::analysis) frame: FrameKey,
    pub(in crate::analysis) program: Program,
    pub(in crate::analysis) calldata: EvmByteArray<N>,
    pub(in crate::analysis) call_value: EvmValue<N>,
    pub(in crate::analysis) saved_store: EvmSnapshot<N>,
    pub(in crate::analysis) initial_store: EvmStore<N>,
}

impl<N: WordElement> EvmRootTerminalInputBinding<N> {
    /// Exact immutable session snapshot, including offline namespace identity.
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }
    /// Immutable initial-fact revision used for this admission.
    pub fn revision(&self) -> FactRevision {
        self.revision
    }
    /// Persistent input source namespace retained by the original checkpoint.
    pub fn storage_source(&self) -> &ExprId {
        &self.storage_source
    }
    /// Original symbolic transaction-input namespace, when available.
    pub fn input_scope(&self) -> Option<u64> {
        self.input_scope
    }
    /// Fork used to decode the captured executing program and finalize effects.
    pub fn fork(&self) -> Fork {
        self.fork
    }
    /// Original effective request, including snapshot-derived environment facts.
    pub fn entry(&self) -> &EvmEntry<N> {
        &self.entry
    }
    /// Original complete execution policy.
    pub fn config(&self) -> &ExecutionConfig {
        &self.config
    }
    /// Validated domain policy used by the real original entry lift.
    pub fn domain_spec(&self) -> DomainSpec {
        self.domain_spec
    }
    /// Captured physical/logical owner, caller, static mode and code identity.
    pub fn frame(&self) -> &FrameKey {
        &self.frame
    }
    /// Original executing code, retained independently of current account code.
    pub fn program(&self) -> &Program {
        &self.program
    }
    /// Actual lifted and scoped original calldata.
    pub fn calldata(&self) -> &EvmByteArray<N> {
        &self.calldata
    }
    /// Actual lifted and scoped original call value.
    pub fn call_value(&self) -> &EvmValue<N> {
        &self.call_value
    }
    /// Original pre-entry rollback checkpoint, with all immutable source facts.
    pub fn saved_store(&self) -> &EvmSnapshot<N> {
        &self.saved_store
    }
    /// Original entered Store before any detailed block or callback executed.
    pub fn initial_store(&self) -> &EvmStore<N> {
        &self.initial_store
    }
}

/// One complete terminal data/Store tuple, without a synthetic machine state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename = "RootTerminalOutcome")]
pub struct EvmRootTerminalOutcome<N: WordElement> {
    pub(in crate::analysis) kind: OutcomeKind,
    pub(in crate::analysis) data: EvmByteArray<N>,
    pub(in crate::analysis) store: EvmStore<N>,
}

impl<N: WordElement> EvmRootTerminalOutcome<N> {
    /// Return, Revert or Failure.
    pub fn kind(&self) -> OutcomeKind {
        self.kind
    }
    /// Complete arbitrary return/revert bytes; Failure is exactly empty.
    pub fn data(&self) -> &EvmByteArray<N> {
        &self.data
    }
    /// Actual transaction-finalized effect bound for this alternative.
    pub fn store(&self) -> &EvmStore<N> {
        &self.store
    }
}

/// A conservative terminal relation, separate from all detailed graph evidence.
///
/// Exactly three finalized alternatives share one original input valuation.
/// Construction, retained lifetime and single attachment are prepaid before
/// detail starts. This representation is not a whole-Rust/EVM verification
/// claim; the semantic proof remains scoped to the implemented operator.
#[derive(Clone, Debug, Serialize)]
#[serde(rename = "RootTerminalCoverage")]
pub struct EvmRootTerminalCoverage<N: WordElement> {
    pub(in crate::analysis) scope: RootTerminalScope,
    // Preparation allocates these once; publication only moves their ownership.
    // Keep the captured Stores out of every graph/result stack frame.
    pub(in crate::analysis) input: Box<EvmRootTerminalInputBinding<N>>,
    pub(in crate::analysis) outcomes: Box<[EvmRootTerminalOutcome<N>; 3]>,
    pub(in crate::analysis) trigger: RootTerminalTrigger,
}

impl<N: WordElement> EvmRootTerminalCoverage<N> {
    /// Whole original input-state scope of the separate terminal relation.
    pub fn scope(&self) -> RootTerminalScope {
        self.scope
    }
    /// Exact owned original assumptions shared by every alternative.
    pub fn input(&self) -> &EvmRootTerminalInputBinding<N> {
        &self.input
    }
    /// Fixed order: Return, Revert, Failure.
    pub fn outcomes(&self) -> &[EvmRootTerminalOutcome<N>; 3] {
        &self.outcomes
    }
    /// Actual admissible detailed boundary at publication.
    pub fn trigger(&self) -> &RootTerminalTrigger {
        &self.trigger
    }
}

/// Root input binding for the default numeric assembly.
pub type RootTerminalInputBinding = EvmRootTerminalInputBinding<NumericValue>;
/// Terminal tuple for the default numeric assembly.
pub type RootTerminalOutcome = EvmRootTerminalOutcome<NumericValue>;
/// Separate root terminal relation for the default numeric assembly.
pub type RootTerminalCoverage = EvmRootTerminalCoverage<NumericValue>;
