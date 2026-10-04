# 09：从多个账户到一次完整调用

本课从可观察结果开始：另一合约返回的字节怎样决定 caller 的分支；两个代理怎样共享实现代码，却保持各自的 storage；失败和重入怎样改变同一笔执行里的状态。

## 输入是一组明确的事实

下面是一个最小离线世界。它固定协议规则、事实来源和已观察到的账户。

```json
{
  "fork": "osaka",
  "provenance": "offline:lesson:v1",
  "accounts": [
    {
      "address": "0x0000000000000000000000000000000000000101",
      "code": "0x60015f5500",
      "storage": {"0x0": "0x0"},
      "storage_unknown": false,
      "balance": "0x0"
    }
  ]
}
```

`storage_unknown:false` 声明该账户初始 storage 完整，未列出的 slot 为零。省略这个字段时，未列出的 slot 为未知。省略 `balance` 时余额未知；省略 `code` 时代码未知。`code:"0x"` 表示代码确定为空。省略整个账户也表示未知；不能把“没有提供账户”理解为空账户。

`provenance` 是调用者提供的事实来源描述，输出原样保存；它本身不能建立链或区块身份。旧输入被标为 offline identity。新增 `identity:{kind:"chain",chain_id,block_hash}` 绑定固定快照；提供代码时必须同时校验 `code_hash`，可用 fingerprint 校验整组事实。`--rpc` 是显式的另一种输入来源；分析阶段不补查节点。详见[第 10 课](10-snapshots-summaries-creation.md)。这里的例子使用完整合成 storage，没有 RPC、交易发送或资金依赖；省略 nonce/存在性的旧例子仍保留这些未知事实。

示例入口选择 `0x101`。Osaka 的 [EIP-7951](https://eips.ethereum.org/EIPS/eip-7951) 在 `0x100` 放置 P256VERIFY 预编译；给这个地址装入普通 fixture 代码，不代表具体执行器会执行这些代码。分析器按 fork 选择原生预编译，具体可表示输入得到真实返回/失败，未知输入或无法预留的资源保留前沿。

```bash
nix run . -- analyze --world examples/worlds/call-return-branch.json --entry 0x0000000000000000000000000000000000000101 --format json
```

可指定 `--caller`、`--calldata 0x...`、`--value 0x...` 与 `--static`。这些参数属于入口帧；更深调用的参数由实际 CALL 指令生成。默认 calldata 为空，默认 value 为零，默认 caller 是 `0x...1000`。

世界余额是进入这个执行帧时的余额；如果外层交易已转入 value 或扣除 gas 费用，快照应反映这些变化。`--value` 单独提供 CALLVALUE，不会再次执行外层交易的转账或手续费处理。

## 先读调用边与返回边

`call-return-branch.json` 的账户 A=`0x...0101` 调用 B=`0x...0200`。在具体成功轨迹中，B 把数值 1 写到自己的内存，返回 32 字节。A 的 CALL 输出区得到这些字节，随后 `MLOAD(0)` 与 1 比较，走 true 分支，最终 A 的 storage slot 0 为 1。抽象图传播这条真实返回数据，同时保留 gas 不精确模型允许的失败分支；完整结果因此可能包含另一个 slot 0 值，不能把所有抽象边都当作必然成功轨迹。

```text
A: CALL B，暂停自己的帧
   ↓ Call
B: MSTORE(0, 1)；RETURN(0, 32)
   ↓ Return：成功位 1 + 返回字节
A: MLOAD(0) == 1；SSTORE(0, 1)
```

文本输出每个 `S` 状态都显示 `depth`、`code`、`address`、`caller`、`static` 和已访问 pc。`code` 是实际读取指令的账户；`address` 是 ADDRESS 和 storage 操作对应的账户。图中的 `Call`、`Return`、`Revert`、`Failure` 边连接整台抽象机器的状态，悬停 caller 的栈和内存仍属于机器状态。

JSON 的 `outcomes` 保存入口停止时的返回数据和状态。只找到 B 内部的一条 CFG，或把 CALL 结果直接设成 Top，都无法解释 A 的这次精确分支。

## 代理的两个地址不能混在一起

```bash
nix run . -- analyze --world examples/worlds/proxy-storage.json --entry 0x0000000000000000000000000000000000000101
```

控制账户分别调用代理 P1=`0x...0201`、P2=`0x...0202`；两者 DELEGATECALL 同一个实现 I=`0x...0300`。I 执行时 `code=I`，但 `address=P1` 或 `P2`。它把各自 slot 0 加 1，写 ADDRESS、CALLER 和 CALLVALUE 到 slot 1、2、3。下面列的是 revm 验证的具体成功轨迹；抽象状态还保留可能失败的调用效果，值集合可能同时包含初始值。

| 账户 | 初始 slot 0 | 最终 slot 0 | slot 1 ADDRESS | slot 2 CALLER | slot 3 CALLVALUE |
| --- | --- | --- | --- | --- | --- |
| P1 | 5 | 6 | P1 | 控制账户 | 7 |
| P2 | 9 | 10 | P2 | 控制账户 | 11 |
| I | 99 | 99 | 0 | 0 | 0 |

DELEGATECALL 继承 caller 和 call value，使用 caller 所在账户的 storage。`callcode-context.json` 把两个代理的指令换成 CALLCODE：仍使用代理 storage，但 CALLER 变为代理，CALLVALUE 取 CALLCODE 显式参数 3。代码身份、状态身份、caller 和 value 是四个需要分别记录的事实。

## 回滚与重入都使用同一份交易状态

`revert-rollback.json` 中 A 先写 slot 0=3；B 写自己的 slot 0=7，再用 REVERT 返回数值 42。返回 A 后，B 的 slot 0 恢复初始值 4；A 先前写入的 3 保留。CALL 成功位为 0，REVERT 的字节仍进入输出区，A 把 42 保存到 slot 1。失败的写入不能泄漏，也不能把整个 caller 一起回滚。

`static-write.json` 使用 STATICCALL。B 在第一次 SSTORE 处失败，回到 A 的成功位为 0，B 的初始 slot 0=4 保留。静态标志随子调用传播。

`log-rollback.json` 中 A 先发出一个事件，B 也发事件后 REVERT。最终只保留 A 的日志摘要；B 的事件随其写入一起回滚。日志摘要记录来源、topics 和数据，不约束日志顺序或次数。

`reentry.json` 中 A 先写 slot 0=1，再调用 B；B 再调用 A。重入帧读到的是当前交易状态里的 1。A 最后把 slot 0 改为 2，并在 slot 1 保存重入读到的 1。每次调用都重新读取最初快照，会把这个结果算错。

## 工作表保存整台机器

[`world.rs`](../crates/evm-abstract/src/world.rs) 保存初始事实；[`Store`](../crates/evm-abstract/src/world/store.rs) 保存执行中的 persistent/transient storage、余额、nonce、代码 overlay、生命周期和可能日志；[`machine.rs`](../crates/evm-abstract/src/analysis/machine.rs) 保存有序调用帧和图。每帧有独立 stack、memory、calldata、returndata、call value、继续位置和调用前的 Store。CALL 保存 checkpoint，成功返回提交当前 Store，REVERT 或故障恢复 checkpoint，再续接 caller。日志按来源指令摘要，保留 topics 与数据，顺序和次数不受该域约束。

状态键包含全部帧的代码账户/hash/执行模式、状态账户、caller、static 标志、块、栈高和帧内跳转历史，以及 Store 的代码/生命周期身份。相同键的 payload 才 join；这让共享实现、不同代理和不同暂停 caller 保持可区分。未知 slot 的写入使用弱更新，不能继续保留可能被覆盖的旧常量。

`--ssa` 让指令的栈值和整机效果进入同一份跨合约 IR；调用/返回转移显式传递帧与状态效果。验证器先要求分析完整，再核对状态、指令和边。单段 `cfg` / `ssa` 是学习这个机制的单账户入口；它们不替代完整世界中的调用语义。

## 未完成区域也是结果的一部分

```bash
nix run . -- analyze --world examples/worlds/missing-code.json --entry 0x0000000000000000000000000000000000000101 --format json
nix run . -- analyze --world examples/worlds/reentry.json --entry 0x0000000000000000000000000000000000000101 --max-call-depth 2
```

这两条命令退出 `2`，前者保留缺少 B 代码的前沿，后者保留重入深度前沿。`--max-work` 限制累计工作，`--max-memory-bytes` 限制每帧追踪内存；状态和 transfer 预算也覆盖所有账户。创建缺少 nonce/碰撞/initcode 等必要事实、未知预编译输入与未知调用目标同样产生有类型的前沿；摘要查找、认证或图导入耗尽累计预算会留下 `SummaryWork`。不能把这些情况当作一次完成的无副作用调用。

[`cross_concrete.rs`](../crates/evm-abstract/tests/cross_concrete.rs) 用独立 revm 对照这些离线世界在三个 fork 下的指令与帧身份、block 入栈、返回字节、最终 storage、合约余额和日志，逐个验证完整图的 SSA。oracle 必须实际访问入口字节码及子帧；预编译地址被当作普通账户时不能靠空轨迹通过检查。另有 CLI 与安装后二进制检查。有限样本能够发现反例；[模型边界](06-boundaries.md) 说明为什么模型内收敛仍不能直接证明合约安全。
