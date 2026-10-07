# 例子索引：从几条指令到跨合约执行

建议先按[下面的单账户顺序](#单账户先把栈和图读懂)学习，再运行[多账户世界](#多账户观察调用怎样影响返回值与状态)。所有命令在**仓库根目录**执行，不要先 `cd examples`。文件都是合成的离线输入；分析不需要 RPC 或资金。

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

先用 `explain` 同时查看反汇编、CFG、SSA；只想看图时改用 `cfg`。下面的 [diamond](diamond.hex) 和 [loop](loop.hex) 显式使用 `--context-depth 0`，[loop](loop.hex) 另外关闭关系层以观察纯数值固定点；以便观察汇合与循环固定点；未指定时默认为 8：

```bash
nix run . -- explain --file examples/straight-line.hex
nix run . -- explain --file examples/diamond.hex --context-depth 0
nix run . -- cfg --file examples/loop.hex --no-relations --context-depth 0
```

调整一个参数，比较同一个输入的结果：

```bash
nix run . -- explain --file examples/internal-calls.hex --context-depth 0
nix run . -- explain --file examples/internal-calls.hex --context-depth 1
```

第二条命令保留一个最近跳转来源块的历史。检查 helper 的状态数、`context` 和返回边怎样变化；不是越大的参数就越容易收敛。

## 内存：从字节读写到抽象数组

[`straight-line.hex`](straight-line.hex) 已展示把计算结果写入内存再返回。继续读[14：EVM内存与抽象字节数组](../docs/14-memory-model.md)，先用 MSTORE/MLOAD、MSTORE8 和 MSIZE 手算字节、偏移与长度，再观察多个可能偏移、逐字节汇合和预算前沿。该课包含运行命令与预期输出；CALL 传递输入、复制返回字节的完整流程见[第 09 课](../docs/09-cross-contract.md)。

| 文件 | 读写过程 | 要观察什么 |
| --- | --- | --- |
| [memory-word.hex](memory-word.hex) | 向偏移 0 写入 42，再读取 word 和 MSIZE | 出口为 `[{0x2a}, {0x20}]`：内容 42 与长度 32 是两个栈槽 |
| [memory-word-overwrite.hex](memory-word-overwrite.hex) | 先存 `0x1234`，再分别覆盖偏移 30、31 | 两次读取保留 `0xaa34`、`0xaabb`，旧读值不随后续写入改变 |
| [memory-symbolic-offset.hex](memory-symbolic-offset.hex) | 同一未知 p 经 DUP 后写入 7，再从 p 读取 | 符号身份仍在，未知位置写后读关系未一般保存；空 calldata 固定 p=0 后读出 7 |
| [memory-write-alias.hex](memory-write-alias.hex) | 未知输入选择偏移 0 或 1，随后 MSTORE8 写一个字节 | `--context-depth 0` 汇合后，两个位置各自可能为零或 aa；并不证明一次执行同时写两处 |
| [memory-byte-correlation.hex](memory-byte-correlation.hex) | 分支得到 `0x0101` 或 `0x0202`，汇合后写入并读出 | `--context-depth 0` 额外允许 `0x0102`、`0x0201`；与深度 8 的分开状态比较 |
| [memory-overlap-copy.hex](memory-overlap-copy.hex) | MCOPY 将偏移 28～30 复制到 29～31 | 重叠复制使用原始源字节，MLOAD(0) 得 `0x01010203` |
| [memory-word-bound.json](memory-word-bound.json) | 向偏移 1 写入 `0x1234`，再读取 word 和 MSIZE | 需要 64 字节；将 `--max-memory-bytes` 降到 32 时留下 `Memory` 前沿 |

最后一项是单账户世界文件，按[第 14 课的完整 memory 查询](../docs/14-memory-model.md#12-怎样看完整-memory而不只看-stack-out)命令显式提供 `--evm.to`、`--evm.value 0` 与 `--evm.calldata 0x`。先读正常结果，再运行[上限实验](../docs/14-memory-model.md#13-范围上限错误与分析完成状态)；不要把 `Incomplete` 的部分输出当作完整内存状态。

## 数值精度：集合以外还能知道什么

完成[第 02 课](../docs/02-domain.md)的集合手算后，用 `--no-relations` 隔离数值层，再比较 product 与 `--domain constants-only`。这三份输入都从未知 calldata 开始：

| 文件 | 手算条件 | 默认组合域的分支 | 仅有限集合的分支 |
| --- | --- | --- | --- |
| [known-bits-branch.hex](known-bits-branch.hex) | `(x AND 15) OR 1`，最低位为 1 | 只有 `BranchTrue` | `BranchTrue` 和 `BranchFalse` |
| [copy-identity.hex](copy-identity.hex) | 入口 calldata word 的同一个符号经 DUP1 复制，`x XOR x = 0` | 只有 `BranchFalse` | 只有 `BranchFalse` |
| [independent-inputs.hex](independent-inputs.hex) | 从偏移 0、32 分别读取 `x`、`y`，再 XOR | 两种分支 | 两种分支 |

```bash
nix run . -- cfg --file examples/known-bits-branch.hex --no-relations --context-depth 0
nix run . -- cfg --file examples/known-bits-branch.hex --no-relations --context-depth 0 --domain constants-only
nix run . -- explain --file examples/copy-identity.hex --context-depth 0
nix run . -- cfg --file examples/copy-identity.hex --context-depth 0 --domain constants-only
nix run . -- cfg --file examples/independent-inputs.hex --context-depth 0
```

三份输入都应 `Converged`。原始 calldata 的固定偏移读取具有稳定输入身份，因此第二份在两个 profile 中都能确认 XOR 的两个操作数相同；偏移 0、32 的符号不同，所以第三份仍保留两条分支。固定输入身份与本次基本块内的临时复制身份是两种机制，前者可跨控制流保留，并在 constants-only 中继续使用。这些符号名字只在同一组输入中表示关联，跨报告的同名符号不证明相等。完整实验与事实交换过程见[第 12 课](../docs/12-product-domains-facts.md)，默认输入和独立命名空间见[第 13 课](../docs/13-evm-environment.md)。

有限结果的完整枚举可以单独调整容量。下面读取未知 calldata word，再计算 Osaka 的 CLZ：

```bash
nix run . -- explain --hex 5f351e00 --no-relations --domain constants-only --max-constants 257
nix run . -- explain --hex 5f351e00 --no-relations --domain constants-only --max-constants 1000
```

CLZ 的完整结果为 `0..=256`，共 257 个候选。第一条命令为 `Converged`，完整保留它们；默认容量 8 则返回 Top，默认 product 仍可用其他组件保留范围和固定位约束。第二条的容量 1000 不再因配置上限被拒绝，但会在 CLZ 前耗尽单账户教学入口固定的 2000 万共享工作预算：输出 `status=Incomplete` 与 `Work` 前沿，显示 `SSA unavailable`，退出码为 2。`--max-constants` 接受运行平台能表示的任意正 `usize`，没有额外的 64 上限；参数不直接预分配容量，容量足够还须检查执行是否完成。

## Storage：先读懂一个账户的单元，再增加调用层级

这些 [`storage-*.json`](#storage先读懂一个账户的单元再增加调用层级) 与[上面的 memory 字节码](#内存从字节读写到抽象数组)分开，用 [第 16 课](../docs/16-storage-model.md)逐步观察。它们是 world 输入，入口都是 `0x0000000000000000000000000000000000000101`；单账户实验也需要 `--world`，文件夹名字不会改变输入格式。

| 文件 | 先手算什么 | 要核对的结果 |
| --- | --- | --- |
| [storage-basic.json](storage-basic.json) | A[0]=5、A[1]=7；覆盖 slot 0 为 42，再读两格 | 成功块出口为 `[42,7]`，初始 World 仍保留 5 |
| [storage-zero-default.json](storage-zero-default.json) | 未列出的 slot 在完整输入中为零 | 读 slot 0、1、2 得到 `[5,0,0]` |
| [storage-unknown-default.json](storage-unknown-default.json) | 相同代码、相同已知 slot，却不声明其他 slot 为零 | `[5,⊤,⊤]`；与[前一文件](storage-zero-default.json)分别看 default |
| [storage-write-unknown-initial.json](storage-write-unknown-initial.json) | 先读未知 slot 0，强写 7，再读一次 | `[⊤,7]`；写入不重绑定旧读值 |
| [storage-finite-alias.json](storage-finite-alias.json) | 未知 calldata 选择 slot 0 或 1；只写其中一个 | `--context-depth 0` 得到 `{4,9}×{7,9}`，额外组合不是具体执行见证 |
| [storage-symbolic-key.json](storage-symbolic-key.json) | 同一个未知 k 写入 7，再从 k 读取 | 未设置 calldata 时仍为 `{0,7}`；空 calldata 把 k 固定为 0 后为 7 |
| [storage-symbolic-value.json](storage-symbolic-value.json) | 键固定为 0，写入未知表达式 x+1，再比较读回值 | EQ 为 1；值未知与位置未知是不同问题 |
| [storage-callback-revert.json](storage-callback-revert.json) | A 先写 1，B 写 7并回调 A 写9，随后 B REVERT | 深层临时状态 `(A[0],B[0])=(9,7)`；根成功结束恢复为 `(1,4)` |
| [storage-transient.json](storage-transient.json) | persistent A[0]=5；transient 初始零，TSTORE 后读7 | `[5,0,7]`；两个 plane 的 slot 0 不是同一个单元 |

先运行第一项：

```bash
nix run . -- explain --world examples/storage-basic.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x
```

之后按[第 16 课](../docs/16-storage-model.md)的输入条件运行后续项：有限别名和符号键实验需要保留未知 calldata，不能把这条命令的 `--evm.calldata 0x` 直接复制到所有对照中。原始 word 如何拆成 memory 字节先读[第 14 课](../docs/14-memory-model.md)；新符号表达式、分支假设与 relation join 见[第 15 课](../docs/15-symbolic-relations.md)。

## 多账户：观察调用怎样影响返回值与状态

世界 JSON 提供多个账户的代码和初始事实，`--evm.to` 指定首先执行的账户。以下文件的入口统一为 `0x0000000000000000000000000000000000000101`。下面显式固定入口 caller=`0x...1000`、空 calldata 和 value=0，origin 默认与入口 caller 相同；子帧 caller 和 value 由调用指令决定。未指定的交易和区块字段仍为符号输入，省略入口的这三个参数也会扩大输入范围。运行后先查 `EVM inputs`，再用下面的具体成功路径理解输出。

先跑第一项：

```bash
nix run . -- explain \
  --world examples/worlds/call-return-branch.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x

nix run . -- explain \
  --world examples/worlds/returndata-copy.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x
```

A 调用 B，B 的返回数据让 A 选择分支。`explain` 默认显示捕获代码的反汇编、简明 CFG 与栈、每个 `O` 的结果概要，最后显示 `Verified cross-contract SSA:`。先在 `Transitions` 中找到 `Call` / `Return`，再看 `Outcomes` 中分别保留的结果；具体地址短引用的完整值在 `Addresses` 中，未知地址直接使用符号名字。SSA 用 `%结果 = 指令 %操作数` 展示值流，`T` 标识转移。需要完整环境字段、捕获帧与效果链时加 `--verbose`；需要查询 JSON 字段时改用 `analyze --format json`，再加 `--ssa` 可导出完成图的 SSA。JSON 为 `schema_version=3`，caller 和逻辑 `address_value` 的有类型格式见[第 09 课的代理实验](../docs/09-cross-contract.md#4-代理实验读谁的代码写谁的-storage)。

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

[`concrete.rs`](../crates/evm-abstract/tests/concrete.rs) 用 revm 核对[前六个单账户例子](#单账户先把栈和图读懂)的具体入口、边、出栈和值，覆盖 Cancun/Prague/Osaka 和 k=0/1/2/8/10。具体调用输入是 32 字节，前 31 字节均为 `00`：[diamond](diamond.hex)/[stack-heights](stack-heights.hex) 的末字节分别取 0 与 1；[dynamic-jump](dynamic-jump.hex) 的末字节取 4，确保走到合法目标并写 storage。[`osaka.rs`](../crates/evm-abstract/tests/osaka.rs) 核对 CLZ，并验证旧 fork 下的指令故障。新增的[三份数值精度输入](#数值精度集合以外还能知道什么)由完整门禁使用打包后的 CLI 执行 CFG、SSA；域、复制身份和反馈限制另有[组合域测试](../crates/evm-abstract/tests/product_domains.rs)。

[`cross_concrete.rs`](../crates/evm-abstract/tests/cross_concrete.rs) 对照多账户具体轨迹和账户效果；[`summaries.rs`](../crates/evm-abstract/tests/summaries.rs) 比较缓存开关后的最终关系；[`creation.rs`](../crates/evm-abstract/tests/creation.rs) 核对创建、延迟删除和原生调用。这些是指定样例的核对证据，范围见[第 06 课](../docs/06-boundaries.md)。

[`symbolic-conflicting-guards.hex`](symbolic-conflicting-guards.hex) 对同一 CALLVALUE 依次检查等于 1、等于 2；默认关系域排除两条真分支连成的矛盾路径。用 `--no-relations` 对照保守数值图，见[符号与关系域](../docs/15-symbolic-relations.md)。
