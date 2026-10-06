# 组合域与语义事实交换

[第 2 课](02-domain.md) 用有限常量集合解释一个栈槽可能装哪些数。集合容量默认为 8；候选超过容量时，常量集合组件会变成 Top。组合域继续保存这些候选共有的位、范围和同余性质。例如，不能列完所有偶数，并不妨碍证明最低位一定为零。

本课从两个可运行的分支实验开始，再解释各个域如何交换信息、为什么交换不会无限膨胀，以及怎样区分精度截断与分析未完成。实现入口在 [`domain.rs`](../crates/evm-abstract/src/domain.rs)，设计边界见 [issue #21](https://github.com/ethever/evm-abstract-lab/issues/21)。

## 1. 先观察分支精度

下面的普通 EVM 字节码读取未知 calldata，保留最低 4 位，再将最低位设为 1，然后把结果作为 JUMPI 的条件：

```text
x = CALLDATALOAD(0)
condition = (x AND 15) OR 1
JUMPI(condition, 0x000c)
```

无论 x 是什么，condition 只能是 1、3、5、7、9、11、13、15，因此一定非零。分别运行默认组合域和常量集合对照：

```bash
nix run . -- cfg \
  --hex 5f35600f16600117600c57005b00 \
  --context-depth 0 --format json > /tmp/facts-product.json

nix run . -- cfg \
  --hex 5f35600f16600117600c57005b00 \
  --domain constants-only \
  --context-depth 0 --format json > /tmp/facts-constants.json

jq '.status, [.edges[] | .kind]' /tmp/facts-product.json
jq '.status, [.edges[] | .kind]' /tmp/facts-constants.json
```

组合域可排除 BranchFalse；constants-only 在未知 x 上失去 AND/OR 的位信息，需要保留两种分支。`--context-depth 0` 让这个实验集中展示数值域的区别。

接着观察复制关系：

```bash
# 读一次未知 word，DUP1 复制，然后 XOR 两个副本。
nix run . -- cfg --hex 5f358018600857005b00 --context-depth 0

# 分别读取 calldata 的第 0 和第 32 字节开始的两个 word，然后 XOR。
nix run . -- cfg --hex 5f3560203518600a57005b00 --context-depth 0
```

第一段的两个操作数来自同一次定义，`x XOR x = 0`，所以只有不跳转的分支。第二段的操作数都带 Calldata 来源标签，但它们可以不同，分析必须保留两种分支。来源相同不能证明数值相等。

这些命令的 calldata 是单合约 CFG 入口的未知输入；`analyze --calldata ...` 可以另外提供具体调用数据。

## 2. 每个域分别保存什么

数值域都描述同一个有限 word 空间：`W = {0, ..., 2^256 - 1}`。一个组合值的数值含义是各组件约束的交集。

| 组件 | 保存的性质 | 例子 |
| --- | --- | --- |
| 常量集合 | 完整、非空的有限候选集合 | `{8,16}` |
| KnownBits | 一定为零与一定为一的位掩码 | `x AND 255` 的高 248 位一定为零 |
| Interval | 无符号闭区间与有符号排序坐标上的闭区间 | 无符号 `1 <= x <= 20` |
| Congruence | 一般整数同余在 W 中的解 | `x ≡ 1 (mod 3)` |
| Provenance | 可能来源、代码地址使用角色及临时复制身份 | Calldata 来源；DUP 保留同一次定义的身份 |

`Value::constants() == None` 只表示常量集合组件不能完整枚举，不表示整个组合值是 Top。`contains`、单点和零值查询会检查全部数值组件。来源与使用角色不改变数值成员关系。

### KnownBits

两个掩码分别记录必须为零和必须为一的位，且不能重叠。AND、OR、XOR、NOT、移位等指令直接传播这些性质。路径汇合保留两个输入共有的固定位；同一值的两个约束取交时，则合并固定位，发现零/一冲突后报告数值矛盾。

连续的低 k 位若都已固定，就能导出 `x ≡ r (mod 2^k)`。不连续的任意位掩码通常不能直接换成一个同余。

### Interval

无符号区间按 word 的普通大小排序。有符号区间使用 `biased(x) = x XOR 2^255`：有符号最小值映射到 0，-1 映射到 `2^255 - 1`，0 映射到 `2^255`，有符号最大值映射到 `2^256 - 1`。这样两个区间组件都可以使用线性闭区间操作。

两个组件同时约束同一个 word。它们的交集可能分成两个无符号片段；这不等同于一个任意圆形区间域。路径汇合分别取线性包络，可能丢失交集内部的空洞。

EVM ADD、SUB、MUL 按模 `2^256` 回绕。区间运算必须检查回绕；无法用当前线性界准确覆盖时，扩大到安全的上界，不能把数学整数上的结果直接当作 EVM word 结果。DIV/MOD 的零除数、移位超过 255 和有符号最小值等边界使用 EVM 的规则。

### Congruence

这里支持一般正模数，包含 3、5 等奇数模数。`m=1` 表示 Top；精确单点单独表示。余数规范到 `0 <= r < m`，并与有限 W 取交。

路径汇合通过 gcd 保留共有同余性质。同一个值的两个同余约束，通过一般中国剩余定理求交；例如 `x ≡ 1 (mod 3)` 与 `x ≡ 2 (mod 5)` 合成 `x ≡ 7 (mod 15)`。中间乘积可能超过 256 位，实现使用宽整数计算，避免把溢出当成矛盾。

同余的普通整数规则也必须处理 EVM 回绕。奇数模数不能原样穿过任意模 `2^256` 的加法或乘法；不能证明不回绕时，转为能覆盖回绕结果的较弱同余。

### Provenance

来源描述当前分析中最近的可观察读取位置以及纯运算输入的来源摘要，例如 Constant、Calldata、Memory、Storage、TransientStorage、Environment、Address、Arithmetic。读取环境、内存或存储时，建立该观察位置的标签；纯数值运算合并操作数标签并加入 Arithmetic。它不声称保留完整祖先或污点历史。来源集合在路径汇合时取并集；容量固定且有界，超出时升到未知来源。

复制身份由执行器在每次进入基本块时分配。新定义得到新身份，DUP 复制身份，SWAP 只改变位置。再次进入基本块时刷新身份；数值 join 丢弃身份。身份不参与抽象状态相等性，也不序列化进 JSON，因此不会让循环因为编号不断变化而产生新状态。

这是基本块内的复制关系精度。两个独立未知读取即使 PC、来源标签或数值摘要相同，也不会获得同一个身份；跨基本块的完整关系环境、带版本的内存/存储符号尚未实现。块转换结束时删除所有复制身份，状态表、回滚数据及摘要不保存可复用的身份证明。运行时身份与分析完成后建立的 SSA 编号属于不同阶段。

代码地址角色是另一项元数据：它说明这个值被作为代码地址使用，不能证明该账户存在或有代码。数值相等的两个 word 仍可能具有不同来源和使用角色；FactLattice 因 Eq 共享数值约束时，分别保留这两类元数据。

## 3. facts 是语义接口

一个域导出已经成立的性质，另一个域导入它能理解的性质。交换格式不是 KnownBits 的内部结构，也不是 Interval 的内部状态，而是一组有明确含义的声明：

```rust
enum Fact {
    Unary(UnaryFact),
    Binary(BinaryFact),
    Relation(RelationalFact),
}
```

| 类型 | 当前语义 |
| --- | --- |
| Unary | Exact、MemberOf、批量 KnownBits、BitSet/BitClear、无符号/有符号界、Congruent、IsZero/NonZero、IsAddress、IsCodeAddress、PossibleOrigins |
| Binary | Eq、Ne、Ult/Ule、Slt/Sle；常量参与的比较可规约成标量界限 |
| Relation | `result = opcode(operands)`，操作数按 EVM 弹栈顺序排列 |

Symbol 是本次交换中的局部值编号，`Symbol::THIS` 表示正在规约的那个标量。它不会从 SSA 编号推导运行时身份。操作关系构造时验证纯 word 操作码和参数个数：ISZERO/NOT/CLZ 一个参数、ADDMOD/MULMOD 三个，其他支持的纯数值操作两个。

`MemberOf({3,7})` 的意思是 `x=3 OR x=7`，并不是 `x=3 AND x=7`。BitIndex 拒绝 256 及以上编号；位掩码构造拒绝重叠；范围构造拒绝下界超过上界；同余构造拒绝零模数；有限集合构造拒绝空集合。

地址的两个事实也必须分清：

| 事实 | 可使用的保证 |
| --- | --- |
| IsAddress | 高 96 位为零，即 word 落在 160 位地址范围内 |
| IsCodeAddress | 代码地址使用角色；没有账户存在或代码存在的证明 |

标量规约的一个实际交换链是：KnownBits 导出范围；范围与同余共同缩小可能的端点；范围再导出固定前缀位；完整候选足够少时，恢复有限常量集合。引擎的纯数值 transfer 还导出 Operation 关系，只有已经证实的复制身份才能为未知的两个操作数加入可信 Eq，并应用 `x XOR x = 0` 等规则。

可以通过受控事实 API 建立初始值：

```rust
use alloy_primitives::U256;
use evm_abstract::domain::{Domain, facts::{UnaryPredicate, WordBounds}};

let domain = Domain::default();
let value = domain.from_facts(&[
    UnaryPredicate::UnsignedBounds(WordBounds::new(U256::from(1), U256::from(20))?),
    UnaryPredicate::multiple_of(U256::from(8))?,
])?;
assert!(value.contains(U256::from(8)));
assert!(value.contains(U256::from(16)));
assert!(!value.contains(U256::from(7)));
# Ok::<(), evm_abstract::domain::facts::FactError>(())
```

这里的候选是 8 和 16。导入的事实是初始语义前提；API 对矛盾返回错误，不能把矛盾输入改成一个成功的 Top 或空集合。

## 4. 防止回馈膨胀

FactLattice 保存当前最强的规范约束，重复插入相同事实返回 Unchanged，真正增强约束才返回 Strengthened。位事实合并到一对掩码，而不是每轮新增 256 条消息；范围只保留合取后的界限；同余合并到一个规范槽位；有限候选按集合基数计算容量。Eq 的分量使用确定的代表和规范等价边，查询无需无限遍历回馈消息。

插入是事务式的：容量不足、数值矛盾或来源声明冲突都会完整保留旧表。容量不足是资源或精度边界，绝不等于已经证明不可能执行。数值矛盾与来源声明冲突也有独立错误：后者不能证明数值不可达。

交换还有完整轮数上限。每一轮将已有组合约束导出到表，再导回组件，保留已经证明的安全结果。

| ReductionStatus | 怎样解释 |
| --- | --- |
| Stable | 当前支持的交换规则没有进一步变化 |
| RoundLimit | 达到完整轮数上限，尚未声称完整规约闭包 |
| FactLimit | 事实容量不足，保留已完成轮的安全结果 |
| Empty | 数值约束已经证明矛盾 |
| OriginConflict | 来源声明不相容，数值不可达性仍未知 |

Stable 不表示已经表达了全部 EVM 关系，也不是任意事实理论的完备求解。纯栈指令达到 RoundLimit/FactLimit 时记录 FactExchangeLimited 诊断；两者保留覆盖所有具体结果的较粗摘要。嵌套字节运算的完整局部报告由库 detailed API 查询，不汇总为逐条指令诊断；共享根工作预算耗尽则留下前沿，整个分析为 Incomplete。精度上限、数值空集、EVM 执行失败和工作预算耗尽属于不同情况。

## 5. 路径汇合、循环与摘要

工作表保存原始笛卡尔组件，并逐组件执行真正的 join：常量并集、共有固定位、线性区间包络、共有同余、可能来源并集及共同成立的角色。join 不运行有界 facts 交换。

原因是：有界传播可能停在不同的中间状态。如果将这种部分规约直接混进存储 join，路径的合并次序就可能影响表示，甚至破坏结合律。当前实现把路径上界与临时精化分开，不声称“运行几轮就构造了理论上最精确的 reduced product”。

区间虽处在有限 word 空间中，逐步扩大的循环仍可能需要极多次迭代。引擎从同一已知状态的第二次入口更新起应用区间 widening：若下界继续下降，扩大到最小值；若上界继续上升，扩大到最大值。触发点依据运行时状态的更新次数，覆盖动态发现的回边；只检查字节码静态图中的环不够。工作表保存的原始区间组件作为后续汇合和 widening 的锚点；临时规约得到的较窄区间不能反过来制造无休止的窄化/扩大。

常量容量、位信息和规范同余也各有有限上升性质；整个执行仍受状态、transfer 和工作预算约束。Converged 说明当前抽象分析完成固定点，不等于单次具体执行轨迹或完全精确的关系证明。

DomainSpec 在根分析入口冻结，包含 profile、word 宽度、常量容量、交换轮数、事实容量、费用版本与 provenance 策略。子调用使用同一份策略。调用摘要的输入 guard 比较完整 DomainSpec，不能仅因为代码地址或常量容量相同就跨策略复用。

初始化、运算、交换、字节与状态复制、摘要认证和导入共同消耗根工作账本。嵌套调用和摘要命中不会重新获得预算。这里计的是分析工作量；EVM gas 的保守成功/失败分支仍由执行模型处理。

## 6. 选择运行策略

默认使用组合域。以下参数可复现实验设置：

```bash
nix run . -- analyze \
  --world examples/worlds/returndata-copy.json \
  --entry 0x0000000000000000000000000000000000000101 \
  --domain product --reduction-rounds 4 --max-facts 256 \
  --max-work 20000000
```

将 `--domain product` 换成 `--domain constants-only` 可对照原来的有限集合精度。`cfg` 和 `ssa` 同样支持 domain、reduction-rounds 和 max-facts；两项交换上限必须为正数，且不会按配置上限预分配事实表。提高上限可能增加精度与工作量，不能自动消除未知调用、未知跳转或不支持语义等前沿。

当前实现仍保守处理无法证明的别名、跨基本块关系和完整路径相关性。未来可以增加新的 Unary/Binary/Relation 消费者，但每个规则都必须先说明其 EVM word 语义、路径汇合行为、完整候选覆盖性和工作费用。

有关组合抽象与迭代规约的理论，参见 Cousot、Cousot 和 Mauborgne 的 [The Reduced Product of Abstract Domains and the Combination of Decision Procedures](https://cs.nyu.edu/~pmc309/publications.www/CousotCousotMauborgne-FoSSaCS11-LNCS6604-proofs.pdf)。论文讨论通过信息交换组合抽象；本仓库实现的是有界局部交换与原始组件 join，具体边界以上面的接口和测试为准。


## 8. 输出和资源对照

结果 JSON 的 `schema_version` 为 1，`domain_spec` 记录完整冻结策略。有限值保留原 `Constants` 键，并额外输出位、范围、同余及来源。有限集合组件未知而其他组件有约束时，输出对象；只有数值与来源都未知时才写成 `"Top"`。观察事实 fingerprint 使用 v2 编码，包含原始约束，独立于运行 profile。

文本和 DOT 展示数值概要，完整结构以 JSON 为准。`Domain::new` 保留 constants-only 兼容构造；`Domain::default` 和 CLI 默认 Product。纯库 API 有界运行；需要累计账本的调用方使用 `apply_budgeted`，可与执行器共用 `WorkBudget`。

成本模型版本 1 使用逻辑工作 credit。固定 256 位规则、完整候选、DP、gcd/CRT、数值组合、载荷复制以及摘要检查/导入均计费；它不是 EVM gas 或 CPU 时间测量。精确小集合采用无需再次交换的快路径。组合域默认工作额度为 2000 万；显式小额度仍返回 typed `Incomplete`。不同 profile 的 used work 需要结合成本策略解释，不能只按数字比较耗时。

输入 JSON/RPC 的读取发生在执行账本建立之前；本次实现没有添加独立输入字节配额或全过程峰值内存配额。更完整的关系环境、路径分组和输入准入仍属于后续工作，见 [#21](https://github.com/ethever/evm-abstract-lab/issues/21)。
