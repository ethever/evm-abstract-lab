//! Artifact-local semantic identities and typed IR. EVM operands use pop order.

use crate::{Fork, U256};
use serde::{Deserialize, Serialize};

/// Stable bytecode+fork+configuration+schema namespace as full keccak256.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArtifactId(pub String);
/// Stable identity of an entity inside one artifact namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(#[serde(with = "crate::decimal_id")] pub u64);
/// Immutable stage identity inside an artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StageId(pub u32);
/// An entity occurrence in a particular immutable snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityRef {
    /// Snapshot containing the occurrence.
    pub stage: StageId,
    /// Stable artifact-local entity.
    pub entity: EntityId,
}
/// Half-open original bytecode range; synthetic control never invents a PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceSpan {
    /// First original byte offset.
    pub pc: usize,
    /// End offset, clamped to the supplied bytecode for truncated PUSH.
    pub end_pc: usize,
}
/// Offline input; adapters resolve addresses before entering the library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Ordinary EVM runtime bytes.
    pub runtime_bytecode: Vec<u8>,
    /// Explicit supported rules.
    #[serde(deserialize_with = "crate::deserialize_fork")]
    pub fork: Fork,
}
impl Input {
    /// Capture runtime bytecode and its explicit fork.
    pub fn new(runtime_bytecode: Vec<u8>, fork: Fork) -> Self {
        Self {
            runtime_bytecode,
            fork,
        }
    }
}
/// Treatment of known irreducible control flow, independently of recovery gaps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Lower residual control to an explicit executable state dispatcher.
    #[default]
    Dispatcher,
    /// Split selected nodes and remap operations, values and edges first.
    CodeDuplication,
}
/// Frozen limits of a single independent attempt. No limit preallocates storage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Reject larger inputs before decode.
    pub max_bytecode_bytes: usize,
    /// Maximum decoded blocks.
    pub max_blocks: usize,
    /// Maximum decoded operations, including unreachable instructions.
    pub max_operations: usize,
    /// Maximum allocated word and memory identities.
    pub max_values: usize,
    /// Shared logical-work ceiling for recovery and transforms.
    pub max_work: usize,
    /// Maximum block transfer evaluations.
    pub max_iterations: usize,
    /// Finite numeric candidate capacity; zero is rejected.
    pub max_constant_set: usize,
    /// Maximum real block copies across node splitting.
    pub max_cloned_blocks: usize,
    /// Recursion ceiling for structural region construction.
    pub max_structuring_depth: usize,
    /// Maximum retained stage/entity/provenance occurrences across history.
    pub max_history_entities: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytecode_bytes: 100_000,
            max_blocks: 8_192,
            max_operations: 100_000,
            max_values: 500_000,
            max_work: 100_000_000,
            max_iterations: 100_000,
            max_constant_set: 8,
            max_cloned_blocks: 2_048,
            max_structuring_depth: 128,
            max_history_entities: 2_000_000,
        }
    }
}
/// Frozen per-artifact policy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Irreducible control strategy (dispatcher by default).
    pub strategy: Strategy,
    /// Work and retained-entity ceilings.
    pub limits: Limits,
}
/// A finite upper approximation, with pending bottom separate from unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueFact {
    /// No contribution has been evaluated yet.
    Bottom,
    /// Complete alternatives, never a truncated sample.
    Constants(Vec<U256>),
    /// Unconstrained word.
    Unknown,
}
impl ValueFact {
    /// A proved exact word.
    pub fn singleton(&self) -> Option<U256> {
        match self {
            Self::Constants(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }
}
/// Definition identity; facts never substitute for identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    /// Original PUSH or a validated pure fold.
    Constant(U256),
    /// One result slot of one operation.
    Operation { operation: EntityId, index: usize },
    /// Simultaneously assigned block-entry stack slot, bottom to top.
    Parameter { block: EntityId, index: usize },
    /// Ordered memory snapshot after a writer, or initial zero memory.
    Memory { operation: Option<EntityId> },
    /// Phi-like memory input.
    MemoryParameter { block: EntityId },
    /// Ordered snapshot of all observable primitive effects.
    Effect { operation: Option<EntityId> },
    /// Phi-like incoming effect sequence.
    EffectParameter { block: EntityId },
}
/// One stable definition and its final local numeric upper approximation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    /// Stable definition ID.
    pub id: EntityId,
    /// Definition, independent of current numeric precision.
    pub kind: ValueKind,
    /// Locally proved possibilities.
    pub fact: ValueFact,
}
/// Conservative observable primitive effects. Reads are never silently cached.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effects {
    /// Reads local memory.
    pub memory_read: bool,
    /// May change local memory (unknown ranges conservatively kill facts).
    pub memory_write: bool,
    /// Reads persistent storage.
    pub storage_read: bool,
    /// May change persistent storage.
    pub storage_write: bool,
    /// Reads transaction transient storage.
    pub transient_read: bool,
    /// May change transient storage.
    pub transient_write: bool,
    /// Reads the current return-data buffer.
    pub return_data_read: bool,
    /// Replaces the return-data buffer.
    pub return_data_write: bool,
    /// Reads external state or execution context.
    pub world_read: bool,
    /// May change external state.
    pub world_write: bool,
    /// Executes an external call primitive, including static calls.
    pub call: bool,
    /// Creates a contract.
    pub create: bool,
    /// Emits a log.
    pub log: bool,
    /// Halts this execution.
    pub terminate: bool,
}
impl Effects {
    /// Whether the operation has no observable or state-dependent effects.
    pub fn is_pure(&self) -> bool {
        *self == Self::default()
    }
}
/// An original instruction or a real copied instruction, in execution order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    /// Independent operation identity.
    pub id: EntityId,
    /// Exact original range, absent for synthetic control.
    pub source: Option<SourceSpan>,
    /// EVM opcode.
    pub opcode: u8,
    /// Decoder mnemonic.
    pub name: String,
    /// EVM pop-order operands, after DUP/SWAP aliasing.
    pub inputs: Vec<EntityId>,
    /// Defined word results (DUP/SWAP define no new value).
    pub outputs: Vec<EntityId>,
    /// Explicit effect envelope.
    pub effects: Effects,
    /// Captured memory dependency, including reads.
    pub memory_input: Option<EntityId>,
    /// New memory snapshot after a possible write.
    pub memory_output: Option<EntityId>,
    /// Validated exact pure simplification; original opcode and inputs remain.
    pub replacement: Option<U256>,
    /// Previous ordered observable effect, separate from memory aliasing.
    pub effect_input: Option<EntityId>,
    /// New ordered observable effect when this primitive is state-dependent.
    pub effect_output: Option<EntityId>,
}
/// Kind of represented control-flow edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Fallthrough,
    Jump,
    True,
    False,
}
/// Simultaneous stack and memory assignment along one control-flow edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Stable edge identity.
    pub id: EntityId,
    /// Source block.
    pub from: EntityId,
    /// Destination block.
    pub to: EntityId,
    /// Original decision role, preserved by splitting.
    pub kind: EdgeKind,
    /// Destination parameter arguments, bottom to top.
    pub arguments: Vec<EntityId>,
    /// Destination memory argument.
    pub memory: EntityId,
    /// Final observable effect sequence passed to the destination.
    pub effect: EntityId,
}
/// Observable terminal primitive result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    Stop,
    Return,
    Revert,
    SelfDestruct,
    EndOfCode,
}
/// Explicit semantic transfer, including unknown and exceptional targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Terminator {
    /// Only decoded or interrupted frontiers may remain pending.
    Pending,
    /// Unconditional edge, including a simplified known branch.
    Fallthrough { edge: EntityId },
    /// Target alternatives; invalid targets are exceptional, unresolved is partial.
    Jump {
        target: EntityId,
        edges: Vec<EntityId>,
        unresolved: bool,
        invalid_targets: Vec<U256>,
    },
    /// EVM nonzero conditional jump, with an independent fallthrough.
    Branch {
        condition: EntityId,
        target: EntityId,
        then_edges: Vec<EntityId>,
        else_edge: Option<EntityId>,
        unresolved: bool,
        invalid_targets: Vec<U256>,
    },
    /// Normal or revert termination.
    Exit {
        kind: ExitKind,
        values: Vec<EntityId>,
    },
    /// Exceptional halt, including invalid opcodes and stack faults.
    Trap { reason: String },
}
impl Terminator {
    /// All represented known edges, without hiding unresolved alternatives.
    pub fn edges(&self) -> Vec<EntityId> {
        match self {
            Self::Fallthrough { edge } => vec![*edge],
            Self::Jump { edges, .. } => edges.clone(),
            Self::Branch {
                then_edges,
                else_edge,
                ..
            } => then_edges.iter().copied().chain(*else_edge).collect(),
            _ => Vec::new(),
        }
    }
    /// May execution escape to a target not recovered in the graph?
    pub fn is_partial(&self) -> bool {
        matches!(
            self,
            Self::Pending
                | Self::Jump {
                    unresolved: true,
                    ..
                }
                | Self::Branch {
                    unresolved: true,
                    ..
                }
        )
    }
}
/// Immutable block body with explicit phi-like entry assignments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Independent block identity.
    pub id: EntityId,
    /// Original range; cloned blocks retain the same original ancestry.
    pub source: Option<SourceSpan>,
    /// Stack entry definitions.
    pub parameters: Vec<EntityId>,
    /// Memory entry definition.
    pub memory_parameter: EntityId,
    /// Incoming observable effect sequence.
    pub effect_parameter: EntityId,
    /// Instruction order, including external effects.
    pub operations: Vec<Operation>,
    /// Terminal control transfer.
    pub terminator: Terminator,
}
/// Scope under which recovered numeric facts and predecessor sets hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactScope {
    /// Facts apply only to executions before taking an explicit partial exit.
    #[default]
    RecoveredPaths,
    /// Local recovery has no unresolved predecessor frontier.
    CompleteLocalCfg,
}
/// Semantic graph and optional actual structured representation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ir {
    /// Original input length, including unreachable bytes and PUSH padding boundaries.
    pub bytecode_len: usize,
    /// Explicit qualification of numeric facts in open recovery.
    pub fact_scope: FactScope,
    /// Runtime entry block, absent for empty bytecode.
    pub entry: Option<EntityId>,
    /// Retained instruction bodies.
    pub blocks: Vec<Block>,
    /// Stable word and memory definitions.
    pub values: Vec<Value>,
    /// Known transfers with simultaneous parameter assignments.
    pub edges: Vec<Edge>,
    /// Executable structured lowering, never assembled from display text.
    pub structured: Option<StructuredProgram>,
}
impl Ir {
    /// Find a block by artifact-local identity.
    pub fn block(&self, id: EntityId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }
    /// Find a definition by artifact-local identity.
    pub fn value(&self, id: EntityId) -> Option<&Value> {
        self.values.iter().find(|v| v.id == id)
    }
    /// Find a transfer by artifact-local identity.
    pub fn edge(&self, id: EntityId) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }
}
/// Owned executable structured root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredProgram {
    /// Actual AST root.
    pub root: StructuredNode,
}
/// Every AST node has a separate identity and transformation origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredNode {
    /// Independent syntax entity identity.
    pub id: EntityId,
    /// Typed semantics.
    pub kind: StructuredKind,
}
/// One explicit state-machine case with effectful operation body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchCase {
    /// Source IR block body.
    pub block: EntityId,
    /// Explicit state assignments on known exits from the case.
    pub transitions: Vec<DispatchTransition>,
}
/// A traceable synthetic state assignment derived from an original edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTransition {
    /// Synthetic assignment identity.
    pub id: EntityId,
    /// IR edge including phi arguments.
    pub edge: EntityId,
    /// Assigned next block/state.
    pub target: EntityId,
}
/// Structural control. All primitives remain in referenced operation bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredKind {
    /// Ordered syntax children.
    Sequence(Vec<StructuredNode>),
    /// Execute block operations only; following nodes choose its terminator.
    Block { block: EntityId },
    /// Simultaneously assign destination parameters.
    Transfer { edge: EntityId },
    /// Nonzero word selects the first arm.
    If {
        condition: EntityId,
        then_branch: Box<StructuredNode>,
        else_branch: Box<StructuredNode>,
    },
    /// Header executes before every test. None is an unconditional loop.
    While {
        header: Box<StructuredNode>,
        condition: Option<EntityId>,
        condition_nonzero: bool,
        body: Box<StructuredNode>,
        exit_edge: Option<EntityId>,
    },
    /// Leave the nearest natural loop after an explicit transfer.
    Break,
    /// Begin the next natural-loop iteration.
    Continue,
    /// Evaluate a terminal IR exit, exceptional halt or partial frontier.
    Exit { block: EntityId },
    /// Explicit unresolved indirect escape; never silently falls through.
    PartialExit { block: EntityId },
    /// Target-based local alternatives, distinct from residual CFG dispatch.
    Indirect {
        block: EntityId,
        arms: Vec<IndirectArm>,
        unresolved: bool,
    },
    /// A real state variable loop over only the residual region's cases.
    Dispatcher {
        entry: EntityId,
        cases: Vec<DispatchCase>,
    },
}
/// One known dynamic-target choice with its own structured continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectArm {
    /// Concrete original jump destination PC.
    pub target_pc: usize,
    /// Chosen edge assignment and continuation.
    pub body: StructuredNode,
}

/// Observable recovery/transformation phase, without transport state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Fork-aware decode.
    #[default]
    Decode,
    /// Local CFG/stack/memory convergence.
    Recovery,
    /// Pure numeric simplification.
    Simplify,
    /// Irreducibility analysis and optional splitting.
    ControlFlow,
    /// Natural regions and residual state-machine lowering.
    Structure,
    /// Candidate invariant checks.
    Validate,
    /// Materializing forward/reverse origins.
    Provenance,
    /// Attempt finished, including partial/cancelled/limited outcomes.
    Complete,
}
/// Monotone high-water counters plus an actual immutable stage boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Actual current phase.
    pub phase: Phase,
    /// Maximum decoded/retained block count observed.
    pub blocks: usize,
    /// Maximum decoded/retained operation count observed.
    pub operations: usize,
    /// Successful logical-work reservation.
    pub work: usize,
    /// Latest validated stage.
    pub stage: Option<StageId>,
}
