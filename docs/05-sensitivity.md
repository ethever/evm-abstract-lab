# 05：敏感性就是“哪些执行暂时不合并”

本课目标：不用术语互相替换，能够指出本工具具体保留什么、丢失什么，以及增加上下文是否真的改变例子。

## 四个常被混淆的选择

| 维度 | 保留的区别 | 本仓库现状 |
| --- | --- | --- |
| 流敏感 | 指令顺序和各程序点的输入不同 | 块内顺序执行、块间独立状态，已实现 |
| 路径敏感 | 同一位置来自不同分支的约束/历史 | 已知条件剪枝；有限历史可分组；没有完整约束域 |
| 上下文敏感 | 相同代码在不同调用/控制来源下分别分析 | 可配置最近 k 个跳转来源，已实现；不是外部合约调用分析 |
| 栈高敏感 | 同一位置的不同入栈高度分别分析 | 总是区分，保证逐槽 join 与 SSA 有定义 |

有限集合域也能把不同值放在一个摘要里，但那不等于保留完整执行路径。尤其无法保留多个槽位之间的相关性。

## EVM 内部调用不一定使用 CALL opcode

Solidity 常把同一个合约里的内部子程序编译成“压入返回地址，然后 JUMP 到共享代码”。外部 CALL 才涉及另一段合约代码和新的执行环境。仅从 bytecode 推断函数边界与调用语义需要额外研究。

`internal-calls.hex` 故意使用最容易观察的内部约定：

```text
pc=00: PUSH return=05; PUSH helper=0e; JUMP
pc=05: JUMPDEST; POP 第一次结果
       PUSH return=0c; PUSH helper=0e; JUMP
pc=0c: JUMPDEST; STOP
pc=0e: JUMPDEST; PUSH 42; SWAP1; JUMP
```

helper 入口栈是 `[return_address]`。压 42 后变成 `[return_address,42]`，SWAP1 变成 `[42,return_address]`，JUMP 弹出返回地址，调用者收到 `[42]`。

## 先观察合并

```bash
cargo run --locked -p evm-abstract-cli -- cfg --file examples/internal-calls.hex --context-depth 0
```

pc=0e 只有一个状态。两次调用的入栈合并成 `{5,12}`，返回 JUMP 可指向两个返回点。这样会形成“第二次调用也可能返回第一次返回点”的伪路径，甚至在图中产生额外循环。

这是保守而不精确的结果：真实路径仍在图里，但一些组合没有对应真实执行。

## 再观察分离

```bash
cargo run --locked -p evm-abstract-cli -- cfg --file examples/internal-calls.hex --context-depth 1
cargo run --locked -p evm-abstract-cli -- ssa --file examples/internal-calls.hex --context-depth 1
```

进入跳转后继时，把来源块 start_pc 加入历史，保留最后 k 个；普通 fallthrough 不增加历史。

| pc=0e 的状态 | 历史 | 入口返回地址 |
| --- | --- | --- |
| k=0 的共享状态 | `[]` | `{5,12}` |
| k=1 的首次调用状态 | `[0]` | `{5}` |
| k=1 的第二次调用状态 | `[5]` | `{12}` |

helper 的两次执行现在分开了。分别构建 SSA，每个上下文有自己的 φ 与运算定义。k=0 和 k=1 的**字节码**完全相同，改变的是分析怎样划分执行状态。

## 为什么叫跳转历史，而不宣称完整调用串

这个 `--context-depth` 参数不识别内部函数，也不推断 JUMP 的 call/return 标签。条件分支和普通内部跳转都会更新历史。它是一种有界控制历史敏感性，可以在本例中作为调用来源近似，不能直接叫“完整 k-call-string 分析”。

外部 CALL 有另一套真实帧机制：主要 `analyze --world` 入口保存整条调用帧栈，每帧分别保留 code address、storage address、caller、call value、static 标志和局部跳转历史。CALL 暂停 caller；返回边恢复正确的继续位置。这里的 `k` 只控制各帧内部 JUMP 的历史，不会截断外部调用栈；外部栈由 `--max-call-depth` 预算控制，超限明确 `Incomplete`。见[跨合约一课](09-cross-contract.md)。

如果 helper 自己又经过多个分支，k=1 可能很快忘掉调用来源。提高 k 可能保留来源更久，但不保证消除全部伪路径，也不保证状态数只线性增长。

## 为什么不永远保留全部历史

循环每次增加历史，若不截断就有无限多个不同键；即使每个槽位域有限，状态空间也不再有同样的终止保证。k 有界才能保留有限结构空间。即使 k=3，状态数量也可能很多，所以预算与 Incomplete 仍是必要的。

本实现始终把 stack_height 放入键。运行 `explain --file examples/stack-heights.hex`，pc=0c 会有两个状态，入栈高分别为 0 和 1。用同一个向量强行 join 会丢槽位，也破坏 SSA；这里的区分既关乎精度，也关乎表示是否成立。

## 路径约束是下一种不同能力

设某条分支验证 `x == 5`，然后根据 x 跳转。当前域只根据条件是否可能零/非零决定保留哪些边；它不把 true 分支中的 x 自动收窄为 5。即使增加历史，若没有保存比较与原变量的联系，x 仍可能是 Top。

要做到这点，需要在状态中保留变量/谓词关系，或引入可解释的约束与假设 transfer。不要把“上下文深度变大”当作求解器，也不要把 SSA 名字本身当作路径证明。
