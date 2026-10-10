# 执行规则与输入：明确这次在分析什么

[模块总览](../modules.md) · [两条学习路线](../learning-routes.md) · [理论路线同主题](../routes/theory.md#execution) · [实现路线同主题](../routes/implementation.md#execution)

你手里只有一串字节时，哪些信息已经确定，哪些执行条件仍未知？本模块的目标是写出一张准确的“分析输入卡”：代码从哪里来，按哪套规则解释，哪些调用输入已限定。它可以独立学习，无须先走完整条理论或实现路线。

最小准备是会在仓库根目录运行[第 00 课的第一个命令](../00-start.md#第一步准备运行环境)。如果还不熟悉栈，只补读[手算 `2+3`](../00-start.md#第二步先手算这个程序)：本仓库按栈底到栈顶显示，右侧先弹出。本页复用正文命令和预期输出。

四个单元共同建立输入卡。想从理论进入，就先回答表中的语义问题；想从实现进入，就先找到右侧类型或函数，再用同一个例子核对规则。

<a id="decode"></a>

## 1. 一串字节中，哪些位置真的是指令

**问题：**`60 5b 00` 有三个字节，为什么只有两条指令？若 PUSH 声明的立即数不够长，又该怎样解释？

使用[三字节程序](../01-bytecode.md#1-先手算三字节程序)与[截断 PUSH](../01-bytecode.md#2-push-的立即数缺一半时怎么办)两个实验。先逐字节标出操作码和数据，再核对反汇编。

| 理论看什么 | 实现查什么 |
| --- | --- |
| pc 是字节偏移；PUSH 的数据不能作为可独立执行的 opcode。缺失立即数字节按 EVM 规则补零。 | 在 [`Program::decode_with_fork`](../../crates/evm-abstract/src/bytecode.rs) 中找立即数读取、补齐与 pc 前进规则。 |

**完成产物：**给 `60 5b 00` 和 `61 ab` 各写一行“指令起点 → 指令 → 下一 pc”。解释第二段为何压入 `0xab00`，而非 `0x00ab`。可把反汇编与[直线程序的栈表](../00-start.md#第二步先手算这个程序)逐项对应。

<a id="blocks"></a>

## 2. 找到基本块以后，是否已经知道执行顺序

**问题：**切出一个 Bᵢ 块，是否就证明它可达？看到 `5b`，是否就能把它当作 JUMP 目标？

读[切块规则](../01-bytecode.md#3-把指令切成基本块)和[JUMP/JUMPI 手算](../01-bytecode.md#4-检查-jump-与-jumpi-的真实含义)，在 [`diamond.hex`](../../examples/diamond.hex) 中标出基本块起点与分支终点。先使用正文反汇编，不急着推导全部抽象状态。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 基本块描述顺序指令边界；实际后继还依赖栈上的目标和条件。JUMPI 条件为零时走顺序后继。 | 在 [`Instruction::ends_block` 与 `Program`](../../crates/evm-abstract/src/bytecode.rs) 中分别定位切块规则与合法 JUMPDEST 集合。 |

**完成产物：**写出一组 Bᵢ 编号与起始 pc 的对应关系，并解释“B₁”为什么不等于“pc=1”。指出 CALL 后面为什么也需要块边界；调用暂停与恢复留给[调用模块](calls-state.md)。

<a id="fork"></a>

## 3. 同一份字节，换规则会怎样

**问题：**改变 fork，与改变常量容量，是不是同一种实验？

做[CLZ 的三种 fork 对照](../08-forks.md#2-手算-clz-怎样算出跳转地址)，使用 [`osaka-clz.hex`](../../examples/osaka-clz.hex)。先手算 CLZ(1)，再比较 pc=`0x02` 的指令行为和后继。

| 理论看什么 | 实现查什么 |
| --- | --- |
| fork 改变被分析程序的执行规则；精度参数改变分析怎样概括既定规则下的可能性。无效指令也有明确的终止语义。 | 在 [`Fork::supports_clz`](../../crates/evm-abstract/src/fork.rs) 与 [`Instruction`](../../crates/evm-abstract/src/bytecode.rs) 的有效性判断中核对版本选择。 |

**完成产物：**为 Osaka、Cancun、Prague 各写一行“CLZ 是否有效、能否到达后续 JUMPDEST、分析为何可能完成”。默认 fork 固定在项目配置中，不能据此判断任意链上区块采用的规则；核对方式见[选择如何贯穿输出](../08-forks.md#5-怎样确认选择贯穿了整个分析)。

<a id="environment"></a>

## 4. 代码固定后，还有哪些输入没有固定

**问题：**省略 calldata、传入空 calldata、设置 caller，分别改变了什么？

按[默认输入与目标账户](../13-evm-environment.md#1-默认输入与目标账户)读一份环境记录，再做[离线 caller/origin 对照](../13-evm-environment.md#先用离线命令检查默认关联)。使用正文的跨块 EQ 例子，分别观察默认关联和显式不同 origin。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 未指定的输入表示允许范围，不能自行补零。两个未知值仍可来自同一个输入；未知数值与身份相同并不矛盾。 | 在 [`EvmEnvironment::default` / `resolved_origin`](../../crates/evm-abstract/src/world/environment.rs) 核对未知 calldata、value 与默认 caller/origin 关联，再看 [`EvmArgs::environment`](../../crates/evm-abstract-cli/src/evm.rs) 怎样接收显式值。 |

**完成产物：**按[JSON 环境字段位置](../13-evm-environment.md#在-json-中查输入范围)给一份报告填写“代码、fork、to、caller、origin、value、calldata”输入卡。逐项写“具体值”“允许范围”或“与哪个输入相同”，并解释为什么 world/RPC 的账户初始事实不能代替调用环境。

## 学到哪里可以停

完成前两单元，就能读反汇编与基本块；完成全部四单元，就能准确描述一次分析的输入范围。下一步可独立选择[数值摘要](domains.md)或[控制流](control-flow.md)，不必先理解跨合约分析。

进阶问题按需进入：[EIP-7702 的代码委托](../08-forks.md#4-eip-7702账户的-code-可能是委托标记)接到[调用模块](calls-state.md)；[快照与执行环境的区别](../13-evm-environment.md#3-固定快照与执行环境)接到[RPC 与摘要模块](rpc-summaries.md)；[gas 和索引 hash](../13-evm-environment.md#4-索引-hash-与-gas)只在当前程序读取这些输入时展开。`Converged` 表示在声明的输入与模型下完成传播，仍不等于具体交易成功或合约安全。
