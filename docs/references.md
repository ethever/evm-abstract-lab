# 参考资料与阅读顺序

不要求一次读完原论文。先把一个运行结果解释通，再选择相应资料。

| 想确认的事实 | 第一手资料 | 如何对照本仓库 |
| --- | --- | --- |
| EVM JUMP/JUMPI、PC、STOP 语义 | [Ethereum execution-specs：Cancun 控制流](https://github.com/ethereum/execution-specs/blob/master/src/ethereum/forks/cancun/vm/instructions/control_flow.py) | `bytecode.rs`、`analysis/transfer.rs` |
| 算术、补码、位运算规则 | [Cancun VM 指令实现](https://github.com/ethereum/execution-specs/tree/master/src/ethereum/forks/cancun/vm/instructions) | `domain.rs` 的栈顶先弹出参数顺序 |
| 抽象解释为什么能系统地近似执行 | [Patrick 与 Radhia Cousot，POPL 1977](https://www.di.ens.fr/~cousot/COUSOTpapers/POPL77.shtml) | 先掌握集合 join，再阅读抽象域与固定点 |
| SSA 与 φ、dominance frontier | [Cornell CS 6120：Static Single Assignment](https://www.cs.cornell.edu/courses/cs6120/2025sp/lesson/6/) | `ssa/build.rs` 的块参数式教学构建与经典算法比较 |
| 支配算法与定义 | [petgraph 0.8.3 dominators](https://docs.rs/petgraph/0.8.3/petgraph/algo/dominators/index.html) | 验证器复用 `simple_fast` |
| opcode 元数据 | [revm-bytecode 43.0.0](https://docs.rs/revm-bytecode/43.0.0/revm_bytecode/) | 不另维护名称与栈 I/O 表；按程序选择的 fork 检查启用 |
| 当前主网与未来升级 | [Ethereum 官方路线图](https://ethereum.org/roadmap/)、[Fusaka Meta EIP-7607](https://eips.ethereum.org/EIPS/eip-7607) | 2026-10-02 核验：默认 Osaka；BPO 调整不增加 opcode |
| CLZ 数值语义 | [EIP-7939](https://eips.ethereum.org/EIPS/eip-7939) | 0x1e、单输入/单输出、零返回 256；用 revm 核对 |
| EOA 代码委托 | [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) | 区分账户代码标记与执行代码，解析器复用 revm |
| 调用后的返回数据缓冲区与越界 | [EIP-211](https://eips.ethereum.org/EIPS/eip-211) | 每帧 returndata、调用时清空、REVERT 数据与 RETURNDATACOPY |
| 静态调用的继承与禁止效果 | [EIP-214](https://eips.ethereum.org/EIPS/eip-214) | STATICCALL、SSTORE/LOG/value CALL 故障与 CALLCODE 例外 |
| transient storage 的帧归属与回滚 | [EIP-1153](https://eips.ethereum.org/EIPS/eip-1153) | 同一交易的跨帧状态、DELEGATECALL owner 和 REVERT checkpoint |
| Osaka 的 P256VERIFY 预编译地址 | [EIP-7951](https://eips.ethereum.org/EIPS/eip-7951) | 地址 0x100 保留给预编译，离线例子入口选择 0x101；oracle 要实际访问字节码 |
| U256 运算 | [alloy-primitives 1.7.3](https://docs.rs/alloy-primitives/1.7.3/alloy_primitives/) | 大整数与快速幂来自库；边界用 revm 对照 |
| Rust 稳定发行版本 | [Rust 官方发行记录](https://github.com/rust-lang/rust/releases/tag/1.99.0) | `rust-toolchain.toml` 固定创建时最新稳定版 1.99.0 |
| Nix 与同一 Rust 文件 | [rust-overlay 的 fromRustupToolchainFile](https://github.com/oxalica/rust-overlay#cheat-sheet-common-usage-of-rust-bin) | `flake.nix` 与 rustup 共用配置 |
| Rust 的 Nix 构建/依赖缓存 | [crane 官方文档](https://crane.dev/) | 构建、Clippy、Rustdoc 使用同一工具链与依赖锁 |

链接指向上游规范或作者资料；项目依赖的确切版本由 Cargo.lock/flake.lock 决定。execution-specs 的默认分支可能更新，因此本仓库测试显式选择 revm 的 Cancun/Prague/Osaka 配置，并没有把上游整套测试向量复制成当前门禁。
