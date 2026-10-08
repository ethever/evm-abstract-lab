# 12：组合域，把几种不完整的认识放在一起

学习入口：[模块：数值摘要与组合域](modules/domains.md) · [理论：组合域与事实交换](routes/theory.md#domains) · [实现：数值层与规约](routes/implementation.md#domains) · [选择路线](learning-routes.md)。

[第 2 课](02-domain.md)用有限常量集合表示一个栈槽的可能值。如果候选太多，集合容量装不下，就只能放弃枚举。组合域继续保存这些候选共有的位、范围和同余性质。例如，不能列完所有偶数，并不妨碍证明最低位一定为零。

本课接着[第 3 课的 CFG](03-cfg.md)做实验：先手算分支，再观察分析器，最后解释组件怎样交换信息。它同样适用于[第 9 课](09-cross-contract.md)的帧、字节和共享状态；先读单合约实验也可以。所有命令在仓库根目录执行。

## 1. 先手算：这个条件可能为零吗

[`known-bits-branch.hex`](../examples/known-bits-branch.hex)读取未知 calldata，保留最低 4 位，再把最低位设为 1，最后把结果作为 JUMPI 的条件：

```text
x = CALLDATALOAD(0)
condition = (x AND 15) OR 1
JUMPI(condition, 0x000c)
```

先不用分析器，分两步想：

| 步骤 | 可能值 | 一定成立的性质 |
| --- | --- | --- |
| `x AND 15` | 0 到 15，共 16 个值 | 高 252 位为零 |
| 再 `OR 1` | 1、3、5、7、9、11、13、15 | 最低位为一，所以非零 |

最终候选只有 8 个，但中间步骤已有 16 个。默认常量容量为 8：如果中途丢掉所有信息，就无法靠下一条 OR 重新知道最低位为一。KnownBits 组件逐位保存必须为零、必须为一或尚未确定的性质，避免这次信息丢失；内部与 JSON 用两个位掩码编码，文本直接显示[十六进制位模式](#怎样读十六进制位模式)。

这次只比较数值组件，先用 `--no-relations` 关闭符号表达式传播和路径约束。`--domain` 选择数值表示，与关系开关是两项独立策略：

```bash
nix run . -- cfg \
  --file examples/known-bits-branch.hex \
  --no-relations --context-depth 0 --format json > /tmp/facts-product.json

nix run . -- cfg \
  --file examples/known-bits-branch.hex \
  --domain constants-only \
  --no-relations --context-depth 0 --format json > /tmp/facts-constants.json

jq '.status, [.edges[] | .kind]' /tmp/facts-product.json
jq '.status, [.edges[] | .kind]' /tmp/facts-constants.json
```

两次分析都可完成。组合域只有 BranchTrue；constants-only 保留 BranchTrue 和 BranchFalse，因为未知 x 的 AND/OR 无法从有限集合获得上述位信息。`--context-depth 0` 让实验集中展示数值域的区别。

这已经说明两件事：`Converged` 可以对应不同精度；提高常量容量也不等于增加未知输入的位语义。要排除假分支，需要某个组件证明条件非零。

## 2. 再手算：两个未知值一定相同吗

[`copy-identity.hex`](../examples/copy-identity.hex)只读取一次未知 word，然后 DUP1：

```text
x = CALLDATALOAD(0)
y = DUP1(x)
condition = x XOR y
```

虽然不知道 x 的数值，但 y 是 x 的副本，`x XOR x = 0`。对照 [`independent-inputs.hex`](../examples/independent-inputs.hex)：分别从 calldata 的偏移 0 和 32 读取 word，再 XOR；这两个数可以不同。

```bash
nix run . -- cfg \
  --file examples/copy-identity.hex --context-depth 0

nix run . -- cfg \
  --file examples/independent-inputs.hex --context-depth 0
```

第一段只有 BranchFalse，第二段保留两种分支。这些命令的 calldata 是 root frame 的未知输入；显式 `--evm.calldata HEX` 可提供具体数据。

**来源相同**与**同一个值**是两项事实。不同偏移的读取都可以带 Calldata 来源，却没有相等保证。root calldata 是不可变输入，已确定偏移的 word 有稳定符号；DUP 保留这个值，SWAP 只改变位置。因此 [`copy-identity.hex`](../examples/copy-identity.hex) 在 product 和 constants-only 下都只有 BranchFalse：这里利用的是同一个输入，不能用它区分两种数值域。

下面分别展示稳定输入身份与临时复制身份：

```bash
# 两次读取同一个 root calldata word，再 XOR；两种 profile 都得到零。
nix run . -- cfg --hex 5f355f351800 --format json > /tmp/input-identity.json
jq '.states[0].exit_stack[0].Constants' /tmp/input-identity.json

# 先做加法，再复制这个运算结果。复制身份属于共用AbstractValue，两profile均生效。
nix run . -- cfg --hex 5f35600101801800 --no-relations --format json > /tmp/copy-product.json
nix run . -- cfg --hex 5f35600101801800 --domain constants-only --no-relations \
  --format json > /tmp/copy-constants.json
jq '.states[0].exit_stack[0]' /tmp/copy-product.json
jq '.states[0].exit_stack[0]' /tmp/copy-constants.json
```

第一条查询输出 `["0x0"]`。关闭关系传播后的最后两条也都显示单点零：加法产生的新值没有继承原 calldata word 的稳定输入身份，但DUP 复制同一个当前定义。临时复制身份属于共用的 AbstractValue，不由数值 profile或关系开关决定。因此复制身份本身不能作为 product 与 constants-only的数值精度对照；[第 1 节](#1-先手算这个条件可能为零吗)使用 KnownBits性质作对照。

临时复制身份在进入下一块、汇合、调用或摘要边界失效。稳定的不可变输入符号则可跨块保留，默认 caller/origin、重复 CALLVALUE、相同偏移的 root calldata word 都属于这一类；摘要还必须匹配完整环境。不同环境里的同名符号不证明相等，子帧 memory 派生的 calldata 也不会冒充 root 输入。两种身份都不意味着“任意两次读取同一 storage slot 都相等”；读取之间的写入会改变状态。SSA 编号也不会反馈成执行器的关系证明。独立符号表达式可以表示派生值并跨块保留；每个机器状态的关系环境另外保存路径约束。

### 默认关系模式怎样补上派生值

在相同 constants-only 数值 profile 下，把关系模式留在默认开启状态：

```bash
nix run . -- cfg --hex 5f356001016008565b5f356001011800 --domain constants-only --format json > /tmp/copy-relational.json
jq '.states[-1].exit_stack[0].Constants, .config.relations' /tmp/copy-relational.json
```

这里再次得到单点零。加法结果不必继承 calldata 的输入身份：两次计算的表达式都是 `CALLDATALOAD(0)+1`，中间的 JUMP 已使临时复制身份失效；持久表达式仍相同，XOR 规范化为零，并把这个常量反馈给数值层。这是符号表达式提供的新证据，与块内复制身份及来源标签各自独立。这个实验没有依赖 DUP，也没有把 profile 当作元数据开关。

关系模式还能保留 JUMPI 的已选分支假设。例如同一个 CALLVALUE 先被约束为 1，之后再测试是否为 2，后一个 true 分支不可行。具体命令和求解边界见[第 15 课](15-symbolic-relations.md)。

## 3. 几个组件约束同一个 word

EVM 的一个 word 是 256 位，取值空间为 `0 .. 2^256-1`。组合域中的数值组件同时描述这一个值，其含义是约束的**交集**：候选必须满足所有组件。

| 组件 | 保存什么 | 可以回答什么 |
| --- | --- | --- |
| FiniteConstantSet | Top 或完整的非空有限候选集合 | x 是否只能是 8 或 16？ |
| KnownBits | 必须为零和必须为一的位 | x 的最低位是否为一？ |
| Interval | 无符号与有符号闭区间 | x 是否落在无符号 1 到 20？ |
| Congruence | 同余，即除以某个正整数后的固定余数 | x 是否为 8 的倍数？ |
| Nonzero 保证 | 已证明不是零 | 条件是否必须非零？ |

这些是 `NumericValue` 的真实字段，数值查询、候选枚举和格式化只读取这一层。机器中的 `AbstractValue` 则组合独立的数据：

| 层次 | 负责什么 | 不应混入什么 |
| --- | --- | --- |
| `numeric`：`NumericValue` | 有限集合、位、范围、同余、非零保证 | 来源、变量身份、表达式和路径约束 |
| `provenance`：`Provenance` | 可能来源与代码地址使用角色 | 输入身份或任意数值相等证明 |
| `identity`：`ValueIdentity` | 同作用域输入与块内临时定义的可信身份 | 根据相同来源或相同数值摘要猜测身份 |
| `expression`：`ExprId` | 不可变输入、运行时新值及其 EVM 纯运算表达式 | 把每次经过同一个 pc 的新值当成同一个变量 |
| 机器状态的 `relations` | 当前路径保留的跨值假设及数值保证 | 全局共享不同路径的假设 |

`AbstractValue` 通过显式字段组合这些信息，没有通过 Deref 隐藏这几层。更改来源标签不改写身份，SMT 证明后的数值收窄也保留原变量与表达式。

来源记录的是当前观察和纯运算输入的类别摘要。MLOAD、SLOAD 等读取会重新标记为 Memory、Storage；纯算术合并输入类别并加入 Arithmetic。它不保存完整读取位置、祖先链或污点历史。

[`FiniteConstantSet`](../crates/evm-abstract/src/domain/finite_constant_set.rs)只管理常量组件：Top 不限制候选，非空集合限制候选必须属于其中，空交返回错误。这个组件单独格式化时，Top 显示为 `⊤`。`AbstractValue::finite_constants()` 查看这个组件；`AbstractValue::contains()` 则检查它与位、区间、同余和非零保证的交集。常量组件为 Top 不等于整个值没有数值限制。

有限集合无法枚举时，其他组件仍可以排除候选。JSON 没有 `Constants` 键，只说明这个组件不能给出完整列表。只有数值约束、来源、使用角色、固定身份和表达式都未知，且没有符号预算缺口时，整个值才序列化为 `"Top"`。数值上为 Top 的输入仍可带 `identity.input.name` 和 `expression`；`provenance` 只记录来源与角色；因此不能仅根据 JSON 是字符串还是对象判断数值精度。反过来，一个候选没有被约束排除，也不代表存在某条执行能取到它。

### 怎样读十六进制位模式

每个十六进制位置对应 4 个二进制位。下面只画**一个位置**，从高位到低位读：

```text
四个位：    0 1 ? ?
文本位置： [ 0 1 * * ]  → [01**]
```

已知的位直接显示 0 或 1，未知位用 `*`。如果同一位置的四位都已知，就合成一个十六进制数字；四位都未知，则合成一个 `*`：

| 一个位置的四位 | 该位置的文本 | 含义 |
| --- | --- | --- |
| `0000` | `0` | 四位都确定为零 |
| `1010` | `a` | 确定为十六进制 a，即十进制 10 |
| `1111` | `f` | 确定为十六进制 f，即十进制 15 |
| `????` | `*` | 四位全部未知 |
| `01??` | `[01**]` | 高两位确定为 0、1，低两位未知 |
| `000?` | `[000*]` | 高三位为零，最低位未知 |

`[01**]` 是一个十六进制位置内部的**四位二进制模式**，不是可选十六进制数字列表；它允许这个位置取 4、5、6、7。单个 `*` 允许一个位置取 0 到 f。完整模式从最高位到最低位排列，`0x` 后始终有 64 个位置：每个数字、每个单独的 `*`、每组方括号各占一个位置。部分已知的方括号会增加字符数，所以“64 个位置”不等于“64 个字符”。模式不省略前导零、任何 `*` 或连续重复位置。

整个值没有数值限制时显示为 `⊤`。KnownBits 组件本身没有固定位约束时，单独格式化该组件仍显示完整模式：

```text
0x****************************************************************
```

仍有数值约束且没有完整常量集合时，文本在无符号区间后显示 `bits=`。例如[第 2 课的 CLZ](02-domain.md#4-transfer在摘要上执行指令)：

```text
u[0x0,0x100] bits=0x0000000000000000000000000000000000000000000000000000000000000[000*]** mod(0x1)=0x0
```

前 61 个十六进制位置都是零，接着 `[000*]` 保留三位为零，再接两个全未知的 `*`：高 247 位确定为零，低 9 位未知。位模式单独允许 0 到 511；`u[0x0,0x100]` 将同一个值进一步限制在 0 到 256。`mod(0x1)=0x0` 对所有整数都成立，没有增加限制。即使 `bits=` 后全是 `*`，区间、同余或非零保证也可能仍然限制候选，因此不能只看位模式判断整个值为 Top。

有限候选集合仍用 `{0x1, 0x2}` 表示，集合中的常量保持普通十六进制写法。JSON 的 `known_bits.zero` 与 `known_bits.one` 仍保存“确定为零的位置”和“确定为一的位置”的两个掩码；这次文本表示不改变数值含义或 JSON 格式。

### 位、范围和同余各自会丢什么

KnownBits 在路径汇合时只保留共有的固定位。连续低 k 位全部固定时，可以得到 `x ≡ r (mod 2^k)`；任意不连续位掩码不能直接变成一个同余。

区间保存最小值与最大值，无法保留内部所有空洞。有符号与无符号区间同时约束同一个 word，但排序不同：全为一的 word 在无符号下是最大值，在有符号下是 -1。当前实现使用两种线性界，不是能精确表达任意环形集合的区间域。

同余保留重复间隔，支持 3、5 等一般正模数。例如 `x ≡ 1 (mod 3)` 表示 1、4、7……在 word 空间内的候选。两个同余求交可以结合成更强性质：`x ≡ 1 (mod 3)` 且 `x ≡ 2 (mod 5)` 等价于 `x ≡ 7 (mod 15)`。

以上规则必须服从 EVM 运算：ADD、SUB、MUL 按模 `2^256` 回绕。例如 `(2^256-1)+1=0`；数学整数上的递增区间或奇数模数同余不能直接照搬。无法证明不回绕时，分析器保留覆盖结果的较粗约束。DIV/MOD 的零除数、过大移位和有符号边界也使用 EVM 规则。

## 4. 信息交换：从范围和整除性找回候选

设同一个 x 有两条已知事实：

```text
范围：1 <= x <= 20
同余：x ≡ 0 (mod 8)
```

单独看范围，有 20 个候选；单独看同余，有大量候选。放在一起，只有 8 和 16：

| 交换步骤 | 新得到的性质 |
| --- | --- |
| 范围限定同余的候选 | 最小候选为 8，最大候选为 16 |
| 所有约束共同筛选 | 完整候选为 `{8,16}`，容量足够时恢复有限集合 |
| 有限集合反馈组件 | 最低 3 位为零；同时更新范围和同余 |

这个过程叫**规约（reduction）**：每个组件把自己已经知道的性质交给其他组件，缩小表示中的多余可能。它不会凭空取得缺失的代码、storage 或输入事实。显式 RPC 模式可在完整枚举出 SLOAD 槽键后另外采集初始值；这是快照补充，再从入口重跑，不是数值组件从已有事实推导出 storage 内容。

组件交换的是有明确含义的 **facts（语义事实）**，不要求直接读取彼此的内部表示。当前接口有三类：

| 类型 | 例子 | 需要分清什么 |
| --- | --- | --- |
| 一元事实 | x 在某范围、某位为零、x 是 8 的倍数 | `MemberOf({8,16})` 是“8 或 16”，不是同时等于两数 |
| 二元事实 | x=y、x≠y、x<y | 来源相同不能建立 x=y |
| 运算关系 | result 是两个操作数 XOR 的结果 | 必须遵守 EVM word 语义和弹栈顺序 |

运算关系让[第 2 节](#2-再手算两个未知值一定相同吗)的复制身份证明派上用场：可信 x=y 加上 XOR 关系，才得到 result=0。局部事实中的值编号只标记本次交换的值，不是 SSA 编号或永久变量。它与跨基本块保留的 `ExprId` / `RelationState` 是两组接口：前者规约一次数值操作，后者记录状态路径中的表达式、等式和取值假设，并按需要查询进程内位向量求解器。

来源、地址范围和代码用途也必须区分。`IsAddress` 保证高 96 位为零；`IsCodeAddress` 只记录作为代码地址使用的角色。它们都不能证明该账户存在，更不能提供未知账户的代码。

若想继续看库接口，可从 [`Domain::from_facts`](../crates/evm-abstract/src/domain.rs) 和 [`facts.rs`](../crates/evm-abstract/src/domain/facts.rs)进入。上述范围加 8 的倍数就是该接口可以建立的初始前提。`MemberOf` 的 `FiniteSet` 与常量组件共用非空集合表示，但声明成员关系必须给出具体集合，不能使用 Top。事实表按候选元素计入事实容量；导入组合值后，常量容量决定能否继续保留这份完整列表。这是两项独立的限制。非法范围、空候选或零模数会被拒绝；矛盾不能改写成成功的 Top。

## 5. 同一值的约束取交，不同路径的可能取并

[前一节](#4-信息交换从范围和整除性找回候选)的范围和同余都同时成立，所以取交。如果两条路径分别给出 x=8 与 x=16，汇合后则必须保留两者，得到 `{8,16}`。

工作表逐组件做 join：常量取并集，位只留共有保证，区间取包络，同余保留共有性质，可能来源取并集。路径特有的保证会丢失。暂时无法枚举所有候选时，也不能截取前几个数来冒充完整集合。

当前实现把**工作表 join**与**临时 facts 规约**分开。join 不额外运行有界交换；后续运算可以规约已有约束。原因是几轮传播未必得到同一个闭包，把中途结果直接作为存储 join 的定义，会让路径合并次序影响表示。这里不声称实现了理论上最精确的 reduced product。关系环境的 join 只保留两条路径共同具有的保证：`x=1` 与 `x=2` 两条路径不会被错误合成 `x=1 AND x=2`。当前实现会丢失这种析取的部分精度，不是完整的路径分组。

循环还需要控制不断扩大的范围。例如每次回到同一个状态时，无符号上界继续增加，逐个值迭代可能耗费大量工作。引擎在同一状态第二次严格入口更新起应用 **widening（扩大）**：继续上升的上界扩大到最大值，继续下降的下界扩大到最小值。判断依据是实际工作表更新，也覆盖执行中才发现的回边。

widening 有意放弃部分范围精度，使循环不再逐步挪动同一端点。不断改变的表达式也可被遗忘，关系环境只保留共同保证，避免循环无限积累新的项或假设。临时交换得到的较窄范围不会取代存储组件作为 widening 的锚点。整个分析仍受状态、transfer 和工作额度约束；`Converged` 表示完成当前模型中的固定点，不表示所有路径关系都精确。

## 6. 交换停止与执行未完成是两种边界

交换不能无限回馈。事实表合并重复的位、范围与同余声明；相同事实重复出现不算新的增强。每次交换还受完整轮数与事实容量限制。

| 局部交换状态 | 含义 |
| --- | --- |
| Stable | 当前支持的交换规则没有进一步变化 |
| RoundLimit | 轮数用完，保留已经证明的安全约束 |
| FactLimit | 事实容量不足，保留此前完整轮的安全结果 |
| Empty | 数值约束已证明矛盾 |
| OriginConflict | 来源声明不相容，数值是否不可达仍未知 |
| SymbolicLimit | 数值结果仍安全，但运算表达式不能在节点/深度预算内保留 |

Stable 只针对当前规则；它不说明已经表达全部 EVM 关系。RoundLimit / FactLimit 会降低精度，不能当成数值空集或 EVM 执行失败。RoundLimit / FactLimit 等局部数值交换边界记录 `FactExchangeLimited` 诊断。符号表达式预算缺口另外通过 value/字节数组传播，指令边界保留 `Relations(ExpressionLimit)` 前沿；不能因内部字节运算没有返回 detailed 对象就静默丢失预算证据。

另一条边界是整次分析的累计工作额度。初始化、运算、交换、字节与状态复制、子调用、摘要认证和导入共享根账本。若账本耗尽，留下工作前沿，整个结果为 `Incomplete`。摘要命中不会重新得到预算。显式 RPC 补查 callee 或 SLOAD 的有限槽后会从入口重跑，各轮仍共用 work、transfer 和状态分配额度；被后续轮次替换的图也已经消耗工作。这个工作量是逻辑分析费用，不是 EVM gas，也不是 CPU 时间。

组合域仍有以下精度边界：

| 已能表达 | 仍不能据此推出 |
| --- | --- |
| 数值偏移的范围、固定位和同余 | 已完整解决 memory/storage 的未知别名 |
| 固定输入、派生表达式和已保留的路径关系 | 无预算上限的所有表达式、完整析取、未知别名或全部路径等价性 |
| 可能来源类别 | 已保留完整污点祖先或两次读取必定相同 |
| 每条抽象摘要出口的字节和 Store | 已保留每条具体路径的全部数值相关性 |
| 固定点完成 | 未知调用、缺失代码或不支持语义已被补齐 |

位、范围和同余描述的是偏移的可能数值；内存模型还要根据这些数值决定哪些字节可能被读写。如何选择强/弱更新、逐字节汇合会丢掉什么关联，见[第 14 课](14-memory-model.md)。

## 7. 选择策略并读懂 JSON

默认数值策略为 product，常量容量 8、交换轮数 4、事实容量 256；符号表达式和持久关系默认开启。以下命令把设置显式写出，便于复现实验：

```bash
nix run . -- analyze \
  --world examples/worlds/returndata-copy.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --domain product --reduction-rounds 4 --max-facts 256 \
  --max-work 20000000 --format json > /tmp/facts-world.json

jq '.schema_version, .domain_spec, .status' /tmp/facts-world.json
```

结果的 `schema_version` 为 3；其中 `domain_spec.schema_version` 为 2，它描述域策略的格式，两者含义不同。`domain_spec` 保存 profile、word 宽度、常量容量、交换上限、widening、费用版本与来源策略；当前 `cost_version` 为 2，`provenance_policy` 是 `scoped-expressions-and-value-identities-v3`。`domain_spec.relations` 冻结关系开关、表达式/约束预算和 SMT 的 `rlimit`，同样参与摘要资格。子调用沿用同一份策略，[第 10 课的调用摘要](10-snapshots-summaries-creation.md#什么条件下允许命中)也要求它相等。

`--max-constants` 接受 `1..=usize::MAX`，上限由运行平台决定，没有额外的 64 上限。集合按实际候选增长，参数不会直接预分配容量。提高容量可能保留更多完整候选，例如 constants-only 的未知输入 CLZ 在容量至少为 257 且执行预算足够时能保存 `0..=256`；默认容量 8 则为 Top。product 可由其他组件保存范围、位或同余约束，不能把常量容量当作全部数值精度。

`cfg` 和 `ssa` 同样接受 `--domain`、`--reduction-rounds` 与 `--max-facts`。把 product 换成 constants-only 只改变数值组件；用 `--no-relations` 可另外关闭表达式传播和路径约束，保留输入身份。隔离有限集合精度时应同时关闭关系模式；两项交换上限必须为正。提高上限可能增加精度与工作量，不能自动消除模型前沿。不同策略的 work 数字应结合费用策略解读。

最后检查 JSON 的一个实际值。JUMPI 会弹出条件，分支入口不再保留它；下面只运行[第 1 节](#1-先手算这个条件可能为零吗)的算术部分，在 STOP 前留下结果：

```bash
nix run . -- cfg \
  --hex 5f35600f1660011700 --context-depth 0 \
  --format json > /tmp/facts-value.json

jq '.states[0].exit_stack[0]' /tmp/facts-value.json
```

有限值保留 `Constants` 键；数值 JSON 仍平铺 `known_bits`、`interval`、`congruence` 与 `nonzero`，不是新增一层 `numeric` 包装。`provenance` 只存来源与角色；固定身份另在 `identity.input.name`，保留的表达式在 `expression`。这里的候选是 1、3……15。文本与 DOT 只展示数值概要；需要确认某项保证时读完整 JSON，避免把没有常量列表的组合值误读成完全未知。

输入 JSON 与 RPC 初始账户的读取发生在执行账本建立之前。RPC 的后续账户与有限槽采集发生在分析轮次之间，账户数和 HTTP 请求数另有累计上限；成功后扩充初始事实，从入口重建分析，沿用根执行账本。JSON 的 `rpc_acquisition.states_created` 记录全部轮次的状态分配数，因此可能大于最终 `states` 长度。`fetched_storage` 与 `failed_storage` 记录动态槽采集的地址和槽键，显式 `--slot` 的初始采集不计入前者。数值域的 `--max-facts` 只限制局部事实交换，不限制这些 RPC 观察；读[第 10 课](10-snapshots-summaries-creation.md#可选实验从固定区块采集)可对照两组额度。

当前没有对 JSON/RPC 全部输入设置统一字节配额，也没有全过程峰值内存配额。更完整的析取/路径分组、别名关系与输入准入仍是[后续设计边界](https://github.com/ethever/evm-abstract-lab/issues/21)。源码入口是 [`domain.rs`](../crates/evm-abstract/src/domain.rs)、[`numeric.rs`](../crates/evm-abstract/src/domain/numeric.rs)、[`value.rs`](../crates/evm-abstract/src/domain/value.rs)、[`identity.rs`](../crates/evm-abstract/src/domain/identity.rs)、[`relational.rs`](../crates/evm-abstract/src/domain/relational.rs)，真实 CFG、字节和状态精度的回归样例在 [`product_domains.rs`](../crates/evm-abstract/tests/product_domains.rs)。

默认输入、跨帧规则和全部环境参数见[第 13 课](13-evm-environment.md)。
