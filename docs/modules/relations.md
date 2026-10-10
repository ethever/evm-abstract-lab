# 关系约束：什么依据允许排除一条分支？

[模块目录](../modules.md) · [理论视角](../routes/theory.md#relations) · [实现视角](../routes/implementation.md#relations) · [两条完整路线](../learning-routes.md)

本模块从“未知值也可以彼此关联”开始，解释身份、表达式、分支假设和求解结果怎样配合。目标是能为一条消失的 CFG 边指出证据，并分清这份证明没有覆盖哪些语义。

## 最小先修与学习范围

- [01 的 JUMPI 含义](../01-bytecode.md#4-检查-jump-与-jumpi-的真实含义)：条件非零走目标，零走下一块。
- [02 的抽象值](../02-domain.md#2-抽象值保存的是允许的可能性)：未知值是一组允许的数值。
- [13 的默认输入](../13-evm-environment.md#1-默认输入与目标账户)：省略 value 不是零；同一输入可以未知但固定。

前两个单元只用栈与分支，不要求先学 memory、storage、外部调用或 SSA 算法。第三单元补合并和身份范围，第四单元在需要解释模型限制时选读。

<a id="identity"></a>

## 基础单元：同样未知，不一定是同一个值

读[15 的五层信息](../15-symbolic-relations.md#2-五层信息分别保存什么)，并做该节“两次分别计算 x+1，再 EQ”的实验。两次读取同一 root calldata word x，ADD 都按 256 位回绕计算；两份相同输入和相同运算结构足以说明两个结果相等。

**理论角度：**数值摘要相同只说明允许的取值集合相同；Calldata 来源相同只说明来源类别。身份说明是否复用了同一个值，表达式还记录它如何由操作数计算出来。

**实现角度：**在 [`AbstractValue`](../../crates/evm-abstract/src/domain/value.rs) 中区分 numeric、provenance、identity、expression；再到 [`ExprId`](../../crates/evm-abstract/src/domain/symbolic.rs) 找输入叶与 operation。完整路径条件另在 `RelationState`，不属于某个单值。

**观察与停点：**正文默认关系模式下 EQ 得 `{1}`，关闭关系后得 `{0,1}`。能说明差别来自保留派生表达式，即可继续；固定输入身份和 DUP 复制规则在关闭关系后仍可存在。

**自检：**两个不同 calldata 偏移都有 Calldata 标签，能否用它证明两次读取相等？同一 pc 在两次独立执行中产生的未知结果，又能否直接复用一个身份？

<a id="assumptions"></a>

## 基础单元：让同一个 v 不能既等于 1 又等于 2

做[15 的矛盾条件实验](../15-symbolic-relations.md#1-先看同一个输入不能同时等于-1-和-2)，输入也保存在 [`symbolic-conflicting-guards.hex`](../../examples/symbolic-conflicting-guards.hex)。先手算 `v=1` 后继，再检查那里第二次比较 `v=2` 的 true 边。

**理论角度：**身份提供“还是同一个 v”，路径假设提供“在这里 v=1”。两项合起来才能排除第二次 true；仅知道比较结果在 `{0,1}` 中还不够。

**实现角度：**顺着 [`relations::branch`](../../crates/evm-abstract/src/analysis/transfer/relations.rs) 进入 [`RelationState::assume` / `check`](../../crates/evm-abstract/src/domain/relational.rs)。true/false 后继各保存自己的假设；`refine` 再把已证明的后果反馈到数值摘要。完整过程见[指令转换与精化](../15-symbolic-relations.md#3-指令转换assume-和数值精化)。

**观察与停点：**按正文 JSON 查询，B₂ 的出边只有 `BranchFalse`，入口 v 的常量为 `{1}`；关闭关系后两条分支都保留。两份分析都可 `Converged`，边多是精度差别，不自动表示传播未完成。

**自检：**为什么 SAT 只说明当前编码可满足，不能直接证明一笔完整 EVM 交易能走通？Unknown 为什么不能被当成 UNSAT 删除分支？完成这两问就已达到本模块的基础目标。

<a id="join-scope"></a>

## 进阶单元：合并与复用时，哪些保证必须放弃？

先补[02 的 join](../02-domain.md#3-join把两条路径的信息合在一起)，再读[15 的关系合并](../15-symbolic-relations.md#4-join-和-widening-不能漏掉路径)。只考虑左路 `x=1`、右路 `x=2`：汇合要覆盖两路，不能强迫 x 同时等于 1 和 2。

**理论角度：**路径内部的条件合取与路径之间的可能性合并方向不同。当前关系 join 保留共同保证，会忘掉部分条件关联；它不保证生成精确的任意析取。

**实现角度：**阅读 [`RelationState::join`](../../crates/evm-abstract/src/domain/relational.rs)，再找 [`join_covers_both_paths_instead_of_conjoining_their_conditions`](../../crates/evm-abstract/src/domain/relational/tests.rs) 的断言：join 后不是 bottom，x=1 与 x=2 都未被排除。这是一个可直接核对的覆盖要求。

随后读[作用域与 fresh 身份](../15-symbolic-relations.md#5-作用域与摘要重放)的前半部分。同名 Caller 不证明不同环境的输入相等，不同身份也不证明它们数值不等。源码测试 `distinct_input_scopes_remain_independent_not_disequal` 检查后一个区别。

**观察与停点：**能给出“共同保证丢失”和“身份误关联”各一个错误例子即可。摘要重放的 fresh 重命名等到[快照与复用模块](rpc-summaries.md)再学，不必先掌握调用子图。

<a id="limits"></a>

## 可选单元：关系知道什么，数组与预算还缺什么？

如果要解释写后读，先完成[内存与 storage 的两个基础单元](memory-storage.md#memory-basics)，再读[15 的符号索引边界](../15-symbolic-relations.md#有值的表达式不等于有符号索引数组)。对同一个 k 写 7 再读，表达式身份只能说明 k 没换；当前数组表示还可能忘掉该位置的更新关系。

**理论角度：**证明 k=0 可以把操作变成具体位置的读写；仅证明两次 k 相同，不能让没有符号数组模型的实现自动使用 `select(store(S,k,7),k)=7`。同样，求解过程没有完成也不等于条件不成立。

**实现角度：**[`relations::refine`](../../crates/evm-abstract/src/analysis/transfer/relations.rs) 调用数值投影与唯一值查询；[`relational/scalar.rs`](../../crates/evm-abstract/src/domain/relational/scalar.rs) 处理可直接提取的单值事实，[`relational/solver.rs`](../../crates/evm-abstract/src/domain/relational/solver.rs) 提供定宽编码。它们不会替 [`ByteArray` / `Store`](../../crates/evm-abstract/src/world.rs) 新建通用符号索引数组。

再读[资源预算与后端](../15-symbolic-relations.md#7-原生后端与确定性资源预算)，找到当前报告的 provider、rlimit、status 和 frontiers。rlimit 的单位随求解器而异，不能直接读成秒数或 EVM gas。

**观察与停点：**能把“表达能力不够”“合并忘掉关系”“查询资源不足”分别归因即可；不必为学习本模块切换全部后端或提高所有预算。结果结论的范围可继续读[证据模块](evidence.md)。

最终验收：选默认关系模式排除的一条边，写出输入身份、已有假设、新增条件及排除依据；再写出一项该结论没有证明的 EVM 行为。
