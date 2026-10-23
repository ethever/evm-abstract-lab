//! Native generic EVM execution over validated immutable root requests.
//!
//! [`AnalysisEngine`] borrows one numeric algebra, frozen execution policies and
//! an optional static analyzer assembly. [`AnalysisInput`] retains the original
//! snapshot/source identity for every carrier. [`EvmAnalysis`] projects an owned
//! native result into a local CFG without starting another execution.

mod composition;
mod config;
pub mod control;
pub mod cooperation;
mod engine;
mod input;
pub mod layers;
mod machine;
pub mod process;
pub mod progress;
mod rpc;
mod single;
mod state;
mod summary;
mod terminal;
mod transfer;
mod worklist;

#[cfg(test)]
mod composition_tests;
#[cfg(test)]
mod testing;

use crate::{
    bytecode::Program,
    domain::{EvmValue, WordElement},
};
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::fmt;

pub use crate::bytecode::InvalidOpcodeReason;
pub use composition::AnalysisEngine;
pub use config::{Config, ConfigError, ValidatedConfig};
pub use engine::{
    Acceleration, AccelerationInput, AccelerationPolicy, Arrival, BoundedJoin, CoarseSummaries,
    ContextInsensitive, ContextPolicy, CorrelatedJoin, EagerAcceleration, EnginePolicies,
    FeedbackAcceleration, FirstResumeStatus, ForgetJumpHistory, KnownProcessSummaries,
    MachinePartition, MergeResumeStatus, NoAcceleration, PartitionPolicy, PartitionRefusal,
    PartitionResult, Partitioned, PartitionedRelative, PolicyContext, PolicyKey, PolicySite,
    ProcedureOutput, ProcedureSummaryPolicy, ProjectedSummaries, QuotedKey, RecentDistinctJumps,
    RegionalSummaries, RelativePartitionResult, RelativeState, SemanticPolicies, StatePolicy,
    StructuralPartition, WholeStateMetadata, WholeStatePolicy,
};
pub use input::AnalysisInput;
pub use machine::{
    ByteRangeBoundary, ExecutionConfig, FrameCode, FrameKey, FrontierReason, InstructionProgress,
    JoinBoundary, MachineEdge, MachineEdgeKind, MachineError, MachineKey, OutcomeKind,
    ProcedureSummaryKind, RangeKind,
};
#[cfg(test)]
use machine::{
    CallStack, ChildFrame, Continuation, MachineOutcome, MachinePayload, ProcedureSummaryRecord,
    RootFrame, WorldAnalysis,
};
pub use machine::{
    EvmCallStack, EvmChildFrame, EvmContinuation, EvmExecutionEvidence, EvmFrame, EvmFrameState,
    EvmInstructionValue, EvmMachineFrontier, EvmMachineOutcome, EvmMachinePayload, EvmMachineState,
    EvmProcedureSummary, EvmProcedureSummaryOutput, EvmProcedureSummaryRecord, EvmRangeRequest,
    EvmRangeWitness, EvmRootFrame, EvmWorldAnalysis,
};
pub use process::{
    CreationContinuation, EvmProcessInvocation, EvmProcessTransfer, ProcessAction, ProcessBoundary,
    ProcessControlScope, ProcessCreation, ProcessCreationTransfer, ProcessFrameKey, ProcessOwner,
    ProcessSource, RuntimeExecution, TransferSemantics,
};
pub use rpc::{
    RpcAccountFailure, RpcAcquisition, RpcAnalysis, RpcAnalysisError, RpcStorageSlot, analyze_rpc,
};
pub use single::ProjectionError;
pub use summary::{EvmSummaryInput, EvmSummaryOutput, EvmSummaryRecord, SummaryStats};
pub use terminal::{
    EvmRootTerminalCoverage, EvmRootTerminalInputBinding, EvmRootTerminalOutcome,
    RootTerminalRetention, RootTerminalScope, RootTerminalTrigger,
};
pub use transfer::create::CreationBoundary;
pub use transfer::procedure::process::creation as process_creation;
pub use transfer::procedure::{
    EvmRegionalCall, EvmRegionalCfg, EvmRegionalEdge, EvmRegionalNode, EvmRegionalReturn,
    EvmRegionalTransfer,
};

/// Serialized EVM analysis format. Version 14 adds explicit process semantics,
/// coarse action receipts and typed unresolved process obligations.
/// Version 13 adds compact regional CFG receipts
/// with native revisions, separate call continuations and explicit source validity.
/// Version 11 distinguishes reduction work
/// exhaustion, cancellation and kernel failure from the local round limit.
/// Version 10 introduced optional root terminal coverage with owned original
/// input binding and finalized tuples. The domain-policy schema is independent.
pub const SCHEMA_VERSION: u16 = 14;

/// 同一字节码块在不同抽象上下文中的身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StateKey {
    /// Program.blocks() 中的基本块索引，不是指令 PC 或链上区块号。
    pub basic_block_index: usize,
    /// 入栈高；不同高度不做逐槽 join。
    pub stack_height: usize,
    /// 按先后顺序保存的跳转来源 pc，不是函数名或外部 CALL 栈。
    pub context: Vec<usize>,
}

/// 一个可达的抽象基本块实例。
#[derive(Clone, Debug, Serialize)]
pub struct EvmState<N: WordElement> {
    /// Joint guarantees associated with this state's entry values.
    pub entry_relations: crate::domain::relational::RelationState,
    /// Joint guarantees after the recorded transfer, when it has executed.
    pub exit_relations: Option<crate::domain::relational::RelationState>,
    /// 稳定编号，等于 `Analysis::states()` 中的索引。
    pub id: usize,
    /// 状态的结构身份。
    pub key: StateKey,
    /// 底到顶的抽象栈，最后一个元素是栈顶。
    pub entry_stack: Vec<EvmValue<N>>,
    /// 最后一次 transfer 的出栈。
    pub exit_stack: Vec<EvmValue<N>>,
    /// 最后一次 transfer 实际访问的指令 pc；异常之后的指令不出现。
    pub executed_pcs: Vec<usize>,
}

/// CFG 边的控制原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum EdgeKind {
    /// 相邻块的正常流入。
    Fallthrough,
    /// 无条件 JUMP。
    Jump,
    /// JUMPI 条件可能非零。
    BranchTrue,
    /// JUMPI 条件可能为零。
    BranchFalse,
}

/// 一个抽象状态到另一个抽象状态的边。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Edge {
    /// 来源状态编号。
    pub from: usize,
    /// 目标状态编号。
    pub to: usize,
    /// 控制流原因。
    pub kind: MachineEdgeKind,
}

/// 精度损失或可观测的程序异常；它们本身不表示工作表尚未收敛。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum DiagnosticKind {
    /// 未知目标覆盖所有合法 JUMPDEST，并且可能异常终止。
    UnknownJump,
    /// 有限目标中的一个值不是合法 JUMPDEST。
    InvalidJump,
    /// 栈元素不够，当前路径异常终止。
    StackUnderflow,
    /// 栈超过 EVM 的 1024 槽上限，当前路径异常终止。
    StackOverflow,
    /// 在程序所选 fork 下无效的 opcode，当前路径异常终止。
    InvalidOpcode(InvalidOpcodeDiagnostic),
    /// 内存、状态或环境值仅用保守摘要表示。
    OpaqueResult,
    /// 可选语义交换达到固定精度上限；结果仍覆盖全部具体可能性。
    FactExchangeLimited(crate::domain::ReductionStatus),
    /// A value stayed conservative because an optional relational projection
    /// could not finish; its execution path was still fully propagated.
    RelationalPrecisionLost(crate::domain::relational::QueryReason),
}

/// Original opcode evidence from the frame that attempted exceptional execution.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum InvalidOpcodeDiagnostic {
    /// An instruction in the active frame's captured runtime or initcode program.
    Instruction {
        /// The actual byte at the instruction's PC.
        opcode: u8,
        /// Present only when the opcode has an upstream instruction definition.
        mnemonic: Option<String>,
        /// Fork-specific validity failure from the decoded instruction.
        reason: InvalidOpcodeReason,
    },
    /// A nested delegation entry has no decoded executable program or receipt.
    DelegationEntry {
        /// First byte from the active code account, when it can be verified.
        opcode: Option<u8>,
        /// Explains the account-entry failure independently of ordinary instructions.
        detail: String,
        /// Explains missing byte evidence; present whenever `opcode` is absent.
        byte_unavailable: Option<String>,
    },
}

impl fmt::Display for InvalidOpcodeDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instruction {
                opcode,
                mnemonic,
                reason,
            } => {
                write!(f, "0x{opcode:02x}")?;
                if let Some(mnemonic) = mnemonic {
                    write!(f, " ({mnemonic})")?;
                }
                write!(f, " — {reason}")
            }
            Self::DelegationEntry {
                opcode,
                detail,
                byte_unavailable,
            } => {
                f.write_str("delegation entry")?;
                if let Some(opcode) = opcode {
                    write!(f, " 0x{opcode:02x}")?;
                }
                write!(f, " — {detail}")?;
                if let Some(reason) = byte_unavailable {
                    write!(f, "; opcode byte unavailable: {reason}")?;
                }
                Ok(())
            }
        }
    }
}

/// 稳定、可导出的诊断位置。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Diagnostic {
    /// 抽象状态编号。
    pub state: usize,
    /// 指令 pc。
    pub pc: usize,
    /// 诊断类别。
    pub kind: DiagnosticKind,
}

/// 资源边界，而非“程序没有后续执行”的证明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Limit {
    /// 没有资源创建新状态。
    States,
    /// 工作表尚有未执行的 transfer。
    Transfers,
    /// 累计指令、域运算、合并或候选展开工作量。
    Work,
    /// 外部调用帧深度的分析边界。
    CallDepth,
    /// 抽象字节范围超过允许的资源规模。
    Memory,
    /// 事实缺失或尚未实现的语义；不属于 EVM 程序失败。
    Model,
}

/// 无法继续展开的明确前沿，保留其来源和预期状态键。
#[derive(Clone, Debug, Serialize)]
pub struct Frontier {
    /// Full native reason, including symbolic and relational resource limits.
    pub reason: FrontierReason,
    /// 已知来源状态；最初入口没有来源。
    pub from: Option<usize>,
    /// 尚未完全分析的目标状态。
    pub target: StateKey,
    /// 触发的预算。
    pub limit: Limit,
}

/// 收敛只说明这个抽象模型的工作表完成，不表示精确恢复或安全证明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Status {
    /// 所有状态满足传播闭包。
    Converged,
    /// 有资源或语义/事实尚未覆盖的前沿，SSA 构建会拒绝该结果。
    Incomplete,
}

/// 分析结果不可由外部手工构造；SSA 可以信任状态栈高与边的内部不变量。
#[derive(Clone, Debug)]
pub struct EvmAnalysis<N: WordElement> {
    pub(crate) program: Program,
    pub(crate) config: Config,
    pub(crate) schema_version: u16,
    pub(crate) domain_spec: crate::domain::DomainSpec,
    pub(crate) states: Vec<EvmState<N>>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) boundary_edges: Vec<MachineEdge>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) frontiers: Vec<Frontier>,
    pub(crate) status: Status,
    pub(crate) transfers: usize,
    execution: EvmWorldAnalysis<N>,
    execution_state_ids: Vec<usize>,
}

// Serialize the native environment by reference. The single-program report must
// retain its input assumptions without duplicating the owned execution graph or
// cloning input tables outside the shared work budget.
impl<N: WordElement> Serialize for EvmAnalysis<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut report = serializer.serialize_map(Some(
            15 + usize::from(self.root_terminal_coverage().is_some()),
        ))?;
        report.serialize_entry("program", &self.program)?;
        report.serialize_entry("config", &self.config)?;
        report.serialize_entry("schema_version", &self.schema_version)?;
        report.serialize_entry("domain_spec", &self.domain_spec)?;
        report.serialize_entry("environment", self.environment())?;
        report.serialize_entry("states", &self.states)?;
        report.serialize_entry("edges", &self.edges)?;
        report.serialize_entry("boundary_edges", &self.boundary_edges)?;
        report.serialize_entry("execution_state_ids", &self.execution_state_ids)?;
        report.serialize_entry("edge_endpoint_namespace", "local")?;
        report.serialize_entry("boundary_endpoint_namespace", "native")?;
        report.serialize_entry("diagnostics", &self.diagnostics)?;
        report.serialize_entry("frontiers", &self.frontiers)?;
        report.serialize_entry("status", &self.status)?;
        report.serialize_entry("transfers", &self.transfers)?;
        if let Some(coverage) = self.root_terminal_coverage() {
            report.serialize_entry("root_terminal_coverage", coverage)?;
        }
        report.end()
    }
}

impl<N: WordElement> EvmAnalysis<N> {
    /// Numeric specification recorded by the native execution.
    pub fn domain_spec(&self) -> crate::domain::DomainSpec {
        self.domain_spec
    }
    /// 原始解码结果。
    pub fn program(&self) -> &Program {
        &self.program
    }
    /// 本次分析的参数。
    pub fn config(&self) -> &Config {
        &self.config
    }
    /// 可达的抽象状态。未创建的字节码块不等于已证明不可能执行，须看 status。
    pub fn states(&self) -> &[EvmState<N>] {
        &self.states
    }
    /// 已发现的控制流边。
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }
    /// Native transitions crossing the selected root boundary. These IDs are
    /// native machine IDs; execution_state_id maps the local endpoints.
    pub fn boundary_edges(&self) -> &[MachineEdge] {
        &self.boundary_edges
    }
    /// 精度与程序异常诊断。
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// 资源或语义/事实边界留下的分析前沿。
    pub fn frontiers(&self) -> &[Frontier] {
        &self.frontiers
    }
    /// 工作表完成与否。
    pub fn status(&self) -> Status {
        self.status
    }
    /// 已执行的基本块 transfer 数。
    pub fn transfers(&self) -> usize {
        self.transfers
    }
    /// Immutable root, transaction and block inputs used by the native machine.
    pub fn environment(&self) -> &crate::world::EvmEnvironmentValues<N> {
        &self.execution.entry().environment
    }
    /// Native multi-account result underlying this single-program view.
    pub fn execution(&self) -> &EvmWorldAnalysis<N> {
        &self.execution
    }

    /// Separate terminal coverage for the original root, preserving partial CFG/SSA.
    pub fn root_terminal_coverage(&self) -> Option<&EvmRootTerminalCoverage<N>> {
        self.execution.root_terminal_coverage()
    }

    /// Map a local learning-view state to the underlying native machine state.
    /// Partial SSA uses native IDs, including any child frames absent locally.
    pub fn execution_state_id(&self, local_state: usize) -> Option<usize> {
        self.execution_state_ids.get(local_state).copied()
    }
}
