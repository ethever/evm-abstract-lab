# 01：从字节找到指令和基本块

阅读路线：[理论：字节码与具体语义](routes/theory.md#execution) · [实现：解码与基本块](routes/implementation.md#execution) · [选择路线](learning-routes.md)。

这一课解决两个问题：哪些字节是指令，哪些指令应该放在同一个基本块。做完后，你应能解释“字节码里看见 `5b`”为什么还不足以认定它是合法跳转目标。

先完成[快速开始](00-start.md)。本课所有命令都在仓库根目录运行；`0x` 表示十六进制，栈列表按**栈底 → 栈顶**排列。

`disasm` 只解码字节，不执行抽象分析，因此没有 `--evm.*` 输入。`cfg`、`ssa`、`explain` 才读取这些环境参数；省略时使用[符号默认值](13-evm-environment.md)。

本课输入是**普通 EVM 字节码**：操作码及其立即数组成的指令流，解码器从字节偏移 `pc=0` 开始逐条读取。这个名称描述代码格式；指令是否有效仍由所选硬分叉版本决定。EOF 是另一种带格式头和分区的容器格式，本项目不支持。

## 1. 先手算三字节程序

输入是 `60 5b 00`。每两位十六进制数字表示一个字节，但一个字节不总是一条指令：

| 字节偏移 pc | 原始字节 | 角色 |
| --- | --- | --- |
| `0x00` | `60` | `PUSH1` 的操作码：把后面 1 字节作为数值压栈 |
| `0x01` | `5b` | 上一条 `PUSH1` 的立即数，即 `0x5b` |
| `0x02` | `00` | `STOP` 的操作码：结束执行 |

**pc 是字节偏移，不是指令序号。** `PUSH1` 占 2 字节，所以它后面的指令在 pc=`0x02`。本例只有两条指令：

```text
pc=0x00: PUSH1 0x5b   [] → [0x5b]
pc=0x02: STOP         [0x5b] → 执行结束
```

运行反汇编来核对：

```bash
nix run . -- disasm --hex 605b00
```

关键输出是：

```text
B0 @ 0x0000:
       0000: PUSH1          0x5b
       0002: STOP
```

指令行的 pc 省略 `0x`，仍是十六进制；数字列随块编号宽度与标题中 `0x` 后的数字对齐。SSA 也使用这套 B 标题和 pc 列，状态元数据与入口 φ 在标题前另行显示，见[第 04 课](04-ssa.md)。

没有 `0001: JUMPDEST`。虽然 `5b` 单独作为操作码时表示 `JUMPDEST`，这里它属于 PUSH 数据，不能被执行，也不能作为合法跳转目的地。

解码时的步骤因此是：读一个操作码 → 判断它是否是 `PUSHn` → 读取 n 个立即数字节 → pc 前进 `1+n` → 解码下一条。`PUSH0` 直接压入零，没有立即数字节。

## 2. PUSH 的立即数缺一半时怎么办

`61 ab` 中，`61` 是 `PUSH2`，声明要读取两个字节，却只剩一个。EVM 把越过代码尾部的字节当作零，因此读取的是：

```text
声明的 2 字节立即数：ab [缺失]
补齐后：            ab 00
压入的数值：        0xab00
```

按大端序解释时，左边的字节更高位。因此结果是 `0xab00`，不是 `0x00ab`。pc 仍前进完整的 3 字节；后面没有代码，执行隐式停止。

```bash
nix run . -- explain --hex 61ab
```

看反汇编中的 `PUSH2 0xab00`，再看 CFG 部分的 `stack out [{0xab00}]`。花括号表示一个槽位可能取到的值；此处只有一种可能，[抽象域一课](02-domain.md)解释集合。

## 3. 把指令切成基本块

**基本块**是一段正常执行时顺序经过的指令。控制流要跳入、跳出或暂停时，需要一个块边界。块间哪些边可能存在，还要结合[数值摘要](02-domain.md)与[控制流传播](03-cfg.md)来计算。

本实现按以下规则切块：

| 位置或指令 | 块边界的原因 |
| --- | --- |
| 第一条指令 | 程序入口，是第一个块的起点 |
| 真正的 `JUMPDEST` | 是潜在跳转入口，开启一个块 |
| `JUMP`、`JUMPI` | 下一步由跳转目标或条件决定，结束当前块 |
| `STOP`、`RETURN`、`REVERT`、`SELFDESTRUCT`、无效操作码 | 当前路径在这里结束，后面的指令另成块 |
| `CALL`、`CALLCODE`、`DELEGATECALL`、`STATICCALL`、`CREATE`、`CREATE2` | 当前帧暂停；后面的指令是恢复执行的起点，另成块 |

最后一类涉及跨合约执行，先记住边界规则即可，[第九课](09-cross-contract.md)再解释调用帧。

块编号 `B0`、`B1` 按代码位置递增，是当前程序的索引。`B1` 不等于 pc=1；它的字节地址另由 `@ 0x...` 显示。

`cfg --format json` 的结果格式版本是 `.schema_version=3`，环境记录是 `.environment`。其状态 JSON 用 `key.basic_block_index` 记录这个索引：要找到实际 pc，先用索引读取 `program.blocks`，再看该块的 `start_pc`。状态编号 `S` 则表示一次分析中的执行位置；一个基本块可能对应多个状态。下面用分支例子核对两套编号：

```bash
nix run . -- cfg --file examples/diamond.hex --context-depth 0 --format json > /tmp/blocks.json
jq '. as $a | [.states[] | {state: .id, block: .key.basic_block_index, pc: $a.program.blocks[.key.basic_block_index].start_pc}]' /tmp/blocks.json
```

你会看到状态 1 位于块 2，而非块 1；分析按执行发现顺序建立状态，解码按代码位置建立块。跨合约时块索引还属于各自的代码，需同时看代码身份。

看一个“代码存在，但入口执行不会到达”的例子：

```bash
nix run . -- explain --hex 005b600100
```

反汇编有两个块：

```text
B0 @ 0x0000:
       0000: STOP
B1 @ 0x0001:
       0001: JUMPDEST
       0002: PUSH1          0x1
       0004: STOP
```

CFG 部分却只有 `S0 | B0`，没有 `B1` 对应的状态。从 pc=0 的空栈入口开始，第一条 STOP 已经结束执行，也没有跳往 B1 的路径。

为什么还要解码停止指令后面的字节？在一般程序中，前面的其他分支可能跳进那里的 JUMPDEST，从而绕过停止指令；是否能到达，要由控制流分析判断。**反汇编回答“有哪些指令”，CFG 分析回答“从给定入口有哪些可能的执行转移”。**

## 4. 检查 JUMP 与 JUMPI 的真实含义

`JUMP` 从栈顶弹出一个数，把它当作目标 pc。合法目标必须同时满足：在代码范围内、位于指令边界、该指令是 `JUMPDEST`。

```bash
nix run . -- explain --hex 600456605b00
```

逐步看：

```text
pc=0x00: PUSH1 0x4   [] → [0x4]
pc=0x02: JUMP        弹出 target=0x4
pc=0x03: PUSH1 0x5b  这条指令的立即数在 pc=0x04
pc=0x05: STOP
```

pc=`0x04` 虽然有字节 `5b`，却是 PUSH 数据。预期输出有 `InvalidJump`，没有通往后一个块的正常边。

`JUMPI` 按**栈顶先弹出**的顺序取两个参数：先 target，再 condition。condition 非零时跳转；为零时继续执行下一条指令。这意味着压栈时应先放 condition，再放 target。

```bash
nix run . -- explain --hex 600060ff5700
```

```text
PUSH1 0x0   [] → [0x0]            先放 condition
PUSH1 0xff  [0x0] → [0x0,0xff]   再放 target，右边是栈顶
JUMPI       弹出 0xff，再弹出 0；条件为零，继续到 STOP
```

看 CFG 中的 `BranchFalse` 边，并确认没有 `InvalidJump`。零条件路径**不检查目标是否合法**；只有实际选择跳转的路径才检查目标。

## 5. 指令是否有效，还取决于 fork

fork 指执行层协议版本。本仓库支持 `cancun`、`prague`、`osaka`，默认 `osaka`。例如 `CLZ` 在 Osaka 下有效，在 Cancun、Prague 下无效；`SLOTNUM`、`DUPN`、`SWAPN`、`EXCHANGE` 在这三个版本下都按无效指令处理。上游库知道一个操作码的名称，并不代表它在你选择的版本里有效。

程序在解码时固定 fork，后续切块、抽象执行、SSA 与输出使用同一版本。EOF 容器会被明确拒绝；[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 委托标记是代码指针，在支持委托的 fork 下单独输入时会报告目标地址，不能当作普通指令流。版本选择、较新指令以及委托解析见[第八课](08-forks.md)。

## 读源码时对应到哪里

先带着上面的例子读 [`Program::decode_with_fork`](../crates/evm-abstract/src/bytecode.rs)：`width` 决定 PUSH 的声明宽度，`available` 决定实际能读多少字节，`pc += width + 1` 决定下一条指令的位置。立即数放在 32 字节数组的最右侧 n 字节区域；实际数据复制到该区域的开头，未读到的部分保留零。随后检查 `Instruction::ends_block` 与 `jumpdest_blocks()` 如何建立块和合法目标索引。指令名称和栈输入/输出数量来自 `revm-bytecode`。

[`pipeline.rs`](../crates/evm-abstract/tests/pipeline.rs) 中的 `push_data_is_not_a_jumpdest`、`truncated_push_pads_on_the_right`、`zero_condition_does_not_validate_invalid_target` 分别核对本课三个容易出错的边界。

相关主题：[用一份状态概括许多执行](02-domain.md)。你将看到同一个栈槽位为什么可以同时保存 `{1,2}`。
