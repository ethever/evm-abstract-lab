# 08：协议版本为什么是分析输入

本课目标：知道 Cancun 与“最新 Ethereum”不是同一个概念，能亲手观察 opcode 启用如何改变 CFG，并理解代码委托与直接执行的区别。

## 执行层、共识层与升级名

Ethereum 的一次网络升级可能同时修改执行层与共识层。整体升级名常由两者组合而来。分析 EVM bytecode 主要需要执行层规则。

| 整体升级 | 执行层 | 共识层 | 主网激活日 |
| --- | --- | --- | --- |
| Dencun | Cancun | Deneb | 2024-03-13 |
| Pectra | Prague | Electra | 2025-05-07 |
| Fusaka | Osaka | Fulu | 2025-12-03 |

2026-10-02 核验时，最新已激活的具名主网升级是 Fusaka。其后的 BPO1/BPO2 调整 blob 参数，没有增加 EVM opcode。本仓库默认选择 Osaka。Glamsterdam 的执行层名为 Amsterdam，仍在上线准备中，主网激活日未确定。[官方路线图](https://ethereum.org/roadmap/)、[Fusaka 规范](https://eips.ethereum.org/EIPS/eip-7607)、[Glamsterdam 规范](https://eips.ethereum.org/EIPS/eip-7773)

revm 含有 Amsterdam 的开发中实现，不能据此自动启用。新 opcode 只有在选定规则允许时才参与抽象执行；这避免把未来规则混进历史或当前主网分析。

## 同一段代码，两个不同的结果

[`osaka-clz.hex`](../examples/osaka-clz.hex) 是：

```text
60011e60f79003565b602a00
```

在 Osaka 下：

| pc | 指令 | 栈（底 → 顶） |
| --- | --- | --- |
| 0 | PUSH1 1 | `[1]` |
| 2 | CLZ | `[255]` |
| 3 | PUSH1 247 | `[255,247]` |
| 5 | SWAP1 | `[247,255]` |
| 6 | SUB | `[8]`，栈顶先弹出，255−247=8 |
| 7 | JUMP | 到真实 JUMPDEST pc=8 |
| 8 | JUMPDEST | `[]` |
| 9 | PUSH1 42 | `[42]` |
| 11 | STOP | 结束 |

```bash
nix run . -- explain --file examples/osaka-clz.hex
nix run . -- explain --file examples/osaka-clz.hex --fork cancun
nix run . -- explain --file examples/osaka-clz.hex --fork prague
```

默认 Osaka 能精确恢复一条 JUMP 边并保留结果 42。两个旧 fork 会在 pc=2 将 0x1e 视为无效指令，之后的目标不可达；SSA 将该指令显示为异常终止。这不是同一模式下的“精度变化”，而是协议语义真的不同。

## CLZ 的抽象语义

[EIP-7939](https://eips.ethereum.org/EIPS/eip-7939) 引入 0x1e，弹出一个 U256、压入其前导零数。最高位已置位时返回 0，只有最低位置位时返回 255，零返回 256。

实现复用 alloy/ruint 的 `leading_zeros()`。有限集合逐值计算后合并；Top 的结果范围是 0..=256，如果容量不足仍升到 Top。不能因为“只保留八个数”而漏掉其他合法结果。

解析与切块见 [`bytecode.rs`](../crates/evm-abstract/src/bytecode.rs)，启用规则见 [`fork.rs`](../crates/evm-abstract/src/fork.rs)，数值语义见 [`domain.rs`](../crates/evm-abstract/src/domain.rs)。SSA 不需要重新实现 CLZ：普通单输入/单输出的定义与使用从 revm 元数据获得。

## EIP-7702：账户代码不总是指令流

Prague 开始，一个 EOA 的账户代码可以是 23 字节的 `ef0100 || address`。这是委托标记；执行客户端从目标取得代码，使用委托账户的上下文执行。[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702)

如果直接把这个标记当作普通 opcode 流，会在 0xef 停止并生成误导性的终止 CFG。单段字节码入口复用 revm 的格式解析器，返回带目标地址的 `DelegatedCode` 错误；畸形长度/版本保留其具体解析错误。世界入口则把它保存为 `Code::Delegation`，从同一固定世界取得目标代码，同时保持委托账户的 storage、余额和 ADDRESS 身份。两个入口都不自动访问 RPC。

EIP-7702 限制委托解析为一层；世界执行不会递归跟随目标的另一个委托标记。目标代码未提供时保存 MissingCode 前沿，不能仅凭委托账户存在就声明分析完成。授权交易列表本身仍属于模型边界。

## 规则怎样保持一致

世界文件固定 fork，账户插入时拒绝与世界规则不一致的 runtime；单段 CLI 的 `--fork` 传给 `Program::from_hex_with_fork`。Program 与每条 Instruction 保存同一个不可修改的 Fork，解码、切块、抽象 transfer、SSA 都使用它。JSON 的 `world.fork` / `program.fork`、文本和 DOT 的 `fork=` 保留选择结果。

只有图形看起来相同并不足以证明选择生效。本仓库既检查输出 metadata，也用三个 fork 的 revm 配置对照真实执行；CLZ 另用零与全部 256 个单置位值核对，旧 fork 明确验证其未启用。

“支持 Osaka”在这里指世界执行和单账户视图使用 Osaka 的字节码规则，并从同一 fork 的注册表选择原生预编译。内存、storage、调用帧、有限创建与 EIP-6780 已进入语义；gas 精确计量、无法表示的原生输入、完整授权交易处理仍属于[已列出的模型边界](06-boundaries.md)。世界中的 EIP-7702 标记可以按快照解析目标代码，同时保留委托账户的状态身份；这不等于处理交易里的 authorization list。协议版本选择不能代替这些能力。
