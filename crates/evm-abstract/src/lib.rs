//! 从 EVM 字节码出发，学习抽象解释、控制流和栈 SSA。
//!
//! 阅读顺序：[`bytecode`] → [`domain`] → [`analysis`] → [`ssa`]。
//! 库分析 **Cancun legacy runtime bytecode**；它不执行交易、不连接节点。
//! gas、内存、storage 和外部调用结果被保守抽象，因此结果是可能执行的图，
//! 不能直接当作漏洞证明或完整 EVM 验证器。见仓库 `docs/06-boundaries.md`。
//!
//! ```
//! use evm_abstract::{analysis::{analyze, Config, Status}, bytecode::Program, ssa};
//! let program = Program::from_hex("600160020100")?;
//! let analysis = analyze(program, Config::default())?;
//! assert_eq!(analysis.status(), Status::Converged);
//! let ir = ssa::build(&analysis)?;
//! assert_eq!(ir.value_count(), 3);
//! ir.verify(&analysis)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod analysis;
pub mod bytecode;
pub mod domain;
pub mod render;
pub mod ssa;

pub use alloy_primitives::U256;
