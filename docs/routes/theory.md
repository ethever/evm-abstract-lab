# 理论优先：从覆盖具体执行，到判断分析证据

只想学习当前专题，可从[模块总览](../modules.md)选择范围；下面每站也有独立模块入口。模块中的核心单元可以先学，扩展部分按需继续。

这条路线先提出一个分析问题，再用仓库里的程序检验答案，最后找到实现这条规则的代码。你不需要先学完抽象解释、编译器或 SMT；每一步只引入解释当前现象所需的概念。目标是能回答：**分析器保存了什么，合并时忘了什么，得到的结果足以支持哪种结论？**

它与[实现优先路线](implementation.md)学习同一套内容，但推进方向不同。这里先确定一条规则为什么必要，再检查数据结构与算法怎样实现它；另一条路线先追踪实际产物，再提出实现背后的理论问题。每步末尾都能切换到另一条路线的同一主题。

操作命令、完整输出和详细推导集中保留在各课正文。本页负责安排阅读与实验：先写预测，运行正文实验，再解释差异；不要只核对最终数字。实验沿用正文的 fork、调用输入和分析参数，省略输入、传入零与空字节有不同含义。

路线顺序是：[执行对象](#execution) → [抽象域](#domains) → [控制流与固定点](#control-flow) → [SSA 与证据覆盖](#ssa) → [内存和存储](#memory-storage) → [符号与关系](#relations) → [调用和状态版本](#calls-state) → [快照、摘要和生命周期](#rpc-summaries) → [验证结论](#evidence)。回到[学习路线总览](../learning-routes.md)或[仓库 README](../../README.md)可以重新选择入口。

<a id="execution"></a>

## 第一步：先确定“一次执行”究竟由什么决定

本主题的独立模块：[字节码、协议与输入](../modules/execution.md)。

分析许多执行之前，先要说清楚其中一次执行。字节码规定指令，栈保存中间数值，memory 保存调用内的临时字节；调用环境提供 caller、value、calldata 等输入，fork 决定采用哪套指令规则。这些条件共同决定你想覆盖哪些行为。只给一段代码，通常还不足以确定一条具体轨迹。

从最小的 `PUSH1 2; PUSH1 3; ADD` 开始：栈按底到顶经历 `[] → [2] → [2,3] → [5]`。先会手算这一条轨迹，才有依据检查“一槽可能是几个值”的抽象结果。

1. 按[第 00 课的手算与三种输出](../00-start.md#第二步先手算这个程序)，运行 [`straight-line.hex`](../../examples/straight-line.hex)。在纸上连起同一条 ADD 的原始字节、pc、栈变化和 SSA 名字；先把 SSA 当作值的名字，第四步再解释构造规则。
2. 做[第 01 课的 `60 5b 00` 实验](../01-bytecode.md#1-先手算三字节程序)。在每个字节下面写“操作码”或“立即数”，解释为什么中间的 `5b` 不能作为跳转目标。接着读[截断 PUSH](../01-bytecode.md#2-push-的立即数缺一半时怎么办)和[切块规则](../01-bytecode.md#3-把指令切成基本块)，把“字节位置”和“可执行位置”分开。
3. 用[第 08 课的 CLZ 对照](../08-forks.md#2-手算-clz-怎样算出跳转地址)观察同一字节在不同 fork 下的结果，再做[第 13 课的离线默认输入实验](../13-evm-environment.md#先用离线命令检查默认关联)。分别记录协议规则与环境假设：默认 caller 和 origin 是同一输入，省略 calldata 则保持未知，不能当作空字节。读到[委托代码](../08-forks.md#4-eip-7702账户的-code-可能是委托标记)时，先记下“代码来源账户可能不同于状态账户”，第七步再展开。

源码从 [`bytecode.rs`](../../crates/evm-abstract/src/bytecode.rs)、[`fork.rs`](../../crates/evm-abstract/src/fork.rs) 和 [`world/environment.rs`](../../crates/evm-abstract/src/world/environment.rs) 进入，分别寻找指令边界、协议选择与 `AddressInput` / `EvmEnvironment`。环境有符号身份，不等于它已经有具体数值；fork 默认值也不证明它适用于任意链上区块。

**完成问题：** 两次分析用相同字节码，一次省略 calldata，一次明确传入空 calldata，它们在分析同一组调用吗？如果 CLZ 在某个 fork 下异常终止，而分析报告 `Converged`，为什么这两件事可以同时成立？

下一步把多条具体轨迹概括成一份状态；也可以转到[实现路线的执行入口](implementation.md#execution)。

<a id="domains"></a>

## 第二步：用什么摘要代表许多可能值

本主题的独立模块：[数值摘要与组合域](../modules/domains.md)。

如果分支一产生 1，分支二产生 2，汇合后的一槽可以写作 `{1,2}`。这里的集合代表“任取其中一个”，不是栈上有两个数。抽象域规定可以保存哪些摘要，以及怎样计算和合并它们。**覆盖**要求摘要包含相关的具体结果；增加不可能的候选会损失精度，删除一个真实候选则会破坏覆盖。

先只理解两个操作：`join` 合并不同路径的可能性，`transfer` 计算执行指令后的摘要。例如 `{1} join {2} = {1,2}`，再加 `{10}` 得 `{11,12}`。表示能力有上限时，需要扩大摘要：常量集合装不下所有候选便变成 Top，表示任意 word，不能任意删几个候选。

1. 按[第 02 课的有限集合实验](../02-domain.md#1-从可能是-1也可能是-2开始)，用 [`diamond.hex`](../../examples/diamond.hex) 手填汇合入口与 ADD 出口。沿用正文关闭关系层、使用 constants-only 的参数，随后按 [join 容量实验](../02-domain.md#3-join把两条路径的信息合在一起)把容量改为 1，解释为什么数值变粗而执行仍可继续。
2. 读[逐槽合并丢失配对关系](../02-domain.md#5-为什么摘要会包含实际不会出现的组合)：`[1,10]` 和 `[2,20]` 合为 `[{1,2},{10,20}]` 后，写出多出的组合。带着这个问题读[第 12 课的组件表](../12-product-domains-facts.md#3-几个组件约束同一个-word)：位、区间和同余共同约束一个 word，却不会自动记住两个槽位来自同一条路径。
3. 手算[范围与同余交换](../12-product-domains-facts.md#4-信息交换从范围和整除性找回候选)：`1≤x≤20` 且 x 是 8 的倍数，候选为 `{8,16}`。这叫规约：把已有约束相互传递。再读[约束取交与路径取并](../12-product-domains-facts.md#5-同一值的约束取交不同路径的可能取并)，用一句话解释为什么这两处不能都使用“交集”。

源码依次看 [`FiniteConstantSet::join`](../../crates/evm-abstract/src/domain/finite_constant_set.rs)、[`NumericValue`](../../crates/evm-abstract/src/domain/numeric.rs)、[`AbstractValue`](../../crates/evm-abstract/src/domain/value.rs)，再看 [`Domain`](../../crates/evm-abstract/src/domain.rs) 与 [`reduce.rs`](../../crates/evm-abstract/src/domain/reduce.rs)。区分常量组件为 Top 和整个数值没有约束；工作表 join 与有轮数限制的 facts 交换也是不同操作。当前实现没有声称每次都求得理论上最精确的组合域结果。

**完成问题：** 常量组件无法枚举，却知道一个值最低位为 1，能否判断它非零？把容量从 1 提高到 8，为什么仍不能自动恢复 `[1,10]` 与 `[2,20]` 的配对关系？

下一步让这些摘要沿程序传播；对照[实现路线的值表示与转换](implementation.md#domains)。

<a id="control-flow"></a>

## 第三步：为什么要反复传播，哪些执行应该先分开

本主题的独立模块：[CFG、固定点与敏感性](../modules/control-flow.md)。

EVM 的跳转目标来自栈，因此“有哪些边”和“栈里有哪些可能值”互相影响：数值分析发现跳转边，新边又带来新的输入。工作表算法把入口发生变化的状态放进队列，重新执行并传播，直到不再产生需要处理的信息。这个稳定状态称为固定点。

循环不应靠“执行过一次”就停止。有限集合会在容量边界扩大为 Top；组合域对反复扩大的区间使用 widening，把继续外移的端点扩大到极值，减少逐个扩大范围的工作。两者都可能损失精度。状态、transfer 与累计工作预算仍可能先耗尽，此时必须保留未完成前沿。

1. 对照[第 03 课的工作表](../03-cfg.md#3-手动走完一次工作表传播)，用 diamond 逐轮写出队列和汇合入口；再运行[循环实验](../03-cfg.md#4-循环的固定点长什么样)的 [`loop.hex`](../../examples/loop.hex)。指出“再次处理这个 σᵖᵢ”的原因，并区分字节码块 Bᵢ 与分析状态 σᵖᵢ。
2. 阅读[未知跳转的处理](../03-cfg.md#6-跳转目标未知时仍须保留后续行为)，用 [`dynamic-jump.hex`](../../examples/dynamic-jump.hex)检查候选目标为什么仍须来自合法 JUMPDEST。目标值变粗可以增加边；额外边表示抽象可能，不能单凭它声称存在走通整条路径的输入。
3. 按[第 05 课的内部调用实验](../05-sensitivity.md#2-先手算两次内部调用)，比较 [`internal-calls.hex`](../../examples/internal-calls.hex) 在历史深度 0 和 1 下的 helper。写出“同一块、同一栈高、不同跳转来源”怎样变成不同 σᵖᵢ，再读[历史遗忘](../05-sensitivity.md#5-历史如何更新又如何失忆)。这里的敏感性决定哪些输入暂时分开，并没有恢复完整的 Solidity 函数或真实 CALL 栈。

源码看 [`analysis/engine.rs`](../../crates/evm-abstract/src/analysis/engine.rs) 的 `successor`：先 join，检测严格变化，按策略 widening，再使旧执行证据失效并重新排队；单程序状态键可对照 [`analysis.rs`](../../crates/evm-abstract/src/analysis.rs)。阅读引擎结束处时留意：完成状态还检查是否有 frontier，不能只拿“队列空了”当作 `Converged` 的充分解释。完整机器状态还包括调用帧和共享 Store，第七步补齐。

**完成问题：** 增大跳转历史深度与增加常量容量分别保留哪种区别？一个入口已经执行过，后来由 `{1}` 扩大成 `{1,2}`，为什么旧出口不能立即作为新入口的计算结果？

下一步利用这个“旧证据可能失效”的观察理解 SSA；对照[实现路线的工作表与状态分组](implementation.md#control-flow)。

<a id="ssa"></a>

## 第四步：值有了名字以后，怎样判断名字的证据范围

本主题的独立模块：[SSA 与部分执行](../modules/ssa.md)。

SSA 在静态表示中给每个值定义分配一个唯一名字，让使用者能追溯定义。循环可以反复经过同一静态定义，不需要按运行次数无限增加名字。`DUP` 复制一个已有名字，`SWAP` 改变位置；名字不等于栈槽，也不等于该值的数值摘要。汇合处的 φ 按前驱连接不同名字，例如“从左边来用 `%a`，从右边来用 `%b`”。它描述来源选择，本身不会替数值域求出常量或替关系层证明相等。

本仓库的完整 SSA 构建要求分析图已经完成，随后检查前驱、定义与使用。部分 SSA 则给实际保留的执行证据命名，同时暴露未覆盖部分。第三步出现的“入口扩大但还没重访”正是重要边界：旧执行所定义的出口不能冒充当前入口的结果。

1. 用[第 04 课的名字、栈位置与 φ](../04-ssa.md#1-先分清值名字和栈位置)回看 straight-line 与 diamond，再读[循环命名](../04-ssa.md#4-循环为什么不需要无限多个名字)。手画一条“指令定义 → 后续使用 → 汇合 φ”的链，并核对[构建与验证步骤](../04-ssa.md#5-本仓库如何构建和检查-ssa)。
2. 从[第 04 课第 7 节的块内停止实验](../04-ssa.md#从块中途停下的实际输出开始)入手。对 [`partial-ssa-memory.json`](../../examples/partial-ssa-memory.json)分别采用正文的 1 字节和 32 字节 memory 上限，观察 `recorded prefix stack` 与完整 `stack out`。两次数字都可能是 1，但前一次是尚未参与 ADD 的 `%0`，后一次是 ADD 的结果 `%3`。沿[块前缀解释](../04-ssa.md#为什么基本块可以只执行一个前缀)找到 MLOAD 已弹出偏移却尚未产生结果的位置。
3. 再做[未知调用目标实验](../04-ssa.md#先运行一个未知调用目标)。保留同一组符号输入与 `Incomplete` / `UnknownTarget`，观察加入 `--allow-partial-ssa` 后多了哪些可读值流。CALL 参数已有名字，不代表它已经产生成功位或返回数据；CALL 本身结束静态基本块，与 MLOAD 在块内中途停止是两个观察。
4. 按下面四项读完该节的细节，再用正文 JSON 查询把编号连回机器报告。这里的“部分”既涉及图还有哪些边，也涉及一条指令究竟执行到哪一步。

| 要检查的证据 | 本次阅读要回答的问题 | 正文入口 |
| --- | --- | --- |
| `Current` / `Stale` / `Unexecuted` | 最近执行是否对应当前入口，还是入口已扩大，或根本尚未执行？ | [状态证据](../04-ssa.md#每个状态的证据是否仍适用) |
| `Started`、`OperandsConsumed` 等 progress | 只到达 pc、已消费参数、已完成普通效果，还是进入分发或故障；效果名字对应哪一步？ | [指令阶段](../04-ssa.md#一条指令已经走到哪一步)、[进度与效果链](../04-ssa.md#进度和效果链怎样一起读) |
| partial φ、deferred edges、open incoming | 哪些入边有当前证据，哪些仍无法接入，图是否还可能发现新前驱？ | [部分 φ 的覆盖](../04-ssa.md#部分-φ-的输入覆盖哪些边) |
| σᵖᵢ / σᵢ / Tᵢ 编号与 `state_mapping` | 名字对应局部状态还是原生机器状态；边编号能否直接当作数组下标？ | [JSON 编号连接](../04-ssa.md#保存-json-并连接状态编号) |

源码对照 [`ssa/build.rs`](../../crates/evm-abstract/src/ssa/build.rs) 的完整构建入口与 [`ssa/partial.rs`](../../crates/evm-abstract/src/ssa/partial.rs) 的覆盖类型，阶段证据来自 [`analysis/machine/evidence.rs`](../../crates/evm-abstract/src/analysis/machine/evidence.rs)。完整构建仍拒绝 `Incomplete`；部分结构验证也不会补齐未知调用、证明全部路径覆盖，或把退出码 2 改成成功。

**完成问题：** 一个块是 `Current`，为什么其中最后一条 CALL 仍可能未覆盖所有目标？`open incoming=[]` 又为什么不足以证明一个 `Incomplete` 图的 φ 已完整？

下一步看名字流经 memory 和 storage 后还剩下哪些联系；对照[实现路线的 SSA 产物](implementation.md#ssa)。

<a id="memory-storage"></a>

## 第五步：保存值还不够，还要知道读写的是哪个位置

本主题的独立模块：[Memory & Storage](../modules/memory-storage.md)。

栈槽按位置排列，memory 和 storage 则需要额外的地址。memory 按字节寻址，MLOAD/MSTORE 处理连续 32 字节；storage 按“账户、256 位 slot”寻址，每个 slot 保存一个 word。两个地址可能指向同一位置，叫作可能别名。知道写入值为 7，并不能自动知道哪些单元被替换。

唯一位置可以强更新，直接替换旧值；多个候选位置需要覆盖“这次写中了它”和“这次没写它”，即弱更新。若原来 `A[0]=4, A[1]=7`，写入位置可能是 0 或 1，新值是 9，摘要需要保留 `A[0]∈{4,9}`、`A[1]∈{7,9}`。逐位置保存又会丢掉两者必须来自同一选择的关系，这与第二步的逐槽合并是同一个精度问题。

1. 做[第 14 课写入 42 再读出](../14-memory-model.md#2-第一个实验写入-42再读出来)，画出最后一个字节为何是 `2a`；随后做[有限地址写入实验](../14-memory-model.md#6-地址确定时覆盖地址有几个候选时合并)的 [`memory-write-alias.hex`](../../examples/memory-write-alias.hex)。先列两行具体结果，再列逐字节摘要，指出 MLOAD 重新组合时多出了什么。
2. 按[第 16 课的 slot 实验](../16-storage-model.md#2-第一个实验替换-slot-0读取-slot-0-和-slot-1)区分初始事实与当前 Store，再手算[两候选 slot 的弱更新](../16-storage-model.md#5-有两个可能的-slot为什么要保留旧值)。同时检查[未列出的 slot](../16-storage-model.md#没列出的-slot-是零还是未知)：是否为零取决于初始 world 的完整性声明。
3. 比较[同一未知 memory 偏移写后读](../14-memory-model.md#同一个未知偏移写后再读也未必恢复原值)与[同一未知 storage 键写后读](../16-storage-model.md#6-无法列出-slot-候选刚写入再读为什么是-07)。前者的 [`memory-symbolic-offset.hex`](../../examples/memory-symbolic-offset.hex)得到 Top，后者的 [`storage-symbolic-key.json`](../../examples/storage-symbolic-key.json)得到 `{0,7}`。两者都保留了栈上地址的相同身份，却没有普遍保存符号位置的写后读关系；按正文固定输入后再观察强更新。

源码看 [`ByteArray::write_values` / `read_word`](../../crates/evm-abstract/src/world/bytes.rs) 与 [`Store` 内的 `Plane::write` / `read`](../../crates/evm-abstract/src/world/store.rs)。当前表示以具体索引映射和默认值为基础。未知 memory 写入会粗化字节事实，未知 slot 写入则合并该账户的默认值和显式单元。通用符号数组中的 `select(store(S,k,7),k)=7` 表示“在 k 写后再读 k 得到 7”；这是理解缺失关系的理论记号，不是当前已经具备的通用数组能力。

**完成问题：** 既然 DUP 保留了同一个 k，为什么 SSTORE(k,7) 后 SLOAD(k) 仍可能含零？增加单值的常量容量、延迟路径合并、保留符号索引更新关系，分别可能改善哪一步？

下一步检查关系层怎样帮助、又不能自动补回哪些信息；对照[实现路线的 memory/storage 读写](implementation.md#memory-storage)。

<a id="relations"></a>

## 第六步：从“值未知”走到“这些未知值必须满足什么关系”

本主题的独立模块：[符号关系与 SMT](../modules/relations.md)。

一个数未知，不代表关于它什么都不知道。读取同一个输入两次，可以知道它们相等；计算 `y=x+1` 可以保存运算关系；进入 `x==1` 的真分支后，可以保存当前路径上的 `x=1`。数值摘要、值身份、表达式与路径约束分别回答这些问题，不能用一个“符号值”概念把它们混在一起。

SMT 在这里检查已经编码的条件能否同时成立。若同一个 x 已经等于 1，再要求它等于 2，条件矛盾，后继可以删除。求解结果 Unknown 表示没有完成相应判断，需要保留路径和限制证据；SAT 也只说明当前编码可满足，不能直接充当完整 EVM 轨迹的执行见证。

1. 做[第 15 课的两次 CALLVALUE 判断](../15-symbolic-relations.md#1-先看同一个输入不能同时等于-1-和-2)，比较关系开启与关闭时 pc=`0x12` 是否出现。写下每条分支新增的条件，解释删除后继用的是“同一个输入”加“当前路径条件”这两项知识。
2. 读[五层信息](../15-symbolic-relations.md#2-五层信息分别保存什么)和 [`assume` / 数值精化](../15-symbolic-relations.md#3-指令转换assume-和数值精化)，给第二步的数值摘要、第四步的 SSA 名字各找位置。再用正文两次计算 `x+1` 的例子区分表达式相等、来源标签相同与数值摘要相同。
3. 阅读[关系 join](../15-symbolic-relations.md#4-join-和-widening-不能漏掉路径)、[作用域](../15-symbolic-relations.md#5-作用域与摘要重放)与[符号索引数组边界](../15-symbolic-relations.md#有值的表达式不等于有符号索引数组)。左路 `x=1`、右路 `x=2` 汇合时，不能把两者直接合取；当前实现保留共同保证，也不自动保存任意精确析取。将这个规则带回第五步，说明为什么求解器不能凭空找回数组已经忘记的写入关系。

源码看 [`domain/symbolic.rs`](../../crates/evm-abstract/src/domain/symbolic.rs) 的 `ExprId`、[`domain/relational.rs`](../../crates/evm-abstract/src/domain/relational.rs) 的 `assume` / `join` / `unique_value`，再接到 [`analysis/transfer/relations.rs`](../../crates/evm-abstract/src/analysis/transfer/relations.rs)。运算遵守 256 位 EVM 规则；表达式节点、深度、关系数量与求解资源都有上限，见[第 15 课资源预算](../15-symbolic-relations.md#7-原生后端与确定性资源预算)。固定输入身份有环境作用域，跨报告同名 Caller 不能直接建立相等关系。

**完成问题：** 关系层证明 k=0 时，怎样帮助现有 storage 映射变精确？若只能证明两次 k 相同，为什么还不能声称具备通用的符号数组写后读规则？

下一步把这些值和约束放进真实的调用层次；对照[实现路线的表达式与路径条件](implementation.md#relations)。

<a id="calls-state"></a>

## 第七步：调用暂停了谁，失败又该恢复哪一个状态

本主题的独立模块：[调用、状态归属与回滚](../modules/calls-state.md)。

过程间分析需要同时描述调用者的暂停状态、被调用者的执行和返回后的继续位置。A 调用 B 时，机器从 `[A 活动]` 变成 `[A 暂停, B 活动]`；每个帧有独立操作数栈和 memory，各帧共享当前 Store。第三步区分跳转来源的上下文参数，并不能代替这条真实调用栈。

回滚也是语义问题：B 失败时，应恢复进入 B 之前的整份 Store，包括撤销 B 更深调用的效果，而保留 A 先前已经完成的写入。保存旧版本的容器负责让恢复成为可能，执行器负责决定何时恢复哪一层；更换容器不应改变这条规则。

1. 跟随[第 09 课的返回值实验](../09-cross-contract.md#1-第一个实验b-返回-1a-写入-1)，用 [`call-return-branch.json`](../../examples/worlds/call-return-branch.json)连起 A 的 CALL、B 的 RETURN 与 A 的 SSTORE。读[帧的内容](../09-cross-contract.md#3-一次调用需要保存哪些东西)后，标出 memory、calldata、returndata 与 Store 各属于哪一层。
2. 读[代理实验](../09-cross-contract.md#4-代理实验读谁的代码写谁的-storage)和[重入实验](../09-cross-contract.md#6-重入新帧读到当前状态)，分别回答“执行谁的代码、使用谁的状态”和“重入读初始快照还是当前写入”。再用[第 16 课的回调后回滚](../16-storage-model.md#9-revert-恢复的是调用入口检查点不是固定初始快照)，手算 `(A[0],B[0])` 怎样从 `(1,4)` 变为 `(9,7)`，又恢复到 `(1,4)`。
3. 最后读[第 11 课的状态后端](../11-state-backends.md#从同一份-store-切换底层实现)。按[先核对语义再观察时间](../11-state-backends.md#先核对语义再观察时间)理解 std 与 imbl 的对照方式：检查点、恢复与 join 的含义固定，复制和共享数据的成本可以不同。先阅读比较的输入与结果合同；需要测性能时再运行正文的测量脚本。

源码沿 [`analysis/machine/frame.rs`](../../crates/evm-abstract/src/analysis/machine/frame.rs) 的帧数据进入 [`analysis/transfer/calls.rs`](../../crates/evm-abstract/src/analysis/transfer/calls.rs)，查看调用建立保存点、结束时恢复或保留 Store 的位置，再读 [`snapshot-state`](../../crates/snapshot-state/src/lib.rs) 与 [`Checkpoint`](../../crates/snapshot-state/src/checkpoint.rs)。当前模型仍可能保留调用失败分支；未知目标、缺失代码和调用深度限制须按[第 09 课的前沿](../09-cross-contract.md#7-输入缺失和预算停止也要读出来)阅读，不能由一个已知成功结果代替未覆盖行为。

**完成问题：** 回调 A 已成功写入 9，随后 B REVERT，为什么 A 仍应恢复成进入 B 前的 1？如果只给 B 保存它自己账户的 storage，哪一步会错？

下一步确定调用事实从哪里来、何时可以复用；对照[实现路线的跨合约状态与回滚](implementation.md#calls-state)。

<a id="rpc-summaries"></a>

## 第八步：哪些事实必须固定，哪些结果才允许复用

本主题的独立模块：[RPC、快照与调用摘要](../modules/rpc-summaries.md)。

前七步都依赖输入假设。把来源换成 RPC 后，假设必须指向同一条链、同一个固定区块；否则先读的代码和后读的 slot 可能来自不同状态。补查得到的是初始事实，程序执行中的写入仍属于 Store。两者混用，会让刚写入的 7 被旧快照的 4 覆盖。

调用摘要则回答另一种复用问题：已完成的 B 分析，能否在新的调用位置继续使用？前提不仅是代码相同，还包括它读到的状态、环境、关系和分析策略。摘要需要保留结束方式、返回字节、Store 效果及其子图；这些部分属于同一结果关系，不能各自挑选后拼装。

1. 先读[第 10 课的快照身份](../10-snapshots-summaries-creation.md#5-固定快照同一个名字不代表同一组事实)与[补查规则](../10-snapshots-summaries-creation.md#为什么补查也必须固定区块)，画出“固定一次区块 hash → 取得初始事实 → 分析 → 补查有限缺失事实 → 从入口重跑”的顺序。结合[第 16 课的初始值依赖](../16-storage-model.md#11-rpc-补查的是初始事实不是当前交易值)，解释强写过的 slot 为何直接读当前值。RPC 实验是可选的；没有节点时也可阅读本地 HTTP 测试来核对这些规则。
2. 做[摘要开启与关闭实验](../10-snapshots-summaries-creation.md#第一步分别开启和关闭摘要)的 [`summary-reuse.json`](../../examples/worlds/summary-reuse.json)，先检查 `published` 与 `hits`，再按[最终关系比较](../10-snapshots-summaries-creation.md#第二步确认复用没有改变最终关系)比较 outcome。阅读[命中前提](../10-snapshots-summaries-creation.md#什么条件下允许命中)后，写出 B 的 slot 从 7 变成 9 时为什么旧摘要失效；回到第六步的符号作用域，理解重放为何还要给内部 fresh 值重新命名。
3. 用[CREATE 实验](../10-snapshots-summaries-creation.md#2-create先执行构造代码再安装运行时代码)的 [`create-runtime.json`](../../examples/worlds/create-runtime.json)画出 initcode 执行、安装 runtime、随后 CALL 的顺序；再看[SELFDESTRUCT 的时间表](../10-snapshots-summaries-creation.md#3-selfdestruct转账和删除发生在不同时间)和[identity 预编译实验](../10-snapshots-summaries-creation.md#4-预编译没有普通字节码也有调用帧)。这三项检查同一个问题：当前可执行代码与账户生命周期来自哪一层事实，是否能只靠初始地址和代码 hash 判断行为？

源码把三组入口连起来：[`world/rpc/session.rs`](../../crates/evm-abstract/src/world/rpc/session.rs) 与 [`analysis/rpc.rs`](../../crates/evm-abstract/src/analysis/rpc.rs)固定采集并重跑；[`analysis/summary.rs`](../../crates/evm-abstract/src/analysis/summary.rs)认证并复用闭合子图；[`analysis/transfer/create.rs`](../../crates/evm-abstract/src/analysis/transfer/create.rs)与 [`world/store.rs`](../../crates/evm-abstract/src/world/store.rs)管理创建和当前代码。采集直接信任普通 RPC 观察，不请求状态证明；重跑、摘要认证和导入继续消耗共享预算，不能以复用为由重置预算，也不能为未完成子图补发完整摘要。

**完成问题：** 为什么一个固定 block hash 既不能代表完整的输入事实，也不能代替当前 Store？只比较两次运行的最终 slot 数值，又为什么不足以验证摘要复用正确？

下一步将所有观察收束成可检查的结论；对照[实现路线的 RPC、摘要与生命周期](implementation.md#rpc-summaries)。

<a id="evidence"></a>

## 第九步：为结论挑选足够的证据

本主题的独立模块：[边界、证据与验证](../modules/evidence.md)。

学习抽象分析最终要能判断一份报告允许你说什么。`Converged` 说明当前模型和输入范围内的传播已经完成；一个抽象候选可能由合并产生；一个具体样本被覆盖则需要独立执行轨迹与抽象结果对应。三者分别需要证据，不能用“程序退出 0”或“看见了正确的一个数”统包。

1. 先完成[第 06 课的四种边界实验](../06-boundaries.md#1-先读完成状态再读图)，把每份报告记录为“输入范围、是否完成、精度损失、未完成原因”。再按[三种结论与证据](../06-boundaries.md#3-三种结论需要三种证据)重写其中一份实验结论，明确它支持的是当前分析完成，还是某个具体样本被覆盖。
2. 从[第 07 课](../07-exercises.md)回做[容量](../07-exercises.md#3-改容量同一程序为何变得不精确)、[未知跳转副作用](../07-exercises.md#5-未知跳转后副作用不能消失)、[值未知和未完成](../07-exercises.md#7-分辨值未知和分析没做完)与[部分 SSA 记录范围](../07-exercises.md#13-判断部分-ssa-究竟记录到了哪里)四个练习。每题先预测，再运行，再寻找源码依据。读进阶设计题时，区分题目建议进一步改善的能力与当前源码已经实现的行为。
3. 按[测试分层表](../06-boundaries.md#6-测试分别核对了什么)选一条结论，阅读真正验证它的断言：例如有限集合 join 的规律看 [`tests/domain.rs`](../../crates/evm-abstract/tests/domain.rs)，具体 EVM 指令和轨迹看 [`tests/concrete.rs`](../../crates/evm-abstract/tests/concrete.rs)，跨合约效果看 [`tests/cross_concrete.rs`](../../crates/evm-abstract/tests/cross_concrete.rs)。后者要求同一个抽象 outcome 覆盖具体返回数据和状态效果，不能从几个不相容 outcome 中各取一块答案。

准备修改代码时，再进入[开发环境](../development.md)、[本地完整门禁](../local-ci.md)与[静态派发约定](../no-dynamic-dispatch.md)。这些文档规定怎样让实现变更可复现、可交付；有限测试通过仍不等于所有合约的普遍性证明。需要阅读理论或协议原文时，从[参考资料](../references.md)挑与当前实验对应的一项，带着已经能手算的问题查原文。

**完成问题：** 假设报告为 `Converged`，某个 slot 含 `{0,7}`，你能否断言存在一条具体轨迹把它写成零？反过来，具体执行返回 7，抽象报告里也出现 7，还需核对哪些调用边、返回字节、状态效果和输入假设，才算这个样本被覆盖？

完成路线后，选一个已有例子，用一页笔记连起“输入与协议 → 抽象表示 → 传播与合并 → SSA 证据 → 最终结果与边界”。其中每个判断都应能指回一次实验、一条源码规则或一个测试断言。需要从实现重新验证这条链，可转到[实现路线的证据与交付](implementation.md#evidence)；需要选下一轮重点，回到[学习路线总览](../learning-routes.md)。
