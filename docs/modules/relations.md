# 关系约束：什么依据允许排除一条分支？

[模块目录](../modules.md) · [理论视角](../routes/theory.md#relations) · [实现视角](../routes/implementation.md#relations) · [两条完整路线](../learning-routes.md)

本模块从“未知值也可以彼此关联”开始，解释身份、表达式、分支假设和局部证明怎样配合。目标是能为一条消失的 CFG 边指出证据，并分清这份证明没有覆盖哪些语义。

## 最小先修与学习范围

- [01 的 JUMPI 含义](../01-bytecode.md#4-检查-jump-与-jumpi-的真实含义)：条件非零走目标，零走下一块。
- [02 的抽象值](../02-domain.md#2-抽象值保存的是允许的可能性)：未知值是一组允许的数值。
- [13 的默认输入](../13-evm-environment.md#1-默认输入与目标账户)：省略 value 不是零；同一输入可以未知但固定。

前两个单元只用栈与分支，不要求先学 memory、storage、外部调用或 SSA 算法。第三单元补合并和身份范围，第四单元在需要解释模型限制时选读。

<a id="identity"></a>

## 基础单元：同样未知，不一定是同一个值

读[15 的五层信息](../15-symbolic-relations.md#2-五层信息分别保存什么)，并做该节“两次分别计算 x+1，再 EQ”的实验。两次读取同一 root calldata word x，ADD 都按 256 位回绕计算；两份相同输入和相同运算结构足以说明两个结果相等。

**理论角度：**数值摘要相同只说明允许的取值集合相同；Calldata 来源相同只说明来源类别。身份说明是否复用了同一个值，表达式还记录它如何由操作数计算出来。

**实现角度：**在 [`EvmValue<N>`](../../crates/evm-abstract/src/domain/value.rs) 中区分 numeric、provenance、identity、expression；再到 [`ExprId`](../../crates/evm-abstract/src/domain/symbolic.rs) 找输入叶与 operation。完整路径条件另在 `RelationState`，不属于某个单值。

**观察与停点：**正文默认关系模式下 EQ 得 `{1}`，关闭关系后得 `{0,1}`。能说明差别来自保留派生表达式，即可继续；固定输入身份和 DUP 复制规则在关闭关系后仍可存在。

**自检：**两个不同 calldata 偏移都有 Calldata 标签，能否用它证明两次读取相等？同一 pc 在两次独立执行中产生的未知结果，又能否直接复用一个身份？

<a id="assumptions"></a>

## 基础单元：让同一个 v 不能既等于 1 又等于 2

做[15 的矛盾条件实验](../15-symbolic-relations.md#1-先看同一个输入不能同时等于-1-和-2)，输入也保存在 [`symbolic-conflicting-guards.hex`](../../examples/symbolic-conflicting-guards.hex)。先手算 `v=1` 后继，再检查那里第二次比较 `v=2` 的 true 边。

**理论角度：**身份提供“还是同一个 v”，路径假设提供“在这里 v=1”。两项合起来才能排除第二次 true；仅知道比较结果在 `{0,1}` 中还不够。

**实现角度：**顺着 [`relations::branch`](../../crates/evm-abstract/src/analysis/transfer/relations.rs) 进入 [`RelationState::assume` / `check`](../../crates/evm-abstract/src/domain/relational.rs)。true/false 后继各保存自己的假设；`refine` 再把已证明的后果反馈到数值摘要。完整过程见[指令转换与精化](../15-symbolic-relations.md#3-指令转换assume-和数值精化)。

**观察与停点：**按正文 JSON 查询，B₂ 的出边只有 `BranchFalse`，入口 v 的常量为 `{1}`；关闭关系后两条分支都保留。两份分析都可 `Converged`，边多是精度差别，不自动表示传播未完成。

**自检：**为什么未证明矛盾不等于已有一笔可执行的完整 EVM 交易？为什么不能在缺少矛盾证据时删除分支？完成这两问就已达到本模块的基础目标。

<a id="join-scope"></a>

## 进阶单元：合并与复用时，哪些保证必须放弃？

先补[02 的 join](../02-domain.md#3-join把两条路径的信息合在一起)，再读[15 的关系合并](../15-symbolic-relations.md#4-join-和-widening-不能漏掉路径)。只考虑左路 `x=1`、右路 `x=2`：汇合要覆盖两路，不能强迫 x 同时等于 1 和 2。

**理论角度：**路径内部的条件合取与路径之间的可能性合并方向不同。共同保证是普通 RelationState join 的结果；整机 guarded join 另用一个 selector 保存完整替代。数值投影可能看不到配对，循环 widening 也明确丢弃部分保证，不能将三者混为同一操作。

**实现角度：**先读 [`RelationState::join`](../../crates/evm-abstract/src/domain/relational.rs) 与 [`join_covers_both_paths_instead_of_conjoining_their_conditions`](../../crates/evm-abstract/src/domain/relational/tests.rs)：x=1、x=2 都不能被排除。再到 [`machine/join.rs`](../../crates/evm-abstract/src/analysis/machine/join.rs) 跟踪同一个 selector 如何覆盖栈、byte/slot 根、rollback store 与完整路径公式。循环规则与验证范围见[第 17 课](../17-symbolic-state.md#汇合固定点与摘要来源)。

随后读[作用域与 fresh 身份](../15-symbolic-relations.md#5-作用域与摘要重放)的前半部分。同名 Caller 不证明不同环境的输入相等，不同身份也不证明它们数值不等。源码测试 `distinct_input_scopes_remain_independent_not_disequal` 检查后一个区别。

**观察与停点：**能区分独立数值集合、共同保证和完整 guarded alternatives，并给出一个身份误关联的错误例子。摘要重放的 fresh 重命名等到[快照与复用模块](rpc-summaries.md)再学，不必先掌握调用子图。

<a id="limits"></a>

## 可选单元：状态关系怎样参与局部推理，何时仍会未知？

如果要解释写后读，先完成[内存与 storage 的基础单元](memory-storage.md#memory-basics)，再读[15 的状态索引关系](../15-symbolic-relations.md#有值的表达式不等于有符号索引数组)。同一账户的 `SSTORE(k,7); SLOAD(k)` 不需要先枚举 k：SlotStateId 保存写后根，SlotRead 使用同一个键，默认例子得到 7。

**理论角度：**身份回答“地址是否还是 k”，有序根回答“读的是写入之前还是之后”。两项合起来才可用 `select(store(S,k,7),k)=7`。若随后向 q 写入，还需区分 q=k 与 q≠k；不同来源或不同 hash 输入不能直接证明不等。没有局部证明也不等于条件不成立。

**实现角度：**[`EvmByteArray<N>`](../../crates/evm-abstract/src/world/bytes.rs) 与 [`EvmStore<N>`](../../crates/evm-abstract/src/world/store/slots.rs) 建立 EVM 状态根；[`RelationState`](../../crates/evm-abstract/src/domain/relational.rs) 保留局部事实和路径条件；[`relations::refine`](../../crates/evm-abstract/src/analysis/transfer/relations.rs) 再将已证明的上界反馈到数值投影。长度、范围、账户和回滚规则仍由状态实现负责。

再读[局部推理与工作预算](../15-symbolic-relations.md#7-局部推理与工作预算)，找到当前报告的表达式/约束上限、status 和 frontiers。`max_work` 是抽象工作量，不是秒数或 EVM gas。

**观察与停点：**能分别解释数值投影较宽、widening 放弃保证、局部推理规则边界和表示容量不足。可选查询未完成可记录精度诊断，执行覆盖不完整则保留 frontier；不用把所有 Unknown 都读成同一种结果。结论范围继续读[证据模块](evidence.md)。

最终验收：选默认关系模式排除的一条边，写出输入身份、已有假设、新增条件及排除依据；再写出一项该结论没有证明的 EVM 行为。
