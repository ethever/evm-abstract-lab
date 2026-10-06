# 例子索引：从几条指令到跨合约执行

建议先按下面的单账户顺序学习，再运行多账户世界。所有命令在**仓库根目录**执行，不要先 `cd examples`。文件都是合成的离线输入；分析不需要 RPC 或资金。

## 单账户：先把栈和图读懂

`.hex` 文件把字节写成十六进制文本，允许空白。它们是部署后执行的 runtime bytecode，用 `--file examples/文件名.hex` 读取。

| 建议顺序与文件 | 要观察什么 | 检查位置 | 对应教程 |
| --- | --- | --- | --- |
| 1．[straight-line.hex](straight-line.hex) | `2+3` 写入内存并返回；给值起名 | ADD 在 `pc=0x04`；一个块的入口、出口栈均为空 | [00：第一遍运行](../docs/00-start.md) |
| 2．[diamond.hex](diamond.hex) | 两条分支汇合，值变为集合 | `pc=0x0e` 的入栈是 `[{0x1,0x2}]` | [02：集合值](../docs/02-domain.md)、[04：φ](../docs/04-ssa.md) |
| 3．[loop.hex](loop.hex) | 回边反复传播信息，直到不再变化 | 循环头 `pc=0x02`，观察入栈及循环 φ | [03：固定点](../docs/03-cfg.md) |
| 4．[dynamic-jump.hex](dynamic-jump.hex) | 目标未知时覆盖合法跳转及失败可能 | JUMP 在 `0x03`，JUMPDEST 在 `0x04`，SSTORE 在 `0x09` | [03：未知跳转](../docs/03-cfg.md) |
| 5．[stack-heights.hex](stack-heights.hex) | 同一块按不同入栈高分别分析 | `pc=0x0c` 对应 stack height=0 和 stack height=1 的状态 | [05：状态划分](../docs/05-sensitivity.md) |
| 6．[internal-calls.hex](internal-calls.hex) | 同一 helper 的两次内部跳转，比较历史长度 | helper 在 `pc=0x0e`，比较 `context_depth=0/1` | [05：跳转历史](../docs/05-sensitivity.md) |
| 7．[osaka-clz.hex](osaka-clz.hex) | 协议版本影响指令有效性和计算跳转 | Osaka 下 CLZ(1)=255，跳到 `pc=0x08` | [08：fork](../docs/08-forks.md) |

先用 `explain` 同时查看反汇编、CFG、SSA；只想看图时改用 `cfg`。下面的 diamond 和 loop 显式使用 `--context-depth 0`，以便观察汇合与循环固定点；未指定时默认为 8：

```bash
nix run . -- explain --file examples/straight-line.hex
nix run . -- explain --file examples/diamond.hex --context-depth 0
nix run . -- cfg --file examples/loop.hex --context-depth 0
```

调整一个参数，比较同一个输入的结果：

```bash
nix run . -- explain --file examples/internal-calls.hex --context-depth 0
nix run . -- explain --file examples/internal-calls.hex --context-depth 1
```

第二条命令保留一个最近跳转来源块的历史。检查 helper 的状态数、`context` 和返回边怎样变化；不是越大的参数就越容易收敛。

## 数值精度：集合以外还能知道什么

完成[第 02 课](../docs/02-domain.md)的集合手算后，比较默认组合域与 `--domain constants-only`。这三份输入都从未知 calldata 开始：

| 文件 | 手算条件 | 默认组合域的分支 | 仅有限集合的分支 |
| --- | --- | --- | --- |
| [known-bits-branch.hex](known-bits-branch.hex) | `(x AND 15) OR 1`，最低位为 1 | 只有 `BranchTrue` | `BranchTrue` 和 `BranchFalse` |
| [copy-identity.hex](copy-identity.hex) | 同一次读取经 DUP1 复制，`x XOR x = 0` | 只有 `BranchFalse` | `BranchTrue` 和 `BranchFalse` |
| [independent-inputs.hex](independent-inputs.hex) | 从偏移 0、32 分别读取 `x`、`y`，再 XOR | 两种分支 | 两种分支 |

```bash
nix run . -- cfg --file examples/known-bits-branch.hex --context-depth 0
nix run . -- cfg --file examples/known-bits-branch.hex --context-depth 0 --domain constants-only
nix run . -- explain --file examples/copy-identity.hex --context-depth 0
nix run . -- cfg --file examples/independent-inputs.hex --context-depth 0
```

三份输入都应 `Converged`。第三份里的值虽然都来自 calldata，却不保证相等。复制身份只在本次基本块执行内有效；完整实验与事实交换过程见[第 12 课](../docs/12-product-domains-facts.md)。

## 多账户：观察调用怎样影响返回值与状态

世界 JSON 提供多个账户的代码和初始事实，`--entry` 指定首先执行的账户。以下文件的入口统一为 `0x0000000000000000000000000000000000000101`；默认 caller 的地址末尾是 `1000`，calldata 为空，value=0。

先跑第一项：

```bash
nix run . -- explain \
  --world examples/worlds/call-return-branch.json \
  --entry 0x0000000000000000000000000000000000000101

nix run . -- explain \
  --world examples/worlds/returndata-copy.json \
  --entry 0x0000000000000000000000000000000000000101
```

A 调用 B，B 的返回数据让 A 选择分支。`explain` 默认显示捕获代码的反汇编、简明 CFG 与栈、每个 `O` 的结果概要，最后显示 `Verified cross-contract SSA:`。先在 `Transitions` 中找到 `Call` / `Return`，再看 `Outcomes` 中分别保留的结果；地址短引用的完整值在 `Addresses` 中。SSA 用 `%结果 = 指令 %操作数` 展示值流，`T` 标识转移。需要完整世界报告、捕获帧与效果链时加 `--verbose`；需要查询 JSON 字段时改用 `analyze --format json`，再加 `--ssa` 可导出完成图的 SSA。

反汇编中的代码身份由代码地址、hash 和模式区分；状态账户另行保留。代理可以共享实现代码，创建例子可以在同一地址执行 InitCode 与新安装的 Runtime。指令列表与状态入口、出口栈共同描述抽象分析，不能当作逐指令具体步骤记录。表中写的是**具体成功轨迹**；抽象模型还保留 gas 等失败可能，因此实际输出可能包含更大的值集合或其他 outcome。`Incomplete` 时 `explain` 保留反汇编、部分图、每个已知结果及全部诊断与前沿，显示 `SSA unavailable` 并退出 `2`；`--verbose` 保留完整报告分区。

| 阶段与世界文件 | 观察问题 | 具体成功轨迹或检查条件 |
| --- | --- | --- |
| 返回：[call-return-branch.json](worlds/call-return-branch.json) | B 的返回字节如何控制 A 的分支？ | A slot 0=1 |
| 返回：[returndata-copy.json](worlds/returndata-copy.json) | CALL 没有复制输出时，怎样随后取回数据？ | RETURNDATACOPY 后返回 32 字节数值 1 |
| 代理：[proxy-storage.json](worlds/proxy-storage.json) | 共享实现是否意味着共享 storage？ | P1/P2 slot 0=6/10；实现自身仍为 99 |
| 代理：[callcode-context.json](worlds/callcode-context.json) | CALLCODE 的状态账户、caller 和 value 是什么？ | caller 为代理，value=3 |
| 回滚：[revert-rollback.json](worlds/revert-rollback.json) | 子调用失败后，写入和返回字节分别怎样处理？ | B slot 0 保留 4，A slot 1=42 |
| 静态限制：[static-write.json](worlds/static-write.json) | 静态子帧执行 SSTORE 会怎样？ | 调用成功位 0，B slot 0 保留 4 |
| 重入：[reentry.json](worlds/reentry.json) | 再次进入 A 时能否看到刚才的写入？ | A slot 0=2，slot 1=1 |
| 日志：[log-rollback.json](worlds/log-rollback.json) | REVERT 是否撤销子帧的日志？ | 保留 A 的事件，撤销 B 的事件 |
| 缺失事实：[missing-code.json](worlds/missing-code.json) | 缺少 B 的代码能否当作空代码？ | `Incomplete`、`MissingCode`、退出 2 |

这些实验的帧、状态和 JSON 字段解读见[第 09 课](../docs/09-cross-contract.md)。`storage_unknown:false` 表示合成输入明确假设所有未列出的初始 slot 都为零；省略它时，未列出的 slot 是未知。空代码与缺失代码也不同。

## 进阶：复用分析与改变代码

先读[第 10 课](../docs/10-snapshots-summaries-creation.md)的前提和步骤，再查看这些输出：

| 世界文件 | 观察问题 | 检查条件 |
| --- | --- | --- |
| [summary-reuse.json](worlds/summary-reuse.json) | 相同 callee 输入怎样复用已完成的调用分析？ | `summary_stats.hits > 0`；开关摘要后的最终关系一致 |
| [create-runtime.json](worlds/create-runtime.json) | 创建时运行的代码与部署后的代码怎样区分？ | 帧有 InitCode/Runtime 两种模式；成功路径返回 42，新账户 nonce=1 |
| [created-selfdestruct.json](worlds/created-selfdestruct.json) | 同交易创建的账户何时删除？ | 销毁后仍可读到 8 字节代码；完整销毁路径结束后目标 absent、受益人余额=7 |
| [identity-precompile.json](worlds/identity-precompile.json) | 无普通 runtime 的原生调用怎样进入图？ | 成功路径输出 42；图中有 Precompile 帧及 Call/Return 边 |

创建例子显式声明 nonce、账户存在性及确认不存在的目标账户。JSON 没有列出一个地址，仅表示缺少事实，不能据此判定它不存在。

## 这些例子怎样被核对

[`concrete.rs`](../crates/evm-abstract/tests/concrete.rs) 用 revm 核对前六个单账户例子的具体入口、边、出栈和值，覆盖 Cancun/Prague/Osaka 和 k=0/1/2/8/10。具体调用输入是 32 字节，前 31 字节均为 `00`：diamond/stack-heights 的末字节分别取 0 与 1；dynamic-jump 的末字节取 4，确保走到合法目标并写 storage。[`osaka.rs`](../crates/evm-abstract/tests/osaka.rs) 核对 CLZ，并验证旧 fork 下的指令故障。新增的三份数值精度输入由完整门禁使用打包后的 CLI 执行 CFG、SSA；域、复制身份和反馈限制另有[组合域测试](../crates/evm-abstract/tests/product_domains.rs)。

[`cross_concrete.rs`](../crates/evm-abstract/tests/cross_concrete.rs) 对照多账户具体轨迹和账户效果；[`summaries.rs`](../crates/evm-abstract/tests/summaries.rs) 比较缓存开关后的最终关系；[`creation.rs`](../crates/evm-abstract/tests/creation.rs) 核对创建、延迟删除和原生调用。这些是指定样例的核对证据，范围见[第 06 课](../docs/06-boundaries.md)。
