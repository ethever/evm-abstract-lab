//! 把“栈位置”改写成“值的名字”：静态单赋值（SSA）。
//!
//! `DUP` 不创造一个新的运算值，两个槽位指向同一 ValueId；`SWAP` 只交换身份。
//! 每个非入口块的入栈槽位预先分配 φ，然后补上每条前驱边携带的值。
//! 预分配使循环成为有限的引用环，不需要反复生成无限多个名字。
//! 这是便于学习的非最小 SSA：单前驱 φ 也保留，后续可以实现 trivial-φ 消除。
//! dominance 算法使用 petgraph，`verify` 检查唯一赋值、支配和 φ 边一致性。

mod build;
mod error;
pub use error::{SsaInvariant, SsaInvariantKind};
mod partial;
mod verify;
mod world;

pub use partial::{
    DeferredEdgeReason, DeferredSsaEdge, PartialBlockCoverage, PartialInstruction,
    PartialWorldBlock, PartialWorldSsa, build_partial_world,
};
pub use world::{
    EffectId, EffectInput, EffectPhi, FramePhi, Transition, WorldBlock, WorldSsa, build_world,
};

use crate::analysis::Analysis;
use alloy_primitives::U256;
use serde::Serialize;
use thiserror::Error;

/// SSA 中的值名；显示为 `%0`、`%1`……。栈槽位可以同时引用同一个值。
pub type ValueId = usize;

/// φ 的一条边输入；取值由最近经过的前驱确定，不是对数值求并。
#[derive(Clone, Debug, Serialize)]
pub struct PhiInput {
    /// 前驱抽象状态的编号。
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
#[derive(Clone, Debug, Serialize)]
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

/// 一个抽象状态的 SSA；不同上下文会有不同的值名字。
#[derive(Clone, Debug, Serialize)]
pub struct Block {
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
    /// 内部不变量失败，保留可定位的信息。
    #[error("SSA invariant failed: {0}")]
    Invariant(Box<SsaInvariant>),
}

pub(crate) fn checkpoint() -> Result<(), SsaError> {
    if embedded_smt::current_cancellation().is_cancelled() {
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
    /// 唯一定义的值数，ValueId 的范围为 0..value_count。
    pub fn value_count(&self) -> usize {
        self.value_count
    }
    /// 重新检查定义唯一性、普通 use 的 dominance、φ 的边输入和栈高。
    pub fn verify(&self, analysis: &Analysis) -> Result<(), SsaError> {
        verify::verify(self, analysis)
    }
}

/// 从收敛后的上下文 CFG 构建 SSA，并在返回前运行验证器。
pub fn build(analysis: &Analysis) -> Result<Ssa, SsaError> {
    let result = build::build(analysis)?;
    result.verify(analysis)?;
    Ok(result)
}
