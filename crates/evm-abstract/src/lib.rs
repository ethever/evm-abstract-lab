//! 从多账户世界与根调用出发，学习跨合约抽象执行、调用图和 SSA。
//!
//! 阅读顺序：[`world`] → [`bytecode`] → [`domain`] → [`analysis`] → [`ssa`]。
//! 主入口 [`analysis::analyze_world`] 将调用栈、账户状态、输入、返回与回滚
//! 放在同一工作表中；单账户 CFG 是这台机器的学习视图。
//! 固定离线输入使用 **legacy runtime bytecode**，默认 Osaka，可选
//! Cancun/Prague。EIP-7702 单层代码解析保留 authority 的状态上下文。
//! 完整调用关系可在相同快照、代码、上下文及状态前置条件下复用；证书保留
//! 真实指令图。CREATE/CREATE2 执行 initcode 并安装事务代码 overlay，
//! SELFDESTRUCT 遵循所选 fork 的延迟删除规则。固定 hash RPC 是显式输入。
//! gas 与部分环境值保守抽象；缺失或不能表示的事实与资源截断保留未完成
//! 前沿。结果是可能执行的图，不能直接当作安全证明。
//! 见仓库 `docs/06-boundaries.md`。
//!
//! ```
//! use evm_abstract::{Address, Fork, U256, analysis, domain::Value,
//!     world::{Account, ByteArray, Entry, World}, ssa};
//! let address = Address::repeat_byte(0x11);
//! let mut world = World::new(Fork::Osaka, "offline example");
//! world.insert(address, Account::from_hex("602a5f5260205ff3", Fork::Osaka)?)?;
//! let graph = analysis::analyze_world(world, Entry {
//!     address, caller: Address::repeat_byte(0x22), value: Value::constant(U256::ZERO),
//!     calldata: ByteArray::empty(), is_static: false,
//! }, analysis::ExecutionConfig::default())?;
//! assert_eq!(graph.status(), analysis::Status::Converged);
//! ssa::build_world(&graph)?.verify(&graph)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! 单字节码适配入口仍可用于栈 CFG/SSA 实验：
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
pub mod fork;
pub mod render;
pub mod ssa;
pub mod world;

pub use alloy_primitives::Address;
pub use alloy_primitives::U256;
pub use fork::Fork;
