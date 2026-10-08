# 两条学习路线，同一套分析器

本教程围绕同一组问题展开：输入是什么，值如何表示，控制流如何传播，状态怎样读写，调用怎样返回，以及一份分析结果能证明什么。你可以从理论问题进入代码，也可以从一次命令的实现过程进入理论。两条路线都覆盖下面九个主题，复用同一份章节、示例和验收依据。

第一次使用仓库，先完成 [00：运行与输出](00-start.md) 的第一个实验。随后选择一条主线；章节编号用于定位正文，不要求按 00、01、02 的顺序全部读完。

只想学习一个具体问题，可以先选[专题模块](modules.md)。每个模块再拆成几个小单元，标明最少前置知识和可以停下的位置；在模块里仍可选择理论或实现视角。两条完整路线用于串联模块，不是进入模块前必须完成的课程。

| 路线 | 从什么问题开始 | 每一步怎样学 |
| --- | --- | --- |
| [理论路线：从语义与近似走到实现](routes/theory.md) | 一个摘要怎样代表许多执行，为什么合并后仍能使用这个结果？ | 先建立具体语义与抽象规则，再用仓库的实验、数据结构和精度边界检验理解 |
| [实现路线：跟着输入、状态与结果读代码](routes/implementation.md) | 一条 CLI 命令经过哪些模块，哪份状态在何处变化？ | 先追踪函数、类型和实际输出，再解释背后的抽象语义与成立条件 |

例如，同样学习 `MLOAD` 中断：理论路线先问“只完成部分步骤时，哪些结论仍有依据”；实现路线先追踪 `transfer::execute` 中取参数、检查范围和返回部分记录的位置。两条路线最终都要能解释 `OperandsConsumed`、没有结果名字的 `MLOAD`、剩余栈以及 `Memory` 前沿。

## 同一主题的两个入口

下面的两列是同一主题的不同读法。读到一半想切换视角，可以直接横向跳转，不必重读另一条路线的开头。

| 专题模块 | 理论入口 | 实现入口 | 共用正文与实验 |
| --- | --- | --- | --- |
| [执行规则与输入范围](modules/execution.md) | [具体执行的对象](routes/theory.md#execution) | [CLI 到字节码、世界和入口](routes/implementation.md#execution) | [00 运行](00-start.md)、[01 解码](01-bytecode.md)、[08 fork](08-forks.md)、[13 环境](13-evm-environment.md) |
| [数值摘要与组合域](modules/domains.md) | [集合、序与近似](routes/theory.md#domains) | [AbstractValue、NumericValue 与运算](routes/implementation.md#domains) | [02 抽象域](02-domain.md)、[12 事实交换](12-product-domains-facts.md) |
| [控制流、固定点与敏感性](modules/control-flow.md) | [传播为什么需要重访](routes/theory.md#control-flow) | [状态键、工作队列与合并](routes/implementation.md#control-flow) | [03 CFG](03-cfg.md)、[05 敏感性](05-sensitivity.md) |
| [值流、φ 与部分 SSA](modules/ssa.md) | [定义来源和覆盖范围](routes/theory.md#ssa) | [执行记录、SSA 构建与渲染](routes/implementation.md#ssa) | [04 SSA](04-ssa.md)，包括[未完成分析](04-ssa.md#7-未完成时按需查看部分-ssa) |
| [内存与 storage](modules/memory-storage.md) | [字节、槽位及别名](routes/theory.md#memory-storage) | [ByteArray、Store 与读写](routes/implementation.md#memory-storage) | [14 memory](14-memory-model.md)、[16 storage](16-storage-model.md) |
| [身份、路径条件与 SMT](modules/relations.md) | [关系保留了哪些关联](routes/theory.md#relations) | [表达式、约束与求解接口](routes/implementation.md#relations) | [15 符号关系](15-symbolic-relations.md) |
| [调用、状态归属与回滚](modules/calls-state.md) | [调用前后哪些状态相连](routes/theory.md#calls-state) | [调用帧、保存点与状态后端](routes/implementation.md#calls-state) | [09 跨合约](09-cross-contract.md)、[11 状态容器](11-state-backends.md) |
| [快照、复用与代码生命周期](modules/rpc-summaries.md) | [事实与摘要在何时有效](routes/theory.md#rpc-summaries) | [RPC 轮次、摘要和创建](routes/implementation.md#rpc-summaries) | [10 快照与摘要](10-snapshots-summaries-creation.md) |
| [模型边界与验证](modules/evidence.md) | [结果支持什么结论](routes/theory.md#evidence) | [反例、对照执行与检查](routes/implementation.md#evidence) | [06 边界](06-boundaries.md)、[07 练习](07-exercises.md)、[本地检查](local-ci.md) |

两条路线顺序不同：

```text
理论：执行规则 → 数值摘要 → 控制流 → SSA → 内存与 storage
                                      → 关系 → 调用 → 快照与复用 → 验证

实现：输入入口 → 调度与状态 → 数值运算 → 内存与 storage → 调用
                                      → RPC 与复用 → 关系 → SSA 输出 → 验证
```

理论路线较早讨论 SSA，用值的来源理解后面的状态读写；实现路线在看过执行记录如何产生后，再跟进 SSA 的构建。实现路线涉及尚未细读的类型时，先依据该阶段介绍理解其职责，再到后续主题检查内部规则。

## 怎样使用共用章节

每课顶部都有所属模块和两条路线中对应主题的链接。路线负责回答“为什么现在读这一部分，应该带着什么问题”；章节保存完整推导、命令、预期输出和源码入口。读完一个主题，回到当前路线继续下一步。旧的章节间链接仍可用于补足先修知识或查找相关实验。

练习也按主题使用：[第 07 课](07-exercises.md)里挑选当前主题的实验，先预测，再运行并解释差别。遇到 `%value`、`slot`、`S`、`B`、`F`、`T` 等输出记号，可回查 [04 的值与位置](04-ssa.md#1-先分清值名字和栈位置)和[部分 SSA](04-ssa.md#7-未完成时按需查看部分-ssa)。

判断自己是否完成一个主题，可以做三件事：用一个具体输入说明语义；在输出里定位相应事实；找到产生或验证这个事实的代码，并说明它的限制。例如看到 `Incomplete`，还需区分整张图未完成、某块尚未执行，以及某条指令只消费了参数。

## 按当前问题进入

| 你现在看到的现象 | 从哪里开始，再回哪条路线 |
| --- | --- |
| 一串 `%编号`、`partial phi`、`recorded prefix stack` | [04 的部分 SSA 实验](04-ssa.md#7-未完成时按需查看部分-ssa)，然后回 [理论 SSA](routes/theory.md#ssa) 或 [实现 SSA](routes/implementation.md#ssa) |
| 一个值变成 `⊤`，却仍显示 `Converged` | [06 的完成状态与精度](06-boundaries.md)，然后读[数值摘要](routes/theory.md#domains) |
| 内存或槽位写入后读取，为什么多出候选值 | [14 内存](14-memory-model.md)、[16 storage](16-storage-model.md)，然后比较两条路线的 `memory-storage` 主题 |
| RPC 取到了代码，仍出现未知调用或补查 | [10 的固定快照](10-snapshots-summaries-creation.md#可选实验从固定区块采集)，然后读 [RPC 实现](routes/implementation.md#rpc-summaries) |
| 准备修改 Rust 或提交 PR | [开发环境](development.md)、[本地完整检查](local-ci.md)，然后沿[实现路线](routes/implementation.md)定位受影响的状态与规则 |

[例子索引](../examples/README.md)列出可运行输入；[参考资料](references.md)按问题提供规范和理论来源；[动态派发检查](no-dynamic-dispatch.md)解释仓库的实现约束。回到 [README](../README.md#推荐阅读顺序)。
