# 控制流与敏感性：值怎样长成一张图

[模块总览](../modules.md) · [两条学习路线](../learning-routes.md) · [理论路线同主题](../routes/theory.md#control-flow) · [实现路线同主题](../routes/implementation.md#control-flow)

程序只有几个基本块，为什么分析会有更多状态，甚至反复执行同一个状态？本模块追踪“入口信息变化 → 指令转换 → 后继传播”的过程，再检查哪些执行被合并、未知目标如何继续，以及什么时候能称为完成。

最小先修是[基本块](../01-bytecode.md#3-把指令切成基本块)、[JUMP/JUMPI](../01-bytecode.md#4-检查-jump-与-jumpi-的真实含义)和[有限集合 join](../02-domain.md#3-join把两条路径的信息合在一起)。能手算 `{1} join {2}={1,2}` 就足以开始；无需先读完整数值域、SSA 或外部 CALL 模型。

每单元只追踪一张小图。理论读法先解释为何需要这条传播规则，实现读法先找状态键、队列或后继安装分支，两者最终核对同一份实验记录。

<a id="states"></a>

## 1. 分开字节码块和分析状态

**问题：**Bᵢ 是代码位置，σᵖᵢ 又多保存了什么？同一个 Bᵢ 可以有几个 σᵖᵢ？

从 [`diamond.hex`](../../examples/diamond.hex) 的[手算 CFG](../03-cfg.md#1-先看一张能手算的图)开始，按[代码块与分析状态](../03-cfg.md#2-分清代码块-b-与分析状态-s)给每个 σᵖᵢ 找到所属 Bᵢ 和起始 pc。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 状态分组决定哪些输入先合并。同一代码位置的不同栈高不能直接逐槽 join，保留的控制历史也可使输入分组。 | 在 [`StateKey`](../../crates/evm-abstract/src/analysis.rs) 中找块索引、入口栈高、context；再看原生 [`MachineKey`](../../crates/evm-abstract/src/analysis/machine.rs) 如何包含调用帧结构。 |

**完成产物：**画一张“σᵖᵢ → Bᵢ → pc”的对应表。追加[栈高对照实验](../05-sensitivity.md#6-四种敏感性不是同一个能力)的 [`stack-heights.hex`](../../examples/stack-heights.hex)，指出同一 pc 上两个 σᵖᵢ 为什么必须分开。

<a id="worklist"></a>

## 2. 入口扩大后，为什么要重新执行

**问题：**一个状态已经执行过一次，后来又收到新值，可不可以直接保留旧出口？

按[diamond 工作表](../03-cfg.md#3-手动走完一次工作表传播)逐轮写队列，再运行[循环固定点实验](../03-cfg.md#4-循环的固定点长什么样)的 [`loop.hex`](../../examples/loop.hex)。沿用正文 constants-only、关闭关系和历史深度 0 的设置。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 新入口可能产生新出口和边，需要反复传播直到稳定。集合容量和区间 widening 会扩大摘要以控制增长；它们不保证实际预算总够用。 | 在 [`Engine::successor`](../../crates/evm-abstract/src/analysis/engine.rs) 找 join、变化检测、widen、旧凭据失效和重新排队；`Engine::run` 负责取出队列项并执行。 |

**完成产物：**解释 loop 为何只有 3 个状态却有 11 次 transfer，以及入口变成 Top 后为何仍可 `Converged`。对 product 的 widening，只需先理解[继续外移的区间端点被扩大](../02-domain.md#6-为什么循环不会要求无限扩大的集合)；不能把此处的 11 次直接套用于另一策略。

<a id="sensitivity"></a>

## 3. 两次进入同一个 helper，要不要先合并

**问题：**一个 helper 的两个调用来源混在一起，会多出什么返回路径？

按[内部调用的手算](../05-sensitivity.md#2-先手算两次内部调用)运行 [`internal-calls.hex`](../../examples/internal-calls.hex)，比较[历史深度 0](../05-sensitivity.md#3-k0两次输入合并出现额外路径)与[历史深度 1](../05-sensitivity.md#4-k1保留最后一个来源把两次调用分开)。这里使用压返回地址再 JUMP 的约定，没有外部 CALL 指令。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 敏感性通过暂缓合并保留区别，可以减少由不相容输入拼出的伪路径。它与单个槽的数值表示能力不同。 | 在 [`transfer.rs`](../../crates/evm-abstract/src/analysis/transfer.rs) 的跳转处理中追踪 history 更新，再看 [`StateKey`](../../crates/evm-abstract/src/analysis.rs) 如何呈现 context。 |

**完成产物：**列出 helper 的每个 σᵖᵢ、入口返回地址和后继。解释 k=0 时额外的返回组合，以及[更多跳转如何挤掉旧历史](../05-sensitivity.md#5-历史如何更新又如何失忆)。k=1 改善了本例，不等于恢复全部函数边界或完整调用串。

<a id="unknown-jumps"></a>

## 4. 目标未知时，哪些边必须保留

**问题：**不能列出跳转目标的完整集合，是不是只能停止？图中有候选边，又是否证明存在真实路径？

用[未知跳转实验](../03-cfg.md#6-跳转目标未知时仍须保留后续行为)的 [`dynamic-jump.hex`](../../examples/dynamic-jump.hex)定位 pc=`0x09` 的 SSTORE，再按[诊断与前沿](../03-cfg.md#7-看清诊断与未完成前沿)比较未知目标和资源停止。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 保守传播要覆盖尚不能排除的合法目标；候选边可能来自近似，不能当作具体路径见证。预算停止则表示仍有行为没有展开。 | 在 [`transfer.rs`](../../crates/evm-abstract/src/analysis/transfer.rs) 找 JUMP/JUMPI 候选筛选，合法位置来自 [`Program`](../../crates/evm-abstract/src/bytecode.rs)；在 [`Engine::run`](../../crates/evm-abstract/src/analysis/engine.rs) 末尾看 frontiers 如何决定 status。 |

**完成产物：**解释本例为什么同时有 `UnknownJump` 诊断和通向 SSTORE 的边。若候选展开耗尽预算，报告还应保留什么？依据[精度与预算参数](../05-sensitivity.md#7-实验时分别调整精度与预算)，选择一个与问题对应的调整方向，不能只说“把所有上限调大”。

## 学到哪里可以停

完成前两单元，就能跟踪工作表并解释重访；完成全部四单元，就能比较状态分组、候选边与未完成前沿。`Converged` 只说明当前模型与输入下的传播完成；它既不保证每条抽象路径都可具体执行，也不是合约安全结论。

需要追踪值的定义，转到 [SSA](ssa.md)，尤其查看入口扩大后旧证据怎样失效；需要解释分支条件如何排除路径，转到[符号关系](relations.md)；需要真实外部调用栈，转到[调用与状态](calls-state.md)。[第 05 课的四种敏感性](../05-sensitivity.md#6-四种敏感性不是同一个能力)可作进阶对照，但有界跳转历史不能代替这些不同能力。
