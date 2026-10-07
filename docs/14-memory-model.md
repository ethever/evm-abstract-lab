# 14：EVM 内存与抽象字节数组

`MSTORE` 把栈上的数写进 memory，`MLOAD` 再读出来。写入的是确定的数时，这很直观；如果数或地址都有多种可能，分析器应该保存什么？这就是本课要解释的内存模型。

先理解真实 EVM 的字节读写，再看本仓库怎样用一个 Rust 数据结构同时表示许多种 memory。最后亲自观察两种精度损失：不知道写到哪里，以及拆成字节后忘掉字节之间的关联。

读过[第 00 课](00-start.md)的栈表即可开始。所有命令在仓库根目录执行，都是离线例子，不需要 RPC、钱包或资金。`nix run . --` 后面是本项目的参数；首次构建可能需要下载依赖。本课使用默认 Osaka。不了解集合或 `⊤` 时，遇到它们再回看[第 02 课](02-domain.md)。

## 1. 先把字节、word 和偏移分开

一个 **bit** 是一位，只能是 0 或 1。一个**字节**（byte）有 8 位，数值范围为 0～255，写成十六进制就是 `00`～`ff`。两个十六进制数字表示一个字节；`0x` 是进制前缀，不占字节。

EVM 栈上的一个 **word** 有 256 位，也就是 **32 字节**。memory 按字节寻址：偏移 0、1、2 是三个相邻字节的位置，偏移不是 word 的编号。

| 写法 | 含义 |
| --- | --- |
| `MLOAD(1)` | 从字节偏移 1 开始读取 32 字节，即偏移 1～32 |
| `MSTORE(1, x)` | 从字节偏移 1 开始写入 x 的 32 字节编码 |
| `MSTORE8(1, x)` | 只向偏移 1 写入 x 的最低 8 位 |
| `[1,33)` | 包含偏移 1，不包含偏移 33，共 32 字节 |

上面的函数形式是为了讲解指令参数，不是 EVM 源码语法。实际指令从栈取参数。本文的栈按**栈底 → 栈顶**排列，最右边是栈顶；例如执行 MSTORE 前的 `[x, 1]`，先弹出偏移 1，再弹出要写的 x。`MLOAD(1)` 不要求地址按 32 字节对齐。

## 2. 第一个实验：写入 42，再读出来

运行：

```bash
nix run . -- explain --file examples/memory-word.hex
```

它的字节码是 `60 2a 5f 52 5f 51 59 00`，先手算：

| pc（十六进制） | 指令 | 执行后的栈（底 → 顶） | memory 的变化 |
| --- | --- | --- | --- |
| `0x00` | `PUSH1 0x2a` | `[42]` | 无变化；`0x2a` 就是十进制 42 |
| `0x02` | `PUSH0` | `[42,0]` | 准备写入偏移 0 |
| `0x03` | `MSTORE` | `[]` | 写入 `[0,32)`，当前长度变为 32 |
| `0x04` | `PUSH0` | `[0]` | 准备读取偏移 0 |
| `0x05` | `MLOAD` | `[42]` | 读取 `[0,32)`；长度仍为 32 |
| `0x06` | `MSIZE` | `[42,32]` | 把当前内存长度压栈 |
| `0x07` | `STOP` | `[42,32]` | 结束 |

在 CFG 部分找到：

```text
  stack in  []
  stack out [{0x2a}, {0x20}]
```

外层方括号表示栈的两个槽位。`{0x2a}` 是第一个槽唯一可能的值 42；`{0x20}` 是第二个槽唯一可能的值 32。它们分别来自 MLOAD 和 MSIZE，不能把 `0x20` 读成十进制 20。

这里观察的是块入口和出口的摘要。中间每一步发生什么，由上面的指令表解释；`explain` 的短输出没有直接列出整份 memory。

### 42 在 memory 里长什么样

MSTORE 使用**大端**编码：高位字节放在较小的地址，低位字节放在较大的地址。42 的 32 字节编码如下：

| 字节偏移 | `0`～`30`，共 31 字节 | `31` |
| --- | --- | --- |
| 内容 | 每个字节都是 `00` | `2a` |

读这 32 字节时，MLOAD 按同一顺序拼回数值 42。下面的图描述本仓库的实现步骤：

```mermaid
flowchart LR
    W["栈上的 256 位值"] -->|MSTORE| B["拆成 32 个字节"]
    B --> M["memory：字节偏移到 Value"]
    M -->|MLOAD| R["按顺序拼回 256 位值"]
```

## 3. Rust 里怎样保存这份 memory

核心结构是 [`ByteArray`](../crates/evm-abstract/src/world/bytes.rs)：

```rust
pub struct ByteArray {
    length: Value,
    bytes: BTreeMap<usize, Value>,
    default: Value,
    memory: bool,
}
```

先用一句话理解：**保存一份长度摘要、一张显式字节表，以及表中没有列出的位置所使用的默认值。**

| 字段 | 保存什么 | 第一个实验里的例子 |
| --- | --- | --- |
| `length` | 可能的字节长度，也是 memory 的 MSIZE 摘要 | 写入后是 `{0x20}` |
| `bytes` | 某个字节偏移上保存的 `Value` | 偏移 31 的值为 `{0x2a}` |
| `default` | 没有单独记录的位置的字节值 | 新 memory 中为 `{0x0}` |
| `memory` | 是否启用 memory 的 32 字节扩容规则 | memory 为 true；calldata（本次调用的输入字节）、returndata（最近一次子调用的返回字节）为 false |

**稀疏**表示按已记录的偏移保存事实，不需要为所有可能地址分配一整个数组。例如单独记录偏移 100 的值，不要求 `bytes` 中同时存在 0～99。`length` 描述的是 EVM 字节区的长度，不是 `bytes.len()`；字节表中记录了几个位置，与当前 memory 有多大是两件事。

`BTreeMap` 让键按偏移排序。这里的键是具体的 `usize`，也就是 Rust 平台能表示的非负索引；它不是符号表达式。关于未知地址怎样进入这张表，第 6～7 节再解释。

### 没有表项，不等于字节未知

先看 `default`：新 memory 的默认字节为零，所以一个未单独记录的位置仍可确定为 0。其他数组的默认值也可以是有限候选或部分约束；变为 `⊤` 时，默认值本身不能缩小数值候选。读取仍要继续检查长度，确定越界时返回零。

表中也可以显式保存零。`ByteArray::exact` 构造已知数据时会省略零字节；写入操作则可能把零也放进表。因此，不能把这个结构理解成“只存非零字节”。

ByteArray 的读取摘要还要结合 `length`，保留序列长度之外补零的可能；RETURNDATACOPY 另有指令级范围检查，第 10 节再比较。如果长度可能为 0 或 1，那么偏移 0 的内容可能存在，也可能属于越界补零。

长度没有完整有限候选时，`byte_at` 也会把字节值与零合并。例如表中明确保存偏移 7 的 `{0xaa}`、但长度为 `⊤`，读取摘要会包含 `{0x0,0xaa}`；显式表项没有被删除，只是长度与字节分别保存，尚不能排除该位置在某条路径上越界。只有同时查看长度、显式字节和默认字节，才能理解数组。

### 每个字节为什么也用 Value

`Value` 描述**一个位置可能出现的数值**。例如 `{0x1,0x2}` 表示这个字节可能是 1 或 2；不是两个字节，也不是同时保存了两个具体值。

默认 `product` 可以在候选太多时继续保存固定位、区间或同余等约束。例如一个字节只可能是 1～255 中的奇数，完整候选需要 128 个常量；默认常量容量 8 列不完，其他组件仍可记录“值不超过 255”“最低位为 1”。MSTORE 拆字节和 MLOAD 拼 word 都调用同一个数值域进行运算。现在 Value 是 AbstractValue 的兼容名称；数值约束属于 NumericValue，表达式属于独立符号层。MLOAD 用平衡拼接限制表达式深度；若 32 个字节恰好是同一表达式按顺序产生的 BYTE(0..31)，还能精确恢复原表达式，避免重复拆装后膨胀。未知偏移或逐字节合并仍可能失去这种联系，详见[第 15 课](15-symbolic-relations.md)。

这是字节表与[第 12 课的组合域](12-product-domains-facts.md)的连接点。整个数值没有限制时显示 `⊤`；单独的 `bits=` 全星号只表示位组件没有确定的位，不能忽略同一个值的其他约束。

`Value` 是通用的 U256 摘要，并非 Rust 的 `u8` 类型。正常拆字节、MSTORE8 等运算会产生 8 位约束，但未知数组或未知地址写入直接使用 Top 时也可能失去这个约束；真实 EVM 字节的范围仍然是 0～255。

## 4. 初始零、扩容和 MSIZE

每个新帧的 memory 初始状态是：

```text
length  = {0x0}
bytes   = 空表
default = {0x0}
memory  = true
```

访问结束位置是 `offset + size`，memory 长度按 32 字节向上取整，并与已有长度取较大值：

```text
访问后的长度 = max(原长度, ceil((offset + size) / 32) × 32)
```

上式描述确定地址和非零长度的成功访问；大小为零的访问不扩容，也不因为偏移很大而分配空间。

| 原长度 | 访问 | 覆盖的字节 | 新长度 |
| --- | --- | --- | --- |
| 0 | `MLOAD(0)` | 0～31 | 32 |
| 0 | `MSTORE(1, x)` | 1～32 | 64 |
| 0 | `MSTORE8(31, x)` | 31 | 32 |
| 0 | `MSTORE8(32, x)` | 32 | 64 |
| 64 | `MLOAD(0)` | 0～31 | 64 |

**读取也会扩容。** 验证这个容易混淆的地方：

```bash
nix run . -- explain --hex 595f515900
```

指令是 `MSIZE; PUSH0; MLOAD; MSIZE; STOP`。出口应为：

```text
  stack out [{0x0}, {0x0}, {0x20}]
```

三个槽依次是：访问前的长度 0、从新 memory 读出的数值 0、访问后的长度 32。内存的当前长度与读取到的内容分别变化。

源码分成两层：`ByteArray::read_word` 只读取已有摘要，本身不扩容；执行 EVM 的 MLOAD 时，[指令转换](../crates/evm-abstract/src/analysis/transfer.rs)先调用 `touch_memory` / `expand`，再调用 `read_word`。直接阅读这些 Rust 辅助函数时要保留这个顺序。

## 5. MSTORE8：只取最低一个字节

```bash
nix run . -- explain --hex 6112ff5f535f515900
```

它把 `0x12ff` 压栈，向偏移 0 执行 MSTORE8，再从偏移 0 MLOAD，最后读 MSIZE。

MSTORE8 只写入 `ff`，丢弃更高的 `12`。因此第一个 byte 是 `ff`，后面 31 个 byte 仍为零。MLOAD 使用大端拼接，得到完整 word：

```text
0xff00000000000000000000000000000000000000000000000000000000000000
```

MSIZE 是 `0x20`。MSTORE8 写出的一个字节放在 word 的哪个位置，取决于它的地址；在偏移 0 写 `ff` 后从偏移 0 读 word，`ff` 位于最高字节，而不是最低字节。

把命令里的第一个 `5f` 改成 `601f`，就改为在偏移 31 写 `ff`；后面的 `5f51` 仍从偏移 0 读取。此时 MLOAD 的结果应为 `0xff`，长度仍为 32。尝试在运行前手算这个变化。

## 6. 地址确定时覆盖，地址有几个候选时合并

先看唯一地址：`MSTORE8(0, 0xaa)` 确定写偏移 0，所以这个位置原来的字节被替换为 `aa`。分析领域把这种替换叫**强更新**：旧值被新值覆盖。其他位置保留。

再从一份全新的 memory 开始。如果偏移 p 可能为 0 或 1，执行同一条 `MSTORE8(p, 0xaa)` 时，真实执行有两种选择：

| 具体选择 | 偏移 0 | 偏移 1 | 其他字节 |
| --- | --- | --- | --- |
| p=0 | `aa` | `00` | `00` |
| p=1 | `00` | `aa` | `00` |

本仓库先在两份临时副本中分别写入，再按地址合并。结果是：

| 位置 | 合并后的字节 Value | 读法 |
| --- | --- | --- |
| 0 | `{0x0,0xaa}` | 这次可能写它，也可能没有写它 |
| 1 | `{0x0,0xaa}` | 同上 |
| 2 及之后 | `{0x0}` | 两种选择都没有写它 |

旧值与新值都保留下来，叫**弱更新**。这里的“弱”指保存的保证较少，不表示执行失败。地址的多个候选不会被当成一次真实执行里全部都写入。

运行一个从未知 calldata 选择 p=0 或 p=1 的例子：

`--context-depth 0` 关闭按最近跳转历史分组，使两条路径的地址候选在同一个状态汇合。没有这个参数，默认保留的历史可能让两路分别分析，暂时看不到弱更新。

```bash
nix run . -- explain --file examples/memory-write-alias.hex --context-depth 0
```

在 `B3 @ 0x000e` 的汇合状态找到 `stack in [{0xaa}, {0x0, 0x1}]`：左边是待写字节值，右边的地址位于栈顶。MSTORE8 消耗两个槽位，后续 MLOAD(0) 与 MSIZE 留下两个结果。

第一槽会包含四种 word，其中甚至有 0；第二槽为 `{0x20}`。这不是说真实执行可以一次不写任何位置：两个位置的候选已被独立保存，重新组合时允许 `00 00`，也允许 `aa aa`。真正的一次写入只有前一张表中的两行。第 8 节用更短的数值解释同一种关联损失。

源码的有限地址写入在 [`write_values`](../crates/evm-abstract/src/world/bytes.rs) 中。它先验证并计算覆盖所有候选的长度摘要，再分别覆盖候选地址、合并副本。地址选择、长度与各字节之间的关系并未作为完整路径条件一起保存。

## 7. 无法列出地址，与无法列出长度，是两个问题

有限常量集合能枚举时，内存操作遍历它。**没有完整地址候选集合**时，即使偏移 `Value` 还有区间或固定位约束，当前实现也没有一般的区间寻址或符号地址求解。

### 7.1 不知道读哪儿

```bash
nix run . -- explain --hex 5f35515900
```

它执行 `PUSH0; CALLDATALOAD; MLOAD; MSIZE; STOP`。没有指定 `--evm.calldata`，所以第一个 calldata word 未知，并被当作读取地址。

新 memory 的所有字节都为零，分析器仍能确定读出的数值为 0；但访问的结束位置无法确定，长度变为 Top。应看到：

```text
  stack out [{0x0}, ⊤]
```

如果 memory 已保存字节事实，或者默认字节不再为零，无法枚举地址的 `read_word` 通常返回 `⊤`。当前的“纯零可读出零”快速判断要求显式字节表为空且默认字节为零；它不是一般的数组内容证明。

### 7.2 不知道写哪儿

```bash
nix run . -- explain --hex 60aa5f35535f515900
```

先压入 `0xaa`，从未知 calldata 读出 p，执行 `MSTORE8(p, 0xaa)`；然后读取 MLOAD(0) 和 MSIZE。非空写入地址无法枚举时，当前实现：

1. 将长度变为 `⊤`。
2. 清空**全部**显式字节事实。
3. 将默认字节设为 `⊤`，保留 memory 标记。

出口应为：

```text
  stack out [⊤, ⊤]
```

这次分析可以 `Converged`：它已经用更粗的摘要继续计算。清空事实不是把真实内存清零，也不是执行 REVERT；它表示分析器不再承诺旧字节信息仍然成立。当前做法较粗，不能证明某个已知位置与这次写入无关时，那里原有的事实也会丢失。

### 7.3 不知道复制多少字节

下面把未知 calldata word 当作 MCOPY 的长度，源和目标偏移都是 0：

```bash
nix run . -- explain --hex 5f355f5f5e00
memory_status=$?
printf 'exit=%s\n' "$memory_status"
```

应得到 `status=Incomplete`、`Memory` 前沿、`SSA unavailable`，退出码为 2。`Memory` 前沿记录分析在哪条指令遇到了不能展开的字节范围；已有输出是部分结果。

这个例子要复制的数量无法完整枚举。当前字节范围操作要求长度有完整有限候选，并逐个验证能否放入追踪范围；不会创建一个无限长的字节向量。长度只有区间信息也不能自动替代这个要求。

这里要同时读两个信号：`⊤` 说明数值约束丢失，`Incomplete` 说明仍有执行区域没有展开。地址未知导致的粗化和长度未知导致的前沿，不能互相替代。[第 06 课](06-boundaries.md)解释完成状态与精度的区别。

## 8. 为什么刚存进去，再读出来会多出候选

逐字节的数值候选可以保存每个位置的可能内容，却不自动保存这些位置必须**一起选择同一条路径**的关系。共享表达式有时能保留联系；本节先看路径汇合后失去这种联系的例子。

运行：

```bash
nix run . -- explain --file examples/memory-byte-correlation.hex --context-depth 0
```

这个程序的两条分支分别压入 `0x0101`、`0x0202`，汇合后执行 `MSTORE(0, x); MLOAD(0)`。在 `B3 @ 0x000f` 应看到：

```text
  stack in  [{0x101, 0x202}]
  stack out [{0x101, 0x102, 0x201, 0x202}]
```

为什么多出 `0x102` 和 `0x201`？先把前 30 个始终为零的字节放到一边，只看最后两个：

| 原 word | 偏移 30 | 偏移 31 |
| --- | --- | --- |
| `0x0101` | `01` | `01` |
| `0x0202` | `02` | `02` |

拆字节后，偏移 30 和 31 各自保存 `{01,02}`。原来“选了第一行，两字节就都必须为 01”的关系没有保留下来。

| 重新组合偏移 30、31 | 结果 | 是否属于原来的集合 |
| --- | --- | --- |
| `01`、`01` | `0x0101` | 是 |
| `01`、`02` | `0x0102` | 否，分析额外允许 |
| `02`、`01` | `0x0201` | 否，分析额外允许 |
| `02`、`02` | `0x0202` | 是 |

所有真实候选仍被包含，额外候选使结果更保守。这次 `Converged` 与信息丢失可以同时发生。MSTORE 的 [`write_word`](../crates/evm-abstract/src/world/bytes.rs) 对每个位置计算 BYTE（取 word 的指定字节）；MLOAD 的 `read_word` 使用 SHL（左移）与 OR（按位或）拼回。本样例在路径汇合时已丢失不同常量各自的表达式关系；数值候选按字节保存，不能据此恢复完整 word 配对。默认符号层另能识别带表达式的完整 word 往返，条件见下文；它不把任意逐字节 join都变成有关联的数组。

### 有表达式的完整 word 往返可以恢复身份

```bash
nix run . -- cfg --hex 5f35805f525f511800 --format json > /tmp/memory-expression-on.json
nix run . -- cfg --hex 5f35805f525f511800 --no-relations --format json > /tmp/memory-expression-off.json
jq '.states[0].exit_stack[0]' /tmp/memory-expression-on.json /tmp/memory-expression-off.json
```

程序保留原 root calldata word，另外存入 memory，再读出来与原值 XOR。默认表达式模式中，MSTORE 的 32 个 `BYTE(i,sameExpr)` 完整覆盖该 word，MLOAD 可恢复原表达式，XOR 结果为零。关闭关系/表达式传播时，字节的数值摘要仍覆盖原值，但不能证明重新组装的未知 word 就是原变量，结果为数值 Top。

这条规则需要连续 32 字节来自同一个表达式及对应的 BYTE 索引。部分覆盖、未知地址、不同路径的逐字节 join、缺失表达式或表达式预算不足，都不能凭字段名字猜测恢复。它修复完整 word 拆分/重组的特定关系，没有提供一般符号数组、全部内存别名或完整析取。可保留的表达式和节点/深度预算见[第 15 课](15-symbolic-relations.md)。

### 延后合并能帮助这个例子

```bash
nix run . -- explain --file examples/memory-byte-correlation.hex --context-depth 8
```

跳转历史让到达 B3 的两条路径保持为不同状态。一个状态的输入、输出都只有 `{0x202}`，另一个都只有 `{0x101}`。因为存入 memory 时还没有把两条路径合并，字节各自都确定，没有机会产生交叉组合。

这次改善来自[第 05 课的状态划分](05-sensitivity.md)。它没有给 ByteArray 增加一般的字节关联：更复杂的汇合、部分写入或已经丢失的表达式关系仍可能出现类似精度损失；完整 word 表达式恢复和状态划分是另外两种机制。

## 9. 组合域能保留字节性质，但寻址与关联仍是另一层

本节用 `--no-relations` 比较数值组件。下面只保留输入的最低字节，再把最低位设为 1，将结果写入偏移 31，随后从偏移 0 读取整个 word：

```bash
nix run . -- explain --hex 5f3560ff16600117601f535f515900 \
  --max-constants 1 --domain product --no-relations
```

表达式是 `(x AND 255) OR 1`，结果为 1～255 中的奇数。偏移 0～30 仍为零，所以 MLOAD 的结果也是这些奇数。`--max-constants 1` 只能保存一个常量，装不下全部 128 个候选；product 的其他组件应继续保留：

- 无符号范围 `u[0x1,0xff]`。
- 高 248 位为零，最低位为一：`bits=0x` 后是 62 个 `0`、一个 `*`、一个 `[***1]`，共 64 个十六进制位置。
- `mod(0x2)=0x1` 和非零保证 `≠0`。
- MSIZE 为 `{0x20}`。

完整出口模式如下；`*` 和方括号中的每一位都保留，可以逐项对照：

```text
  stack out [u[0x1,0xff] bits=0x00000000000000000000000000000000000000000000000000000000000000*[***1] mod(0x2)=0x1 ≠0, {0x20}]
```

换成只保存有限常量集合的模式比较：

```bash
nix run . -- explain --hex 5f3560ff16600117601f535f515900 \
  --max-constants 1 --domain constants-only --no-relations
```

本例出口为 `[⊤, {0x20}]`。这里损失的是字节内容的数值性质，地址 31 仍确定，所以内存长度仍能保留。

这解释了三个不同问题：NumericValue 决定一个位置上的数值精度；寻址规则决定可能读写哪些位置；字节之间的关联决定哪些组合可同时发生。改善其中一层，不会自动解决另外两层。

## 10. COPY：复制字节与读取 word 有什么区别

| 指令 | 从哪里取字节 | 写到哪里 |
| --- | --- | --- |
| `CALLDATACOPY` | 当前帧 calldata | 当前帧 memory |
| `CODECOPY` | 当前执行的代码字节 | 当前帧 memory |
| `RETURNDATACOPY` | 最近一次子调用的返回数据 | 当前帧 memory |
| `MCOPY` | 当前帧 memory | 同一帧 memory |

copy 的栈在取参数前按 `[size, source_offset, target_offset]` 排列，右边的目标偏移先弹出。它复制任意长度的字节序列，不需要把序列解释成整数。

MLOAD、CALLDATALOAD、CALLDATACOPY、CODECOPY 的读取按相应规则补零；RETURNDATACOPY 另有返回数据范围检查，超出返回数据会失败，不能笼统把所有越界复制都当作补零。即使 size=0，也仍须检查 RETURNDATACOPY 的源偏移规则；零大小不扩张 memory，不代表整条指令总是合法。

### MCOPY 的重叠区域

如果源和目标重叠，必须按复制前的源内容取值。运行：

```bash
nix run . -- explain --file examples/memory-overlap-copy.hex
```

程序先把 `0x01020304` 用 MSTORE 写入偏移 0，所以偏移 28～31 是 `01 02 03 04`。随后执行：

```text
MCOPY(target=29, source=28, size=3)
```

| 位置 | 28 | 29 | 30 | 31 |
| --- | --- | --- | --- | --- |
| 复制前 | `01` | `02` | `03` | `04` |
| 复制后 | `01` | `01` | `02` | `03` |

MLOAD(0) 得到 `0x01010203`，MSIZE 仍为 32，出口显示 `[{0x1010203}, {0x20}]`。如果错误地边写边读已经被覆盖的源字节，就会得到连续多个 `01`；本实现先保存源 memory 的快照，再执行复制。这是 **memmove** 式的重叠复制语义。[指令取快照与复制](../crates/evm-abstract/src/analysis/transfer.rs)和 [`copy_from`](../crates/evm-abstract/src/world/bytes.rs)分别承担这两步。

## 11. 每个帧的 memory 独立，调用通过字节传递数据

**帧**（frame）是一次合约执行所用的栈、memory、calldata 等上下文。A 调用 B 时，A 暂停，B 使用自己的新栈和新 memory；B 返回后，A 继续使用原来的 memory。[FrameState](../crates/evm-abstract/src/analysis/machine/frame.rs) 中的 `memory` 属于该帧。

```mermaid
flowchart LR
    A["A 的 memory"] -->|选定输入范围复制| C["B 的 calldata"]
    C --> B["B 执行；使用 B 的新 memory"]
    B -->|RETURN 或 REVERT 取字节| R["A 的 returndata"]
    R -->|按调用输出范围复制前缀| A
```

DELEGATECALL 也创建独立的 memory。它复用调用方的 storage 状态账户，并不因此共用 memory；重入后的另一份 A 帧也有新 memory。帧间共享的账户状态与帧内临时字节区分别处理，详见[第 09 课](09-cross-contract.md)。

这里有两组长度：B 实际返回的数据长度，以及 A 在 CALL 中请求的输出区长度。返回数据完整保存为 A 的 `returndata`；向 A memory 复制的只是 `min(请求长度, 实际返回长度)` 个字节。输出区按整个请求范围扩张，未被返回字节覆盖的后缀保留原内容。

例如 A 请求复制 4 字节，B 实际只返回 2 字节，那么前两字节被替换，后两字节保留；不能把后两字节自动清零。`copy_return_data` 的[回归测试](../crates/evm-abstract/tests/world.rs)核对了这个行为。返回长度未知时，当前模型逐字节合并“被复制”和“未被复制”两种可能，同样可能损失位置之间的关联。

CALL 的成功位是另一个压栈结果。成功位、完整 returndata、已复制到 memory 的前缀是三件事。子帧 REVERT 会回滚账户状态，但可以带回 revert data；具体哪些字节会复制要按调用结果和输出范围阅读，不能仅凭成功位为零断定所有 returndata 都为空。

现成实验可观察 CALL 没有自动复制输出时，A 如何稍后取回 returndata：

```bash
nix run . -- explain --world examples/worlds/returndata-copy.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x
```

在具体成功路径上，A 的 CALL 请求输出大小为 0，B 返回 32 字节数值 1；A 随后通过 RETURNDATACOPY 复制并返回它们。模型还保留 gas 等失败可能，阅读各个 outcome，不要把成功路径说明当作唯一结果。

## 12. 怎样看完整 memory，而不只看 stack out

`stack out` 只显示栈。当前世界入口的完整文本在 State details 中显示的是状态**入口**的 memory 长度，没有展开执行后的 memory 字节表。要核对写入后的内容，先保存一个单账户世界的 JSON：

```bash
nix run . -- analyze --world examples/memory-word-bound.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.value 0 --evm.calldata 0x --format json > /tmp/evm-memory.json
```

它向偏移 1 写入 `0x1234`，从偏移 1 读取，再压入 MSIZE。这个例子只有一个块、没有子帧，所以直接查看 `.states[0]` 的 root frame。`entry` 表示块执行前，`exit` 表示块执行后；调用嵌套时还须辨认 root 与 children，不能把本例路径机械套到每个状态。

`nix develop -c jq` 使用开发环境提供的 JSON 查询工具，不要求另外安装 jq。先查入口长度：

```bash
nix develop -c jq '.states[0].entry.call_stack.root.state.memory.length.Constants' /tmp/evm-memory.json
```

结果是 `["0x0"]`。再取执行后的 memory，并只显示本例要看的字段：

```bash
nix develop -c jq '
  .states[0].exit.call_stack.root.state.memory |
  {
    length: .length.Constants,
    stored_offsets: (.bytes | length),
    byte31: .bytes["31"].Constants,
    byte32: .bytes["32"].Constants,
    default: .default.Constants,
    memory
  }
' /tmp/evm-memory.json
```

应得到：

```json
{
  "length": ["0x40"],
  "stored_offsets": 32,
  "byte31": ["0x12"],
  "byte32": ["0x34"],
  "default": ["0x0"],
  "memory": true
}
```

长度为 64，但字节表只显式保存了 32 个偏移，即 1～32。JSON 的 map 键 `"31"` 是十进制字节偏移；它对应十六进制的 `0x1f`。偏移 31、32 分别保存 `12`、`34`，其他显式写入的字节为零，未记录的位置也使用默认零。这正好把 `length`、`bytes` 和 `default` 三者区分开。

本例的值都有完整常量候选，所以查询 `.Constants`。其他程序的值可能是缺少 Constants 的约束对象，查询得到 null；也可能是纯 Top 字符串 `"Top"`，直接索引 `.Constants` 会报类型错误。先查看完整 Value 和它的 JSON 类型，再读位、区间、同余等组件；缺字段和 Top 都不能当作数值 0。遇到前沿时，`exit` 也可能没有完成结果；先查报告状态。

### 返回数据的文本字节表

前一节 returndata 示例的完整文本报告可以展示返回数据的长度、默认字节和显式字节。使用：

```bash
nix run . -- analyze --world examples/worlds/returndata-copy.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x
```

成功返回数值 1 的 outcome 中，同一个 ByteArray 结构以普通数据形式显示。这些字段这样读：

| 字段 | 怎样理解 |
| --- | --- |
| `length={0x20} (32 bytes)` | 这份返回数据的长度确定为 32 字节 |
| `kind=data` | 普通字节序列，不按 memory 规则取整 |
| `default={0x0}` | 没有显式表项的位置默认零 |
| `Offset(s)` | 显式字节事实的偏移或连续偏移范围 |
| `Stored value / hex bytes` | 单字节抽象值，或连续且精确字节的 hex |
| `exact hex:` | 长度和内容都足够确定、且满足显示提取上限时提供的完整字节串 |

`stored byte facts: (none)` 只表示没有显式表项；还要检查长度和默认值。没有 `exact hex:` 也不能推出数组为空或没有任何已知字节。完整文本把连续、精确的字节压成 hex 行，保留抽象字节的 Value；它只整理显示，不把未知位置补成具体数据。

这张文本表在这里展示的是 returndata，不是执行后 memory 的完整 dump；二者共享数据结构，但所属字段与读取入口不同。[字节渲染实现](../crates/evm-abstract/src/render/world/text/bytes.rs)可以对应上表逐项阅读。

## 13. 范围上限、错误与分析完成状态

默认 [`ExecutionConfig`](../crates/evm-abstract/src/analysis/machine.rs) 的 `max_memory_bytes` 为 65,536，也就是 **64 KiB**。它限制能具体追踪的内存结束位置和提取的字节范围；32 字节取整后也必须满足上限。世界入口可以用 `--max-memory-bytes` 调整；单段 `cfg`、`ssa`、`explain --hex/--file` 使用内建执行配置，不接受这个参数。

把上一节世界的上限缩到 32：

```bash
nix run . -- analyze --world examples/memory-word-bound.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.value 0 --evm.calldata 0x --max-memory-bytes 32
memory_status=$?
printf 'exit=%s\n' "$memory_status"
```

MSTORE(1, x) 需要长度 64，因此会留下 `Memory` 前沿、`Incomplete`，退出码为 2。这是分析器的追踪上限，不是对真实 EVM out-of-gas 的精确模拟。当前没有精确计算内存扩容 gas；工作预算 `max_work` 也不是 gas。

库层的 [`RangeError`](../crates/evm-abstract/src/world/bytes.rs) 区分未知大小、偏移超出主机索引、加法溢出和范围超过上限。执行层遇到不能处理的字节范围时记录 `Memory` 前沿；耗尽共享逻辑工作预算则记录 `Work` 前沿。已有精度下降与真正未展开的工作分别报告。

两个限制尤其要理解：

- 偏移无法枚举时，长度可以直接粗化为 `⊤`，无需实际分配那些字节；这不证明真实访问一定在 64 KiB 内。
- `max_memory_bytes` 不是整个分析进程的峰值 RAM 配额。多个状态、帧、Value、字节表及临时副本都会占用主机内存。

`write_values` 等库写入先验证候选范围，再提交更新，避免在一个范围错误后留下写了一半的字节。但不能把这扩大成“任何 Incomplete 都完全没有修改状态”：指令转换可能已完成前面的步骤，报告仍保留到前沿为止的部分执行信息。

## 14. 从源码找到每条规则

先读本文例子，再按下表找实现。无需一次读完整分析器。

| 想核对的规则 | 源码入口 |
| --- | --- |
| 四字段表示、初始零、缺失位置与长度补零 | [`world/bytes.rs`](../crates/evm-abstract/src/world/bytes.rs)：`ByteArray`、`memory`、`byte_at` |
| MSTORE/MSTORE8 拆字节、确定与不确定地址写入 | 同文件的 `write_word`、`write_byte`、`write_values` |
| 32 字节取整、范围上限 | 同文件的 `expand`、`bounded_sizes`、`bounded_end` |
| MLOAD 拼 word、路径逐字节合并、重叠复制 | 同文件的 `read_word`、`join`、`copy_from` |
| EVM 指令何时触碰 memory、错误怎样成为前沿 | [`analysis/transfer.rs`](../crates/evm-abstract/src/analysis/transfer.rs)：`touch_memory`、MLOAD、MSTORE、COPY 分支 |
| 帧的 memory 与子调用输入/输出 | [`machine/frame.rs`](../crates/evm-abstract/src/analysis/machine/frame.rs)、[`transfer/calls.rs`](../crates/evm-abstract/src/analysis/transfer/calls.rs) |
| 零填充、有限地址写入、范围、复制的现有回归 | [`tests/world.rs`](../crates/evm-abstract/tests/world.rs) |
| 非有限字节值仍保留范围与奇偶性质 | [`tests/product_domains.rs`](../crates/evm-abstract/tests/product_domains.rs) |

普通单程序 SSA 主要给栈值起名字；跨合约 SSA 还通过效果链记录包括 memory、账户状态和回滚点的整机变化。当前效果没有按具体内存地址分区，结构验证也不会补回已经丢失的字节关联；详见[第 09 课的完整 SSA](09-cross-contract.md)。

## 15. 四个自检问题

先回答，再看提示；这些问题分别检查字节顺序、地址、关联和完成状态。

1. 在新 memory 中执行 MSTORE(1, 0x1234)，MSIZE 是 32 还是 64？`12`、`34` 分别在什么偏移？
2. 对新 memory 执行一次 MSTORE8(p, 0xaa)，其中 p∈{0,1}。合并后两个字节各自有 `{0,0xaa}`，是否证明一次真实执行能同时把两个位置写成 `aa`？
3. `{0x0101,0x0202}` 写入后读出四个候选，分析器是否漏掉了原先允许的值？为什么仍会显示 Converged？
4. 未知地址写入后返回 `⊤`，与未知 MCOPY 大小留下 `Incomplete / Memory`，分别丢失了什么？

<details>
<summary>提示与答案</summary>

1. 访问 `[1,33)`，向上取整到 64；高 30 个字节为零，`12` 在偏移 31，`34` 在偏移 32。地址无需按 word 对齐。
2. 不能。一次真实执行只写一个位置；独立保存的逐字节候选丢失了“两个位置不能同时被写”的关系。
3. 没有漏掉 `0x0101`、`0x0202`，但额外允许交叉组合。传播已经在当前抽象模型中完成，收敛不要求每个值都精确。
4. 前者通过更粗的数值/字节摘要继续传播；后者不能展开所需范围，留下未完成的执行区域和原因。零、Top、空数组、没有表项、没有到达的状态也分别有不同含义。

</details>

接着可以回到[第 09 课](09-cross-contract.md)观察真实调用图中的字节传递，或读[第 12 课](12-product-domains-facts.md)研究单个 Value 怎样交换信息。[例子索引](../examples/README.md)列出本课的可运行输入。
