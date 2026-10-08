//! Transaction-wide abstract states. A suspended caller is part of a state, not
//! a separate analysis whose result is reused for unrelated call inputs.

use super::{Config, ConfigError, Diagnostic, EdgeKind, Limit, Status};
use crate::{
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{AddressInput, ByteArray, Entry, Store, World},
};
use alloy_primitives::{Address, B256, U256};
use serde::Serialize;

mod evidence;
mod frame;
mod stack;
mod symbolic;

pub use evidence::{ExecutionEvidence, InstructionProgress};
pub use frame::{ChildFrame, FrameState, RootFrame};
pub use stack::CallStack;

/// One shared budget covers the root and every callee.
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionConfig {
    /// Finite value-domain and global state/transfer budgets.
    pub analysis: Config,
    /// Maximum shared instruction, candidate, domain-combination and join work.
    pub max_work: usize,
    /// Maximum number of simultaneously active and suspended frames, including entry.
    pub max_call_depth: usize,
    /// Maximum modeled memory footprint and extracted byte-array length.
    pub max_memory_bytes: usize,
    /// Reuse only complete input-qualified callee graph certificates.
    pub use_summaries: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            analysis: Config::default(),
            max_work: 20_000_000,
            max_call_depth: 32,
            max_memory_bytes: 65_536,
            use_summaries: true,
        }
    }
}

impl ExecutionConfig {
    pub(crate) fn domain(&self) -> Result<Domain, ConfigError> {
        let (_, domain) = self.analysis.clone().validate()?.into_parts();
        if self.max_work == 0 || self.max_call_depth == 0 || self.max_memory_bytes == 0 {
            return Err(ConfigError::Budget);
        }
        Ok(domain)
    }
}

/// Code identity and storage identity differ for DELEGATECALL, CALLCODE and 7702.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FrameKey {
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
/// Structural identity of the entire call stack; payload values join only within this identity.
pub struct MachineKey {
    /// Oldest caller first, active frame last.
    pub frames: Vec<FrameKey>,
    /// Code and account lifecycle variants remain distinct while numeric state joins.
    pub code_identity: B256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Caller destination and requested output range retained while a child executes.
pub struct Continuation {
    /// Resume block; calls at end of code resume at a synthetic empty block.
    pub return_block: Option<usize>,
    /// Caller memory destination for the returned prefix.
    pub output_offset: AbstractValue,
    /// Requested output length; bytes beyond actual returndata stay unchanged.
    pub output_size: AbstractValue,
    /// CREATE/CREATE2 deploy at this address and resume with an address result.
    pub creation: Option<Address>,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Abstract values for all frames and the shared transaction effects.
pub struct MachinePayload {
    /// Nonempty execution stack with one root and typed nested children.
    pub call_stack: CallStack,
    /// Shared transaction effects; reverted frames restore their saved snapshot.
    pub store: Store,
    /// Joint constraints for the scoped values in this transaction state.
    pub relations: crate::domain::relational::RelationState,
}

impl MachinePayload {
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
    pub fn active(&self) -> &FrameState {
        self.call_stack.active()
    }
    pub(crate) fn active_mut(&mut self) -> &mut FrameState {
        self.call_stack.active_mut()
    }

    pub(crate) fn work_size(&self) -> usize {
        self.call_stack.iter().fold(
            self.store
                .work_size()
                .saturating_add(self.relations.work_size()),
            |cost, frame| {
                let slots = frame
                    .stack
                    .iter()
                    .fold(0usize, |cost, value| cost.saturating_add(value.work_size()));
                cost.saturating_add(slots)
                    // 估算载荷复制/比较的工作量时，也计入实际保留的跳转历史。
                    .saturating_add(frame.key.jump_history.len())
                    .saturating_add(frame.program.as_ref().map_or(0, |p| {
                        p.byte_len().saturating_add(
                            p.blocks()
                                .iter()
                                .map(|b| b.instructions.len())
                                .sum::<usize>(),
                        )
                    }))
                    .saturating_add(frame.memory.work_size())
                    .saturating_add(frame.calldata.work_size())
                    .saturating_add(frame.returndata.work_size())
                    .saturating_add(frame.saved_store.state().work_size())
                    .saturating_add(frame.call_value.work_size())
                    .saturating_add(3)
            },
        )
    }

    pub(crate) fn widen(&mut self, old: &Self, domain: Domain) {
        self.store.widen(&old.store, domain);
        self.call_stack.widen(&old.call_stack, domain);
        self.relations = self.relations.join(&old.relations);
    }
    pub(crate) fn join(&self, other: &Self, domain: Domain) -> Self {
        assert_eq!(
            self.key(),
            other.key(),
            "payload join requires equal structural keys"
        );
        let mut result = self.clone();
        result.store = self.store.join(&other.store, domain);
        result.call_stack.join(&other.call_stack, domain);
        result.relations = self.relations.join(&other.relations);
        result
    }
}

#[derive(Clone, Debug, Serialize)]
/// A reachable basic-block instance in the transaction-wide worklist.
pub struct MachineState {
    /// Stable index in WorldAnalysis::states().
    pub id: usize,
    /// Structural identity used by the worklist.
    pub key: MachineKey,
    /// Joined abstract input payload for this structural state.
    pub entry: MachinePayload,
    /// Common payload after executed instructions and before control dispatch.
    pub exit: Option<MachinePayload>,
    /// Active stack after the last transfer, with deferred call results still absent.
    pub exit_stack: Vec<AbstractValue>,
    /// Instruction offsets actually visited in the last transfer.
    pub executed_pcs: Vec<usize>,
    /// Receipt for the latest transfer; stale after its entry changes.
    #[serde(skip)]
    pub(crate) execution_evidence: Option<ExecutionEvidence>,
}

impl MachineState {
    /// Phase and edge evidence for partial SSA, if this node was executed.
    pub fn execution_evidence(&self) -> Option<&ExecutionEvidence> {
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
    /// Global state or transfer budget was reached.
    Budget(Limit),
    /// The shared work budget cannot cover the next operation.
    Work,
    /// A certificate lookup, clone or graph import exhausted the shared ledger.
    SummaryWork,
    /// Another frame would exceed the external-call depth budget.
    CallDepth,
    /// A modeled byte range exceeds the cap or has unbounded size.
    Memory,
    /// A relational query could not finish within the declared symbolic/SMT policy.
    Relations(crate::domain::relational::QueryReason),
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
pub struct MachineFrontier {
    /// Source state, or no source for the initial/pending worklist frontier.
    pub from: Option<usize>,
    /// Unexpanded structural state, when an identity is available.
    pub target: Option<MachineKey>,
    /// Instruction offset where expansion stopped, when applicable.
    pub pc: Option<usize>,
    /// Typed explanation for incomplete coverage.
    pub reason: FrontierReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
pub struct MachineOutcome {
    /// State producing this possible outermost outcome.
    pub state: usize,
    /// Control reason or outermost termination category.
    pub kind: OutcomeKind,
    /// Full RETURN or REVERT bytes; exceptional failure has empty data.
    pub data: ByteArray,
    /// Shared transaction effects; reverted frames restore their saved snapshot.
    pub store: Store,
}

#[derive(Clone, Debug, Serialize)]
/// Read-only transaction-wide graph, effects, diagnostics and incomplete frontiers.
pub struct WorldAnalysis {
    pub(crate) world: World,
    pub(crate) entry: Entry,
    pub(crate) config: ExecutionConfig,
    pub(crate) schema_version: u16,
    pub(crate) domain_spec: crate::domain::DomainSpec,
    pub(crate) states: Vec<MachineState>,
    pub(crate) edges: Vec<MachineEdge>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) frontiers: Vec<MachineFrontier>,
    pub(crate) outcomes: Vec<MachineOutcome>,
    pub(crate) status: Status,
    pub(crate) transfers: usize,
    pub(crate) work: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rpc_acquisition: Option<super::RpcAcquisition>,
    pub(crate) summary_stats: super::summary::SummaryStats,
    pub(crate) summaries: Vec<super::summary::SummaryRecord>,
}

impl WorldAnalysis {
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
    pub fn summaries(&self) -> &[super::summary::SummaryRecord] {
        &self.summaries
    }
    /// Fixed input world, including fork and snapshot provenance.
    pub fn world(&self) -> &World {
        &self.world
    }
    /// Supplied outermost execution context.
    pub fn entry(&self) -> &Entry {
        &self.entry
    }
    /// Shared precision and resource parameters.
    pub fn config(&self) -> &ExecutionConfig {
        &self.config
    }
    /// Reachable context-sensitive machine states.
    pub fn states(&self) -> &[MachineState] {
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
    pub fn frontiers(&self) -> &[MachineFrontier] {
        &self.frontiers
    }
    /// Possible root returns, reverts and exceptional failures.
    pub fn outcomes(&self) -> &[MachineOutcome] {
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
