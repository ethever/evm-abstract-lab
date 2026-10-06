# 参考资料：带着一个问题查原文

先运行一个例子、解释输出，再查它对应的资料。教程负责给出操作步骤；这一页用于确认协议规则、理解算法依据或追踪依赖实现，不要求从头读完所有链接。

| 你目前卡在哪里 | 先在本仓库看 | 再查什么 |
| --- | --- | --- |
| 字节、栈、pc 不知道怎么读 | [00](00-start.md)、[01](01-bytecode.md) 的逐指令表 | EVM 指令实现 |
| 不明白为什么合并用并集 | [02](02-domain.md) 的集合实验 | 抽象解释原论文 |
| 不明白循环怎样停止 | [03](03-cfg.md) 的工作表过程 | 抽象域与固定点 |
| 不明白 φ 或值的来源 | [04](04-ssa.md) 的真实输出 | SSA 教学与支配算法 |
| 常量列不完时还怎样排除分支 | [02](02-domain.md)、[12](12-product-domains-facts.md) 的位与复制实验 | 抽象域、组件组合与保守近似；先核对每条指令实际传播哪些约束 |
| 调用、回滚、代理结果不符合预期 | [09](09-cross-contract.md) 的帧与状态表 | 对应 CALL、返回数据或静态限制规范 |
| 快照、创建或销毁事实有疑问 | [10](10-snapshots-summaries-creation.md) 的实验 | 固定 hash RPC、CREATE2、SELFDESTRUCT 规范 |

## EVM 指令与协议版本

先找到正在执行的 opcode，再查看它弹栈、压栈和停止执行的规则。注意选择与输入相符的 fork；一个新指令的规范不能直接用于旧版本的分析。

| 要确认什么 | 第一手资料 | 对照本仓库 |
| --- | --- | --- |
| JUMP/JUMPI、PC、STOP 的控制流 | [Ethereum execution-specs：Cancun 控制流](https://github.com/ethereum/execution-specs/blob/master/src/ethereum/forks/cancun/vm/instructions/control_flow.py) | [`bytecode.rs`](../crates/evm-abstract/src/bytecode.rs)、[`transfer.rs`](../crates/evm-abstract/src/analysis/transfer.rs) |
| 算术、补码、位运算和参数顺序 | [Cancun VM 指令实现](https://github.com/ethereum/execution-specs/tree/master/src/ethereum/forks/cancun/vm/instructions) | [`domain.rs`](../crates/evm-abstract/src/domain.rs)；参数按栈顶先弹出排列 |
| 升级包含哪些规则 | [Ethereum 官方路线图](https://ethereum.org/roadmap/)、[Fusaka Meta EIP-7607](https://eips.ethereum.org/EIPS/eip-7607) | [第 08 课](08-forks.md)、[`fork.rs`](../crates/evm-abstract/src/fork.rs)；项目默认 fork 与某个区块实际采用的 fork 要分别确认 |
| CLZ 怎样计算前导零 | [EIP-7939](https://eips.ethereum.org/EIPS/eip-7939) | `osaka-clz.hex`；`0x1e`，零输入得到 256 |
| EOA 的代码委托标记是什么 | [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) | 区分账户的委托标记与目标代码；单段解码和世界执行的处理不同 |

execution-specs 的默认分支会更新。以上 Cancun 链接用于相应规则的对照，项目测试显式选择 revm 的 Cancun/Prague/Osaka 配置，没有把上游整套测试向量作为本仓库门禁。

## 集合分析与 SSA 的理论

| 要理解什么 | 作者或课程资料 | 建议怎么读 |
| --- | --- | --- |
| 为什么可以用摘要近似许多具体执行 | [Patrick 与 Radhia Cousot，POPL 1977](https://www.di.ens.fr/~cousot/COUSOTpapers/POPL77.shtml) | 先掌握第 02 课的集合 join，再看抽象域、序关系与固定点 |
| 为什么 SSA 给每个定义起唯一名字，怎样处理分支 | [Cornell CS 6120：Static Single Assignment](https://www.cs.cornell.edu/courses/cs6120/2025sp/lesson/6/) | 对照第 04 课的 φ；经典构建算法与本仓库块参数式构建不完全相同 |
| 如何检查一个值的定义是否在使用之前必经 | [petgraph 0.8.3 dominators](https://docs.rs/petgraph/0.8.3/petgraph/algo/dominators/index.html) | 验证器使用 `simple_fast` 计算支配关系；先看第 04 课的定义与使用例子 |

## 跨合约调用、状态与快照

先分清“谁的代码在执行”和“谁的状态在更新”，然后查对应规则。EIP（Ethereum Improvement Proposal，以太坊改进提案）给出特定机制的规范；这里的编号是查找入口，无需背诵。

| 要确认什么 | 第一手资料 | 本仓库中的观察点 |
| --- | --- | --- |
| 返回数据缓冲区、REVERT 数据及复制越界 | [EIP-211](https://eips.ethereum.org/EIPS/eip-211) | 每帧 returndata，RETURNDATACOPY 与 CALL 输出区 |
| STATICCALL 限制怎样向子帧传播 | [EIP-214](https://eips.ethereum.org/EIPS/eip-214) | SSTORE/LOG/value CALL 的故障与 CALLCODE 规则 |
| transient storage 属于哪个账户，怎样回滚 | [EIP-1153](https://eips.ethereum.org/EIPS/eip-1153) | 同交易共享状态、DELEGATECALL 的状态账户、REVERT checkpoint |
| 怎样把 RPC 请求固定在同一个区块 | [EIP-1898](https://eips.ethereum.org/EIPS/eip-1898) | 启动时解析区块，再以 exact block hash 与 canonical selector 采集状态；loader 完全信任提供者，不请求账户或槽位证明 |
| CREATE2 怎样确定地址，initcode 有哪些限制 | [EIP-1014](https://eips.ethereum.org/EIPS/eip-1014)、[EIP-3860](https://eips.ethereum.org/EIPS/eip-3860) | salt 与 initcode hash、创建帧和长度检查 |
| SELFDESTRUCT 何时删除代码与存储 | [EIP-6780](https://eips.ethereum.org/EIPS/eip-6780) | 区分预先存在和同交易创建的账户；后者延迟删除，并接受祖先回滚 |
| Osaka 的 P256VERIFY 位于哪个地址 | [EIP-7951](https://eips.ethereum.org/EIPS/eip-7951) | `0x100` 是预编译地址，因此离线字节码例子入口选用 `0x101` |

## 依赖实现与工具链

这组资料适合准备读 Rust 源码时使用。确切版本由 `Cargo.lock` / `flake.lock` 决定；下表说明各库负责的部分，不意味着依赖文档能替代分析器的语义验证。

| 实现问题 | 官方资料 | 本仓库使用方式 |
| --- | --- | --- |
| opcode 名称与栈 I/O 元数据 | [revm-bytecode 43.0.0](https://docs.rs/revm-bytecode/43.0.0/revm_bytecode/) | 复用元数据，再按所选 fork 检查启用；PUSH 宽度与立即数由 `bytecode.rs` 解码 |
| 原生预编译计算 | [revm-precompile 43.0.3](https://docs.rs/revm-precompile/43.0.3/revm_precompile/) | 选 fork 注册表；分析器另负责输入资格、返回流及累计工作预算 |
| 256 bit 数与模运算 | [alloy-primitives 1.7.3](https://docs.rs/alloy-primitives/1.7.3/alloy_primitives/) | 使用 U256 与库算法；测试和 revm 对照 |
| 固定的 Rust 工具链 | [Rust 1.99.0 发行记录](https://github.com/rust-lang/rust/releases/tag/1.99.0) | [`rust-toolchain.toml`](../rust-toolchain.toml) 指定版本；不表示永远是最新稳定版 |
| Nix 怎样读取同一份 Rust 版本文件 | [rust-overlay 的 fromRustupToolchainFile](https://github.com/oxalica/rust-overlay#cheat-sheet-common-usage-of-rust-bin) | Nix 与 rustup 共用工具链配置 |
| Rust 的 Nix 构建与依赖缓存 | [crane 官方文档](https://crane.dev/) | 构建、Clippy 和 Rustdoc 使用同一套锁定工具链 |

回到[课程导航](../README.md#推荐阅读顺序)，或按[第 07 课](07-exercises.md)的验收方法动手验证一个问题。
