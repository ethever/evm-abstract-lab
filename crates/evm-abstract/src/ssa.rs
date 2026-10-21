//! 把“栈位置”改写成“值的名字”：静态单赋值（SSA）。
//!
//! `DUP` 不创造一个新的运算值，两个槽位指向同一 ValueId；`SWAP` 只交换身份。
//! 原生帧 SSA 分配 φ 和跨调用的 continuation 名字，再显式投影根程序块。
//! 预分配使循环成为有限的引用环，不需要反复生成无限多个名字。
//! 单前驱 φ 也保留，原生状态、边、子帧和效果名字不会在投影时重编号。
//! dominance 算法使用 petgraph，`verify` 检查唯一赋值、支配和 φ 边一致性。

pub mod block;
mod build;
mod error;
pub use error::{SsaInvariant, SsaInvariantKind};
pub mod normalized;
pub use normalized::{
    FactScope, FactSource, FactValidity, NormalizationInvariant, NormalizationInvariantKind,
    NormalizedBlock, NormalizedInstruction, NormalizedOperation, NormalizedSsa, ValueFact,
    ValueMapping, ValueOperand, ValueOrigin, ValueTransformation, build_normalized,
    build_normalized_partial_world, build_normalized_world,
};
#[cfg(test)]
mod native_tests;
mod partial;
#[cfg(test)]
pub(crate) mod test_support;
mod verify;
mod world;

pub use partial::{
    DeferredEdgeReason, DeferredSsaEdge, PartialBlockCoverage, PartialInstruction,
    PartialWorldBlock, PartialWorldSsa, build_partial_world,
};
pub use world::{
    EffectId, EffectInput, EffectPhi, FramePhi, Transition, WorldBlock, WorldSsa, build_world,
};

use crate::analysis::EvmAnalysis;
use crate::domain::WordElement;
use alloy_primitives::U256;
use serde::Serialize;
use thiserror::Error;

/// SSA 中的值名；显示为 `%0`、`%1`……。栈槽位可以同时引用同一个值。
pub type ValueId = usize;

/// φ 的一条边输入；取值由最近经过的前驱确定，不是对数值求并。
#[derive(Clone, Debug, Serialize)]
pub struct PhiInput {
    /// Original native transition ID; parallel and returning edges remain distinct.
    pub edge: usize,
    /// Native predecessor state ID, including child states outside the root view.
    pub predecessor: usize,
    /// 前驱出栈对应槽位的 SSA 名字。
    pub value: ValueId,
}

/// 基本块入栈的一个参数，用 φ 语法显示。
#[derive(Clone, Debug, Serialize)]
pub struct Phi {
    /// 在此唯一定义的名字。
    pub result: ValueId,
    /// 底到顶的入栈槽位编号。
    pub slot: usize,
    /// 每个不同前驱恰好一条输入。
    pub inputs: Vec<PhiInput>,
}

/// 保留每条已执行 opcode 的定义和使用，包含零输出的副作用指令。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Instruction {
    /// 原始指令 pc。
    pub pc: usize,
    /// 原始 opcode 字节；名称由 revm 的表渲染。
    pub opcode: u8,
    /// PUSH 常量。
    pub immediate: Option<U256>,
    /// 栈顶先弹出的操作数；DUP 使用被复制值，SWAP 使用交换的两个值。
    pub operands: Vec<ValueId>,
    /// 此运算产生的名字。DUP/SWAP 的结果用既有名字表示，没有新定义。
    pub results: Vec<ValueId>,
    /// 当前指令因无效 opcode 或栈异常终止，此时没有正常栈效果。
    pub fault: bool,
}

impl Instruction {
    /// Original uses and retained definitions, excluding bytecode and effect IDs.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut ValueId> {
        self.operands.iter_mut().chain(&mut self.results)
    }
}

/// 一个抽象状态的 SSA；不同上下文会有不同的值名字。
#[derive(Clone, Debug, Serialize)]
pub struct Block {
    /// Underlying native machine-state ID.
    pub native_state: usize,
    /// 对应 Analysis 中的状态编号。
    pub state: usize,
    /// 入栈 φ 定义。
    pub phis: Vec<Phi>,
    /// 保留程序顺序的指令。
    pub instructions: Vec<Instruction>,
    /// 出栈值，作为所有出边的块参数。
    pub exit_stack: Vec<ValueId>,
}

/// 已通过结构验证的栈 SSA。尚未做 memory/storage SSA 或副作用重排。
#[derive(Clone, Debug, Serialize)]
pub struct Ssa {
    pub(crate) blocks: Vec<Block>,
    pub(crate) value_count: usize,
    pub(crate) execution: WorldSsa,
}

/// 输入未完成，或构建结果违反 SSA 不变量。
#[derive(Debug, Error)]
pub enum SsaError {
    /// The enclosing analysis was cancelled before the next SSA operation.
    #[error("SSA construction cancelled")]
    Cancelled,
    /// 缺边会使 φ 和 dominance 产生误导，因此拒绝截断分析。
    #[error("SSA requires a converged graph; analysis has unresolved frontiers")]
    IncompleteAnalysis,
    /// A native postprocessing copy or query cannot be fully funded.
    #[error("SSA postprocessing work exhausted or unavailable")]
    Work,
    /// 内部不变量失败，保留可定位的信息。
    #[error("SSA invariant failed: {0}")]
    Invariant(Box<SsaInvariant>),
    /// A normalized value or its proof does not match the verified execution.
    #[error("SSA normalization invariant failed: {0}")]
    Normalization(Box<NormalizationInvariant>),
}

pub(crate) fn checkpoint() -> Result<(), SsaError> {
    if analysis_control::current_cancellation().is_cancelled() {
        Err(SsaError::Cancelled)
    } else {
        Ok(())
    }
}

impl Ssa {
    /// 只读的 SSA 基本块。
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
    /// Native value namespace, including child definitions and continuation results.
    pub fn value_count(&self) -> usize {
        self.value_count
    }
    /// Full frame-aware SSA from which the local blocks are projected.
    pub fn execution(&self) -> &WorldSsa {
        &self.execution
    }
    /// Recheck native definitions, dominance, edge arguments and projection identity.
    pub fn verify<N: WordElement>(&self, analysis: &EvmAnalysis<N>) -> Result<(), SsaError> {
        verify::verify(self, analysis)
    }
}

/// 从收敛后的上下文 CFG 构建 SSA，并在返回前运行验证器。
pub fn build<N: WordElement>(analysis: &EvmAnalysis<N>) -> Result<Ssa, SsaError> {
    let result = build::build(analysis)?;
    result.verify(analysis)?;
    Ok(result)
}

/// Reserve postprocessing work before invoking opaque numeric code.
pub(crate) fn pay(budget: &mut crate::resource::WorkBudget, work: usize) -> Result<(), SsaError> {
    use crate::resource::{CloneError, clone_payment};
    clone_payment::pay(budget, work).map_err(|error| match error {
        CloneError::Cancelled => SsaError::Cancelled,
        CloneError::Work => SsaError::Work,
    })
}
