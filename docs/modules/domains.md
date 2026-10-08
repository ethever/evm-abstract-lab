# 数值摘要与组合域：一个槽位能保存多少知识

[模块总览](../modules.md) · [两条学习路线](../learning-routes.md) · [理论路线同主题](../routes/theory.md#domains) · [实现路线同主题](../routes/implementation.md#domains)

输出里一槽是 `{1,2}`、`⊤` 或一串位约束，它们各自允许哪些数？本模块从单个 word 出发，学习合并、运算、组件交换，以及这些操作丢掉的信息。完成后应能解释一个值为什么变粗，而不把所有原因都归结为“分析失败”。

最小先修只有[栈的手算](../00-start.md#第二步先手算这个程序)和[省略输入的含义](../13-evm-environment.md#1-默认输入与目标账户)。不必先学 CFG 算法；下文 diamond 只需知道两条路径在一处汇合，位条件实验只需知道[JUMPI 按零/非零选择后继](../01-bytecode.md#4-检查-jump-与-jumpi-的真实含义)。

每个单元用同一个实验连接两种读法：理论侧先写“这个摘要覆盖什么”，实现侧先找“哪个字段和操作保存这项保证”。命令、参数与完整输出沿用链接正文。

<a id="coverage"></a>

## 1. 用一槽概括多个可能值

**问题：**`[{1,2}]` 与 `[{1},{2}]` 为什么不是一回事？Top 又是不是没有值？

读[第 02 课的 diamond 栈表](../02-domain.md#1-从可能是-1也可能是-2开始)，运行 [`diamond.hex`](../../examples/diamond.hex) 的 constants-only 对照。沿用正文的 `--no-relations` 和历史深度 0，先隔离有限集合的作用。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 一个抽象值覆盖多个可能的具体 word；Top 允许任意 word。未到达状态与一槽的 Top 不是同一概念。 | 在 [`FiniteConstantSet`](../../crates/evm-abstract/src/domain/finite_constant_set.rs) 看 Top 与非空有限集合，再在 [`AbstractValue`](../../crates/evm-abstract/src/domain/value.rs) 看数值之外的元数据。 |

**完成产物：**写出汇合前、汇合后与 ADD 后的栈高和候选值。结合[抽象值的含义](../02-domain.md#2-抽象值保存的是允许的可能性)，解释“没有对应状态”为什么还要先检查分析是否未完成，才能讨论不可达。

<a id="join-transfer"></a>

## 2. 合并与运算分别怎样保留可能性

**问题：**两个前驱到达时为何取并集，ADD 为何要检查输入组合？候选太多时能不能只保留前几个？

做[join 容量实验](../02-domain.md#3-join把两条路径的信息合在一起)，比较 diamond 的容量 1 与默认容量；再用[SUB 手算](../02-domain.md#4-transfer在摘要上执行指令)核对栈顶先弹出的顺序。

| 理论看什么 | 实现查什么 |
| --- | --- |
| join 覆盖任一前驱；transfer 覆盖输入组合的计算结果。超过表示容量时扩大摘要，不能删除真实候选；算术须遵守 EVM word 回绕规则。 | 在 [`FiniteConstantSet::join`](../../crates/evm-abstract/src/domain/finite_constant_set.rs)、[`Domain::finite_apply`](../../crates/evm-abstract/src/domain.rs) 与 [`concrete::evaluate`](../../crates/evm-abstract/src/domain/concrete.rs) 分别核对合并、枚举和具体运算。 |

**完成产物：**手算 `{1,2}+{10,20}`，再解释容量不足为何可能得到 Top。另将 `[1,10]` 与 `[2,20]` 逐槽合并，列出[额外允许的配对](../02-domain.md#5-为什么摘要会包含实际不会出现的组合)：增加容量为什么不能自动修复这项关系损失？

<a id="product"></a>

## 3. 列不完候选，还能知道哪些性质

**问题：**一个未知输入经过 `(x AND 15) OR 1`，为什么能证明结果非零？

运行[位条件对照](../12-product-domains-facts.md#1-先手算这个条件可能为零吗)的 [`known-bits-branch.hex`](../../examples/known-bits-branch.hex)。两份命令都关闭关系层，观察 product 与 constants-only 的 false 边差别；然后手算[范围与同余交换](../12-product-domains-facts.md#4-信息交换从范围和整除性找回候选)的 `1≤x≤20` 且 x 是 8 的倍数。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 位、区间和同余同时约束同一个 word，其含义取交；不同路径的可能性仍需取并。规约把已有保证传给其他组件，不会凭空增加输入事实。 | 在 [`NumericValue`](../../crates/evm-abstract/src/domain/numeric.rs) 对应组件字段，沿 [`domain/transfer.rs`](../../crates/evm-abstract/src/domain/transfer.rs) 与 [`reduce.rs`](../../crates/evm-abstract/src/domain/reduce.rs) 追踪运算和 facts 交换。 |

**完成产物：**指出常量组件为 Top 时，哪项位保证仍排除零；为范围/同余例子写出候选 `{8,16}`。用[组件约束与路径 join](../12-product-domains-facts.md#5-同一值的约束取交不同路径的可能取并)解释为什么这两个地方不能都使用交集。

<a id="limits-identity"></a>

## 4. 判断精度来自数值、身份，还是更多工作

**问题：**两个值都来源于 calldata，就能证明它们相等吗？多做几轮 facts 交换能补回任意关系吗？

比较[同一输入与独立输入实验](../12-product-domains-facts.md#2-再手算两个未知值一定相同吗)的 [`copy-identity.hex`](../../examples/copy-identity.hex) 和 [`independent-inputs.hex`](../../examples/independent-inputs.hex)，再读[交换停止与执行未完成](../12-product-domains-facts.md#6-交换停止与执行未完成是两种边界)。

| 理论看什么 | 实现查什么 |
| --- | --- |
| 来源、相等身份与单值数值约束是不同知识；局部精化少做几轮与程序后继尚未分析也是不同边界。 | 在 [`AbstractValue`](../../crates/evm-abstract/src/domain/value.rs) 分开看 identity、provenance 和 numeric，在 [`ReductionStatus`](../../crates/evm-abstract/src/domain/reduce.rs) 区分局部交换结果。 |

**完成产物：**解释为什么两种数值 profile 都可利用 DUP 或同一固定输入身份，而同为 Calldata 来源的独立读取仍可能不同。给“集合容量不足”“事实交换轮数不足”“共享工作预算耗尽”分别写出保留的信息与完成状态影响；参数位置可查[策略与 JSON](../12-product-domains-facts.md#7-选择策略并读懂-json)。

## 学到哪里可以停

完成前两单元，就能读有限集合与 Top；完成全部四单元，就能区分组合数值精度、有限身份保证和资源边界。当前工作表 join 不运行有界 facts 交换，也不保证得到理论上最精确的组合闭包。

下一步按问题选模块：循环中的摘要为何扩大，读[控制流](control-flow.md)；多个值怎样保留路径关系，读[符号关系](relations.md)；字节或 slot 为何丢失配对，读[内存与 storage](memory-storage.md)。这些进阶问题不会仅因增加常量容量而自动解决。
