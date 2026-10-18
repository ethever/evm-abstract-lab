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
    domain::{AbstractValue, DomainSpec, symbolic::ExprId},
    world::{ByteArray, Entry, Snapshot, Store},
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
/// authority. There is no public constructor or deserialization path.
#[derive(Clone, Debug, Serialize)]
pub struct RootTerminalInputBinding {
    pub(in crate::analysis) snapshot: SnapshotId,
    pub(in crate::analysis) revision: FactRevision,
    pub(in crate::analysis) storage_source: ExprId,
    pub(in crate::analysis) input_scope: Option<u64>,
    pub(in crate::analysis) fork: Fork,
    pub(in crate::analysis) entry: Entry,
    pub(in crate::analysis) config: ExecutionConfig,
    pub(in crate::analysis) domain_spec: DomainSpec,
    pub(in crate::analysis) frame: FrameKey,
    pub(in crate::analysis) program: Program,
    pub(in crate::analysis) calldata: ByteArray,
    pub(in crate::analysis) call_value: AbstractValue,
    pub(in crate::analysis) saved_store: Snapshot,
    pub(in crate::analysis) initial_store: Store,
}

impl RootTerminalInputBinding {
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
    pub fn entry(&self) -> &Entry {
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
    pub fn calldata(&self) -> &ByteArray {
        &self.calldata
    }
    /// Actual lifted and scoped original call value.
    pub fn call_value(&self) -> &AbstractValue {
        &self.call_value
    }
    /// Original pre-entry rollback checkpoint, with all immutable source facts.
    pub fn saved_store(&self) -> &Snapshot {
        &self.saved_store
    }
    /// Original entered Store before any detailed block or callback executed.
    pub fn initial_store(&self) -> &Store {
        &self.initial_store
    }
}

/// One complete terminal data/Store tuple, without a synthetic machine state.
#[derive(Clone, Debug, Serialize)]
pub struct RootTerminalOutcome {
    pub(in crate::analysis) kind: OutcomeKind,
    pub(in crate::analysis) data: ByteArray,
    pub(in crate::analysis) store: Store,
}

impl RootTerminalOutcome {
    /// Return, Revert or Failure.
    pub fn kind(&self) -> OutcomeKind {
        self.kind
    }
    /// Complete arbitrary return/revert bytes; Failure is exactly empty.
    pub fn data(&self) -> &ByteArray {
        &self.data
    }
    /// Actual transaction-finalized effect bound for this alternative.
    pub fn store(&self) -> &Store {
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
pub struct RootTerminalCoverage {
    pub(in crate::analysis) scope: RootTerminalScope,
    // Preparation allocates these once; publication only moves their ownership.
    // Keep the captured Stores out of every graph/result stack frame.
    pub(in crate::analysis) input: Box<RootTerminalInputBinding>,
    pub(in crate::analysis) outcomes: Box<[RootTerminalOutcome; 3]>,
    pub(in crate::analysis) trigger: RootTerminalTrigger,
}

impl RootTerminalCoverage {
    /// Whole original input-state scope of the separate terminal relation.
    pub fn scope(&self) -> RootTerminalScope {
        self.scope
    }
    /// Exact owned original assumptions shared by every alternative.
    pub fn input(&self) -> &RootTerminalInputBinding {
        &self.input
    }
    /// Fixed order: Return, Revert, Failure.
    pub fn outcomes(&self) -> &[RootTerminalOutcome; 3] {
        &self.outcomes
    }
    /// Actual admissible detailed boundary at publication.
    pub fn trigger(&self) -> &RootTerminalTrigger {
        &self.trigger
    }
}
