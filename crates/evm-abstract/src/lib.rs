//! 从多账户世界与根调用出发，学习跨合约抽象执行、调用图和 SSA。
//!
//! 阅读顺序：[`world`] → [`bytecode`] → [`domain`] → [`analysis`] → [`ssa`]。
//! [`analysis::AnalysisInput`] 固定世界与根调用；主入口 [`analysis::AnalysisEngine`]
//! 借用显式构造的数值代数、执行策略和协作组件，将调用栈、账户状态、输入、
//! 返回与回滚放在同一工作表中。单程序 CFG 是原生执行结果的显式投影。
//! 固定离线输入使用**普通 EVM 运行时字节码**，即操作码及其立即数组成的
//! 指令流，不支持 EOF 容器格式。执行规则默认 Osaka，可选 Cancun/Prague；
//! 格式与规则版本是不同概念。[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 单层代码解析保留 authority 的状态上下文。
//! 完整调用关系可在相同快照、代码、上下文及状态前置条件下复用；证书保留
//! 真实指令图。CREATE/CREATE2 执行 initcode 并安装事务代码 overlay，
//! SELFDESTRUCT 遵循所选 fork 的延迟删除规则。固定 hash RPC 是显式输入。
//! gas 与部分环境值保守抽象；缺失或不能表示的事实与资源截断保留未完成
//! 前沿。结果是可能执行的图，不能直接当作安全证明。
//! 见仓库的[模型边界文档](https://github.com/ethever/evm-abstract-lab/blob/main/docs/06-boundaries.md)。
//!
//! 直接构造世界与根入口，执行返回整数 `42` 的程序，并验证整个原生图的 SSA：
//!
//! ```
//! use evm_abstract::{
//!     Address, Fork,
//!     analysis::{AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, Status,
//!         control::Control, cooperation::NoExtension},
//!     domain::{DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra},
//!     world::{EvmAccount, EvmEntry, EvmWorld}, ssa,
//! };
//!
//! let values = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
//! let policies = EnginePolicies::default();
//! let mut extension = NoExtension;
//! let mut engine = AnalysisEngine::new(&values, &policies, &mut extension);
//! let control = Control::default();
//! let address = Address::repeat_byte(0x11);
//! let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "offline example");
//! world.insert(address,
//!     EvmAccount::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap()).unwrap();
//! let input = AnalysisInput::new(world, EvmEntry::new(address)).unwrap();
//! let graph = engine.run(input, ExecutionConfig::default(), &control).unwrap();
//! assert_eq!(graph.status(), Status::Converged);
//! control.scope(|| ssa::build_world(&graph).unwrap().verify(&graph).unwrap());
//! ```
//!
//! 单程序输入也交给同一原生引擎执行；随后显式投影局部 CFG，验证栈 SSA。
//! 投影在 `Control::scope` 内使用独立于执行预算的有限后处理账本，不再次执行程序。
//!
//! ```
//! use evm_abstract::{
//!     analysis::{AnalysisEngine, AnalysisInput, EnginePolicies, EvmAnalysis, ExecutionConfig,
//!         Status, control::Control, cooperation::NoExtension},
//!     bytecode::Program,
//!     domain::{DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra},
//!     resource::WorkBudget, world::EvmEnvironmentValues, ssa,
//! };
//!
//! let values = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
//! let policies = EnginePolicies::default();
//! let mut extension = NoExtension;
//! let mut engine = AnalysisEngine::new(&values, &policies, &mut extension);
//! let control = Control::default();
//! let program = Program::from_hex("600160020100").unwrap();
//! let input = AnalysisInput::<NumericValue>::from_program(
//!     program, EvmEnvironmentValues::default()).unwrap();
//! let native = engine.run(input, ExecutionConfig::default(), &control).unwrap();
//! control.scope(|| {
//!     let mut postprocessing = WorkBudget::new(1_000_000);
//!     let local = EvmAnalysis::from_execution(native, &mut postprocessing).unwrap();
//!     assert_eq!(local.status(), Status::Converged);
//!     let ir = ssa::build(&local).unwrap();
//!     ir.verify(&local).unwrap();
//! });
//! ```

pub mod analysis;
pub mod bytecode;
pub mod domain;
pub mod fork;
pub mod render;
pub mod resource;
pub mod ssa;
pub mod world;

#[cfg(test)]
mod test_instrumentation;

pub use alloy_primitives::Address;
pub use alloy_primitives::U256;
pub use fork::Fork;

/// Typed facts, registered reductions and dependency scheduling used by the
/// production analysis composition. Downstream components can implement these
/// exact protocols through the same dependency instance as the EVM adapters.
pub use analysis_kernel as kernel;
