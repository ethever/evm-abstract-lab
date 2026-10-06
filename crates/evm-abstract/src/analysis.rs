//! 多账户调用图与控制流由同一台抽象机逐步发现。
//!
//! [`analyze_world`] 是完整入口：工作表保存执行与暂停的调用帧，并且让所有
//! 子调用共享事务状态和预算。代码地址与状态地址各自参与帧身份。
//! [`analyze`] 是单账户学习视图，复用同一执行核心后投影为局部 CFG。
//! [`analyze_rpc`] 显式采集固定区块的初始事实，并按缺失 callee 扩充快照后
//! 从入口重建分析；各轮共用执行预算，纯离线入口不会隐式访问网络。
//! 栈高区分是必要的类型边界；有限内部跳转历史是可选的精度选择。
//! 每次新输入都逐槽 join，只有输入变大才重新入队。边也只增加，不随某次
//! 更精确的执行删除。读 `analysis/engine.rs` 中的循环时，始终检查这两个不变量。
//!
//! 配置在 [`Config::validate`] 阶段完成准入检查，转换成不能手工伪造的
//! [`ValidatedConfig`] 后构造引擎使用的域；世界执行的附加预算在同一准入阶段
//! 检查。域容量的非零条件在类型中携带。

mod config;
mod engine;
mod machine;
mod rpc;
mod single;
mod summary;
mod transfer;

use crate::{bytecode::Program, domain::Value};
use serde::{Serialize, Serializer, ser::SerializeMap};

pub use config::{Config, ConfigError, ValidatedConfig};
pub use machine::{
    CallStack, ChildFrame, Continuation, ExecutionConfig, FrameCode, FrameKey, FrameState,
    FrontierReason, MachineEdge, MachineEdgeKind, MachineFrontier, MachineKey, MachineOutcome,
    MachinePayload, MachineState, OutcomeKind, RootFrame, WorldAnalysis,
};
pub use rpc::{
    RpcAccountFailure, RpcAcquisition, RpcAnalysis, RpcAnalysisError, RpcStorageSlot, analyze_rpc,
};
pub use summary::{SummaryInput, SummaryOutput, SummaryRecord, SummaryStats};
pub use transfer::create::CreationBoundary;

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
pub struct State {
    /// 稳定编号，等于 `Analysis::states()` 中的索引。
    pub id: usize,
    /// 状态的结构身份。
    pub key: StateKey,
    /// 底到顶的抽象栈，最后一个元素是栈顶。
    pub entry_stack: Vec<Value>,
    /// 最后一次 transfer 的出栈。
    pub exit_stack: Vec<Value>,
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
    pub kind: EdgeKind,
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
    InvalidOpcode,
    /// 内存、状态或环境值仅用保守摘要表示。
    OpaqueResult,
    /// 可选语义交换达到固定精度上限；结果仍覆盖全部具体可能性。
    FactExchangeLimited(crate::domain::ReductionStatus),
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
pub struct Analysis {
    pub(crate) program: Program,
    pub(crate) config: Config,
    pub(crate) schema_version: u16,
    pub(crate) domain_spec: crate::domain::DomainSpec,
    pub(crate) states: Vec<State>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) frontiers: Vec<Frontier>,
    pub(crate) status: Status,
    pub(crate) transfers: usize,
    execution: WorldAnalysis,
}

// Serialize the native environment by reference. The single-program report must
// retain its input assumptions without duplicating the owned execution graph or
// cloning input tables outside the shared work budget.
impl Serialize for Analysis {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut report = serializer.serialize_map(Some(11))?;
        report.serialize_entry("program", &self.program)?;
        report.serialize_entry("config", &self.config)?;
        report.serialize_entry("schema_version", &self.schema_version)?;
        report.serialize_entry("domain_spec", &self.domain_spec)?;
        report.serialize_entry("environment", self.environment())?;
        report.serialize_entry("states", &self.states)?;
        report.serialize_entry("edges", &self.edges)?;
        report.serialize_entry("diagnostics", &self.diagnostics)?;
        report.serialize_entry("frontiers", &self.frontiers)?;
        report.serialize_entry("status", &self.status)?;
        report.serialize_entry("transfers", &self.transfers)?;
        report.end()
    }
}

impl Analysis {
    /// 原始解码结果。
    pub fn program(&self) -> &Program {
        &self.program
    }
    /// 本次分析的参数。
    pub fn config(&self) -> &Config {
        &self.config
    }
    /// 可达的抽象状态。未创建的字节码块不等于已证明不可能执行，须看 status。
    pub fn states(&self) -> &[State] {
        &self.states
    }
    /// 已发现的控制流边。
    pub fn edges(&self) -> &[Edge] {
        &self.edges
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
    pub fn environment(&self) -> &crate::world::EvmEnvironment {
        &self.execution.entry().environment
    }
    /// Native multi-account result underlying this single-program view.
    pub fn execution(&self) -> &WorldAnalysis {
        &self.execution
    }
}

/// Analyze a declared offline world from one external entry, sharing one
/// worklist and transaction state through all nested calls.
pub fn analyze_world(
    world: crate::world::World,
    entry: crate::world::Entry,
    config: ExecutionConfig,
) -> Result<WorldAnalysis, ConfigError> {
    engine::run_world(world, entry, config)
}

/// Single-bytecode learning adapter. Environment, calldata and initial
/// persistent state remain unknown; missing external code is an explicit
/// incomplete frontier in the same native machine used by [`analyze_world`].
pub fn analyze(program: Program, config: Config) -> Result<Analysis, ConfigError> {
    single::analyze(program, config, crate::world::EvmEnvironment::default())
}

/// Analyze bytecode under explicit or symbolic root, transaction and block inputs.
///
/// A missing logical destination stays symbolic even though the internal store
/// uses an isolated concrete namespace. CALL-family rules preserve logical
/// addresses and caller identity; unknown targets remain incomplete frontiers.
pub fn analyze_with_environment(
    program: Program,
    config: Config,
    environment: crate::world::EvmEnvironment,
) -> Result<Analysis, ConfigError> {
    single::analyze(program, config, environment)
}
