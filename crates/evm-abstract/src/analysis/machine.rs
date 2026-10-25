//! Transaction-wide abstract states. A suspended caller is part of a state, not
//! a separate analysis whose result is reused for unrelated call inputs.

use crate::domain::WordElement;

use super::{Config, ConfigError, Diagnostic, EdgeKind, Limit, Status};
use crate::{
    bytecode::Program,
    domain::EvmValue,
    world::{AddressInput, EvmByteArray, EvmEntry, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, B256, U256};
use serde::{Serialize, Serializer};
use std::sync::Arc;

mod evidence;
#[cfg(test)]
pub(in crate::analysis) mod fixtures;
mod frame;
mod join;
mod payload;
mod procedure;
mod process_key;
mod range;
mod stack;
mod symbolic;

pub use evidence::{EvmExecutionEvidence, EvmInstructionValue, InstructionProgress};
#[cfg(test)]
pub(in crate::analysis) use frame::{ChildFrame, RootFrame};
pub use frame::{EvmChildFrame, EvmFrame, EvmFrameState, EvmRootFrame};
pub(in crate::analysis) use frame::{FrameByteKind, FrameMapOrder};
pub(crate) use join::EvmEntryAlternatives;
pub use join::{JoinBoundary, MachineError};
pub use procedure::{
    EvmProcedureModel, EvmProcedureModelData, EvmProcedureModelOutput, EvmProcedureModels,
    EvmProcedureReport, EvmProcedureSummary, EvmProcedureSummaryOutput, EvmProcedureSummaryRecord,
    EvmRegionalEvidence, ProcedureEvidenceVersion, ProcedureModelVersion, ProcedureReportOutcome,
    ProcedureSummaryKind,
};
pub use range::{ByteRangeBoundary, EvmRangeRequest, EvmRangeWitness, RangeKind};
#[cfg(test)]
pub(in crate::analysis) use stack::CallStack;
pub use stack::EvmCallStack;

/// One shared budget covers the root and every callee.
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionConfig {
    /// Finite value-domain and global state/transfer budgets.
    pub analysis: Config,
    /// Maximum shared instruction, candidate, domain-combination and join work.
    pub max_work: usize,
    /// Maximum unfolded active/suspended frames, including entry, before recursive folding.
    pub max_call_depth: usize,
    /// Exact-byte extraction cap for checked consumers, not a logical-memory
    /// bound or a process-wide allocation limit.
    pub max_memory_bytes: usize,
    /// Reuse complete callee graph certificates and procedure relations between
    /// outer entries. Recursive closure remains enabled either way.
    pub use_summaries: bool,
    /// Prepare a separate conservative root terminal relation before detail (enabled by default).
    /// Detailed status, graph, frontiers and observed outcomes remain independent.
    pub root_terminal_coverage: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            analysis: Config::default(),
            max_work: 20_000_000,
            max_call_depth: 32,
            max_memory_bytes: 65_536,
            use_summaries: true,
            root_terminal_coverage: true,
        }
    }
}

impl ExecutionConfig {
    /// Validate execution budgets without constructing a numeric algebra.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.analysis.check()?;
        if self.max_work == 0 || self.max_call_depth == 0 || self.max_memory_bytes == 0 {
            return Err(ConfigError::Budget);
        }
        Ok(())
    }
}

/// Code identity and storage identity differ for DELEGATECALL, CALLCODE and 7702.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameKey {
    /// A process has its own finite coordinates. Other fields are then private
    /// canonical adapter anchors and are omitted by native serialization.
    pub process: Option<Box<super::ProcessFrameKey>>,
    /// Account supplying executed runtime bytecode after delegation resolution.
    pub code_address: Address,
    /// Hash of captured executable code, including creation initcode.
    pub code_hash: B256,
    /// Runtime, initcode, empty or native execution have distinct identities.
    pub mode: FrameCode,
    /// ADDRESS value and persistent/transient storage owner.
    pub address: Address,
    /// Logical ADDRESS, independently of an internal state-owner namespace.
    pub address_value: AddressInput,
    /// CALLER value for this frame, with concrete or symbolic identity.
    pub caller: AddressInput,
    /// Whether state-changing instructions exceptionally fail in this frame.
    pub is_static: bool,
    /// Index into the captured program's basic blocks, not a bytecode PC or chain block.
    /// The block count denotes a synthetic end-of-code continuation.
    pub basic_block_index: usize,
    /// Entry stack height, used to keep differently typed stacks apart.
    pub stack_height: usize,
    /// Bounded intraprocedural jump history, independently retained per call frame.
    pub jump_history: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
/// Structural projection of the complete call stack and executable code.
/// The default policy uses this identity; injected policies own their actual
/// keys and establish a covering representation before grouping inputs.
pub struct MachineKey {
    /// Oldest caller first, active frame last.
    pub frames: Vec<FrameKey>,
    /// Code and account lifecycle variants remain distinct while numeric state joins.
    pub code_identity: B256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Caller destination and requested output range retained while a child executes.
pub struct EvmContinuation<N: WordElement> {
    /// Resume block; calls at end of code resume at a synthetic empty block.
    pub return_block: Option<usize>,
    /// Caller memory destination for the returned prefix.
    pub output_offset: EvmValue<N>,
    /// Requested output length; bytes beyond actual returndata stay unchanged.
    pub output_size: EvmValue<N>,
    /// CREATE/CREATE2 retain a concrete native or class-valued process result.
    pub creation: Option<super::CreationContinuation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
/// Executable interpretation after one account-code resolution.
pub enum FrameCode {
    /// Ordinary EVM runtime bytecode from the resolved code account.
    Runtime,
    /// Initcode executing in a newly created account's frame.
    InitCode,
    /// Native execution selected by the world's fork, independently of code facts.
    Precompile(Address),
    /// Explicit empty code, including a delegated precompile target.
    Empty,
    /// A second [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) marker executes its invalid EF opcode.
    InvalidDelegation,
    /// A conservative process for code or a target remainder not established by observations.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Abstract values for all frames and the shared transaction effects.
pub struct EvmMachinePayload<N: WordElement> {
    /// Nonempty execution stack with one root and typed nested children.
    pub call_stack: EvmCallStack<N>,
    /// Shared transaction effects; reverted frames restore their saved snapshot.
    pub store: EvmStore<N>,
    /// Joint constraints for the scoped values in this transaction state.
    pub relations: crate::domain::relational::RelationState,
}

impl<N: WordElement> EvmMachinePayload<N> {
    pub(crate) fn normalize(&mut self) {
        self.call_stack.forget_identities();
        for frame in self.call_stack.iter_mut() {
            frame.key.stack_height = frame.stack.len();
        }
    }
    /// Compute the structural key after payload stack heights have been normalized.
    pub fn key(&self) -> MachineKey {
        MachineKey {
            code_identity: self.store.code_identity(),
            frames: self
                .call_stack
                .iter()
                .map(|f| {
                    let mut key = f.key.clone();
                    key.stack_height = f.stack.len();
                    key
                })
                .collect(),
        }
    }
    /// Common execution data of the active root or child.
    pub fn active(&self) -> &EvmFrameState<N> {
        self.call_stack.active()
    }
    pub(crate) fn active_mut(&mut self) -> &mut EvmFrameState<N> {
        self.call_stack.active_mut()
    }

    pub(crate) fn work_size(&self) -> usize {
        let store_work = if self
            .call_stack
            .iter()
            .all(|frame| frame.key.process.is_some())
        {
            self.store.process_work_size()
        } else {
            self.store.work_size()
        };
        let cost = self.call_stack.iter().fold(
            store_work.saturating_add(self.relations.work_size()),
            |cost, frame| cost.saturating_add(frame.work_size()),
        );
        self.call_stack.children().iter().fold(cost, |cost, child| {
            cost.saturating_add(child.continuation.output_offset.work_size())
                .saturating_add(child.continuation.output_size.work_size())
        })
    }
}

#[derive(Clone, Debug, Serialize)]
/// A reachable basic-block instance in the transaction-wide worklist.
pub struct EvmMachineState<N: WordElement> {
    /// Stable index in WorldAnalysis::states().
    pub id: usize,
    /// Structural identity used by the worklist.
    pub key: MachineKey,
    /// Joined abstract input payload for this structural state.
    pub entry: EvmMachinePayload<N>,
    /// Common payload after executed instructions and before control dispatch.
    pub exit: Option<EvmMachinePayload<N>>,
    /// Active stack after the last transfer, with deferred call results still absent.
    pub exit_stack: Vec<EvmValue<N>>,
    /// Instruction offsets actually visited in the last transfer.
    pub executed_pcs: Vec<usize>,
    /// Receipt for the latest transfer; stale after its entry changes.
    #[serde(skip)]
    pub(crate) execution_evidence: Option<EvmExecutionEvidence<N>>,
    /// Call result partition retained only at its immediate continuation entry.
    /// It changes scheduling precision, not the public EVM frame identity.
    #[serde(skip)]
    pub(crate) resume_status: Option<bool>,
}

impl<N: WordElement> EvmMachineState<N> {
    /// Phase and edge evidence for partial SSA, if this node was executed.
    pub fn execution_evidence(&self) -> Option<&EvmExecutionEvidence<N>> {
        self.execution_evidence.as_ref()
    }

    pub(crate) fn invalidate_execution(&mut self) {
        if let Some(evidence) = &mut self.execution_evidence {
            evidence.current = false;
        }
    }

    /// Return the active frame identity or payload.
    pub fn active(&self) -> &FrameKey {
        self.key
            .frames
            .last()
            .expect("an analyzed state has a frame")
    }
    /// Captured executable bytecode for this state's active frame.
    pub fn program(&self) -> Option<&Program> {
        self.entry.active().program.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
/// Control transfer within a frame or across an external call boundary.
pub enum MachineEdgeKind {
    /// Jump, branch or fallthrough within the active frame.
    Intraprocedural(EdgeKind),
    /// Caller suspension and child entry; depth grows by exactly one.
    Call,
    /// Successful child completion and caller resumption with result one.
    Return,
    /// Immediate call rejection or exceptional child completion with result zero.
    Failure,
    /// Child REVERT and caller resumption with result zero and revert data.
    Revert,
    /// A procedure relation resumes the parent with this termination category.
    Summary(OutcomeKind),
    /// A coarse region repeats after a local primitive without entering a child.
    Process(super::ProcessAction),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
/// An observed transition between two structural machine states.
pub struct MachineEdge {
    /// Source state, or no source for the initial/pending worklist frontier.
    pub from: usize,
    /// Destination machine-state index.
    pub to: usize,
    /// Control reason or outermost termination category.
    pub kind: MachineEdgeKind,
}

/// Model boundaries are explicit incomplete frontiers, never successful calls
/// whose return value and effects were guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FrontierReason {
    /// An explicit coarse-process semantic rule has not been covered.
    Process(super::ProcessBoundary),
    /// Global state or transfer budget was reached.
    Budget(Limit),
    /// The shared work budget cannot cover the next operation.
    Work,
    /// A certificate lookup, clone or graph import exhausted the shared ledger.
    SummaryWork,
    /// Another frame would exceed the external-call depth budget.
    CallDepth,
    /// A live known bytecode region stabilized without covering finite terminal behavior.
    RecursiveNoTerminal,
    /// A modeled byte range exceeds the declared memory or representation policy.
    Memory,
    /// A checked byte-array range cannot be retained in its declared representation.
    ByteRange(ByteRangeBoundary),
    /// A relational query could not complete under the declared local reasoning policy.
    Relations(crate::domain::relational::QueryReason),
    /// A complete incoming machine alternative could not be merged safely.
    StateJoin(JoinBoundary),
    /// Top call target includes addresses beyond the supplied world.
    UnknownTarget,
    /// The fixed world does not establish this account's executable code.
    MissingCode(Address),
    /// RPC discovery needs this owner's initial slot at the fixed snapshot.
    MissingStorage {
        /// The execution/storage owner, independently of delegated code.
        address: Address,
        /// A concrete member of the complete finite slot-key set.
        slot: U256,
    },
    /// A discovered account could not be acquired at the fixed RPC snapshot.
    RpcAcquisition {
        /// Address requested by the incomplete call or entry.
        address: Address,
        /// Typed acquisition category and fixed-snapshot request context.
        failure: Box<crate::world::rpc::Failure>,
    },
    /// Direct native precompile execution is outside the bytecode model.
    Precompile(Address),
    /// Native precompile input cannot be represented or its work bound is unavailable.
    PrecompileInput(Address),
    /// Creation requires an unobserved or unrepresentable fact.
    Creation(super::transfer::create::CreationBoundary),
    /// Account creation or another explicitly unsupported effect.
    UnsupportedOpcode(u8),
}

#[derive(Clone, Debug, Serialize)]
/// A path left unexpanded with its location and explicit resource or model reason.
pub struct EvmMachineFrontier<N: WordElement> {
    /// Source state, or no source for the initial/pending worklist frontier.
    pub from: Option<usize>,
    /// Unexpanded structural state, when an identity is available.
    pub target: Option<MachineKey>,
    /// Instruction offset where expansion stopped, when applicable.
    pub pc: Option<usize>,
    /// Typed explanation for incomplete coverage.
    pub reason: FrontierReason,
    /// Conditional pre-effect evidence for a range boundary, when applicable.
    pub range: Option<EvmRangeWitness<N>>,
    /// Complete incoming state left outside an entry join. Its immutable roots
    /// keep values, path conditions and rollback snapshots paired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<std::sync::Arc<EvmMachinePayload<N>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
/// How the outermost execution can terminate.
pub enum OutcomeKind {
    /// STOP, RETURN or reaching the end of code commits this frame.
    Return,
    /// REVERT restores this frame's snapshot and exposes revert data.
    Revert,
    /// Exceptional failure restores the snapshot and returns empty data.
    Failure,
}

#[derive(Clone, Debug, Serialize)]
/// A possible outermost completion, including returndata and committed or restored effects.
pub struct EvmMachineOutcome<N: WordElement> {
    /// State producing this possible outermost outcome.
    pub state: usize,
    /// Control reason or outermost termination category.
    pub kind: OutcomeKind,
    /// Full RETURN or REVERT bytes; exceptional failure has empty data.
    pub data: EvmByteArray<N>,
    /// Shared transaction effects; reverted frames restore their saved snapshot.
    pub store: EvmStore<N>,
}

/// The running engine is the sole writer; returning WorldAnalysis ends append.
/// Replay shares retained reports while staging graph changes, never appending,
/// and drops one owner before ordinary execution resumes. These report roots
/// never supply executable procedure inputs or outputs.
#[derive(Debug)]
pub(super) struct EvmProcedureEvidence<N: WordElement>(Arc<Vec<EvmProcedureSummaryRecord<N>>>);

impl<N: WordElement> Clone for EvmProcedureEvidence<N> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<N: WordElement> EvmProcedureEvidence<N> {
    fn as_slice(&self) -> &[EvmProcedureSummaryRecord<N>] {
        self.0.as_slice()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn is_unique(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    /// Only a prepaid final certification may change flags. Replay shares this
    /// Arc and cannot acquire a mutable view or copy the retained Store reports.
    pub(super) fn unique_records(&mut self) -> Option<&mut [EvmProcedureSummaryRecord<N>]> {
        Arc::get_mut(&mut self.0).map(Vec::as_mut_slice)
    }

    pub(super) fn push(&mut self, record: EvmProcedureSummaryRecord<N>) {
        // Replay never appends. Requiring the unique writer prevents an
        // accidental copy-on-write clone of every retained store and buffer.
        Arc::get_mut(&mut self.0)
            .expect("procedure evidence is appended outside replay staging")
            .push(record);
    }
}

impl<N: WordElement> Serialize for EvmProcedureEvidence<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(serializer)
    }
}

#[derive(Clone, Debug, Serialize)]
/// Read-only transaction-wide graph, effects, diagnostics and incomplete frontiers.
pub struct EvmWorldAnalysis<N: WordElement> {
    pub(crate) world: EvmWorld<N>,
    pub(crate) entry: EvmEntry<N>,
    pub(crate) config: ExecutionConfig,
    pub(crate) schema_version: u16,
    pub(crate) domain_spec: crate::domain::DomainSpec,
    pub(crate) snapshot: super::layers::SnapshotId,
    pub(crate) revision: super::layers::FactRevision,
    pub(crate) states: Vec<EvmMachineState<N>>,
    pub(crate) edges: Vec<MachineEdge>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) frontiers: Vec<EvmMachineFrontier<N>>,
    pub(crate) outcomes: Vec<EvmMachineOutcome<N>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) root_terminal_coverage: Option<super::EvmRootTerminalCoverage<N>>,
    pub(crate) status: Status,
    pub(crate) transfers: usize,
    pub(crate) work: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rpc_acquisition: Option<super::RpcAcquisition>,
    pub(crate) summary_stats: super::summary::SummaryStats,
    pub(crate) summaries: Vec<super::summary::EvmSummaryRecord<N>>,
    pub(super) procedure_summaries: EvmProcedureEvidence<N>,
}

impl<N: WordElement> EvmWorldAnalysis<N> {
    /// Original request snapshot, also available for custom carriers/extensions.
    pub fn snapshot(&self) -> &super::layers::SnapshotId {
        &self.snapshot
    }
    /// Exact initial-fact version consumed by this result.
    pub fn revision(&self) -> super::layers::FactRevision {
        self.revision
    }

    /// Separately prepaid terminal relation for the original entered root.
    pub fn root_terminal_coverage(&self) -> Option<&super::EvmRootTerminalCoverage<N>> {
        self.root_terminal_coverage.as_ref()
    }
    /// Cumulative discovery evidence when this result was acquired through RPC.
    pub fn rpc_acquisition(&self) -> Option<&super::RpcAcquisition> {
        self.rpc_acquisition.as_ref()
    }
    /// 冻结的域、交换、widening 和成本策略。
    pub fn domain_spec(&self) -> crate::domain::DomainSpec {
        self.domain_spec
    }

    /// Qualified summary hits, misses, publications and import work.
    pub fn summary_stats(&self) -> &super::summary::SummaryStats {
        &self.summary_stats
    }
    /// Complete callee certificates and observed reuse sites.
    pub fn summaries(&self) -> &[super::summary::EvmSummaryRecord<N>] {
        &self.summaries
    }
    /// Whole-state closure evidence for the unknown/known procedure worklist.
    pub fn procedure_summaries(&self) -> &[EvmProcedureSummaryRecord<N>] {
        self.procedure_summaries.as_slice()
    }
    /// Fixed input world, including fork and snapshot provenance.
    pub fn world(&self) -> &EvmWorld<N> {
        &self.world
    }
    /// Supplied outermost execution context.
    pub fn entry(&self) -> &EvmEntry<N> {
        &self.entry
    }
    /// Shared precision and resource parameters.
    pub fn config(&self) -> &ExecutionConfig {
        &self.config
    }
    /// Reachable context-sensitive machine states.
    pub fn states(&self) -> &[EvmMachineState<N>] {
        &self.states
    }
    /// Observed intraprocedural, call and return transitions.
    pub fn edges(&self) -> &[MachineEdge] {
        &self.edges
    }
    /// Opcode faults and abstract-value precision diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// Paths not fully expanded; a nonempty list makes the result incomplete.
    pub fn frontiers(&self) -> &[EvmMachineFrontier<N>] {
        &self.frontiers
    }
    /// Possible root returns, reverts and exceptional failures.
    pub fn outcomes(&self) -> &[EvmMachineOutcome<N>] {
        &self.outcomes
    }
    /// Whether the worklist closed with no model or resource frontier.
    pub fn status(&self) -> Status {
        self.status
    }
    /// Executed block transfers, including re-execution after joins.
    pub fn transfers(&self) -> usize {
        self.transfers
    }
    /// Consumed shared work units; never exceeds max_work.
    pub fn work(&self) -> usize {
        self.work
    }
}

// Local regression fixtures use explicit built-in carriers.
#[cfg(test)]
pub(in crate::analysis) type Continuation = EvmContinuation<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type MachinePayload = EvmMachinePayload<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type MachineOutcome = EvmMachineOutcome<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type ProcedureSummaryRecord =
    EvmProcedureSummaryRecord<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type WorldAnalysis = EvmWorldAnalysis<crate::domain::NumericValue>;
impl<N: WordElement> Default for EvmProcedureEvidence<N> {
    fn default() -> Self {
        Self(Arc::new(Vec::new()))
    }
}
