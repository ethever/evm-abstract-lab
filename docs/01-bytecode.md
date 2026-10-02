# 01：从字节找到基本块

本课目标：亲手解码一个例子，理解为什么正确的指令边界是 CFG 的前提。

## 一个字节不总是一条指令

把 `60 5b 00` 当作三个 opcode，就会误以为 pc=1 是 `JUMPDEST`。真实含义是：

```text
pc=0: PUSH1 0x5b   ; opcode=60，立即数=5b
pc=2: STOP
```

`PUSHn` 消耗后面 n 个立即数字节。解码器必须先跳过它们，再看下一条 opcode。本仓库借用 `revm-bytecode` 的名称和栈元数据；解码循环只负责字节边界与立即数。

源码入口：[`Program::decode`](../crates/evm-abstract/src/bytecode.rs)。关注 `width`、`available` 和 pc 的更新，先手算再对照。

## 缺失的 PUSH 字节如何处理

`61 ab` 是不完整的 `PUSH2`。EVM 读取越过代码尾部的位置时取零，因此立即数是 `ab 00`，即 `0xab00`，不是 `0x00ab`。随后 pc 已经越过代码末尾，执行相当于 STOP。

在一个 32 字节数组中，立即数占最右边的 n 个位置；可用字节复制到这段的开头，其余保持零。这同时表达了大端序与右侧补零。

```bash
cargo run --locked -p evm-abstract-cli -- explain --hex 61ab
```

观察出栈里的 `{0xab00}`。[回归测试](../crates/evm-abstract/tests/pipeline.rs) 用这个非对称例子检查补零方向。

## 什么是基本块

基本块是一段正常执行时从头到尾线性经过的指令。它的入口可能是控制流汇合点；出口可能是跳转、分支、停止或者代码末尾。

本实现的块起点是：pc=0、真实 `JUMPDEST`、上一条块终结指令后的指令。块结束在 JUMP/JUMPI、STOP/RETURN/REVERT/SELFDESTRUCT 或无效 opcode；遇到下一个 JUMPDEST 时，也会分开相邻线性区域。

为什么停止指令后还要解码？那里的代码可能被别处的 JUMPDEST 跳入；也可能完全不可达。**解码发现“代码存在”，抽象执行发现“代码是否可达”。** 两者不能混为一谈。

```bash
cargo run --locked -p evm-abstract-cli -- explain --hex 005b600100
```

反汇编有两个块，CFG 只有 pc=0 的状态。第二块没有从空栈入口开始的可达路径。

## EVM 的动态跳转

JUMP 弹出一个数，把它当作新的 pc，但只有真正指令边界上的 JUMPDEST 才合法。目的地藏在 PUSH 数据里、超出代码长度或指向普通指令，都会异常终止。

`600456605b00` 在 pc=2 跳向 pc=4；pc=4 是另一个 PUSH 的数据 `5b`，因此没有合法边。先正确解码，后查询 `jumpdest_blocks()`，才能避免错误边。

JUMPI 弹出 target，再弹出 condition。当 condition=0 时顺序执行，**不检查 target 的合法性**。可以运行 `600060ff5700`：即使 target=255 非法，零条件路径仍会继续到 STOP。

## fork 是语义的一部分

上游 opcode 表包含较新 fork 的定义，这不代表它们已在主网启用。本实现支持 Cancun、Prague 和 Osaka，默认使用最新已激活主网执行层 Osaka。CLZ 只在 Osaka 启用；SLOTNUM/DUPN/SWAPN/EXCHANGE 属于尚未上线的 Amsterdam，仍按无效指令处理。EOF 容器直接返回不支持的格式错误。

选择规则发生在 `Program::from_hex_with_fork` / `decode_with_fork`；每条指令保存相同的不可修改版本。切块、transfer、SSA 与输出共享这一选择，避免“解码按 Cancun、执行按 Osaka”的混用。EIP-7702 委托标记通过 revm 识别并明确报告目标，因为它是代码指针，不是直接可执行的指令流。详见[协议版本一课](08-forks.md)。

相关规则可与 [Ethereum execution-specs 控制流实现](https://github.com/ethereum/execution-specs/blob/master/src/ethereum/forks/cancun/vm/instructions/control_flow.py) 对照。

## 现在你可以检查的三个事实

1. `Program::blocks()` 按地址递增，块的第一条指令地址等于 `start_pc`。
2. `jumpdest_blocks()` 只含真实 JUMPDEST 指令，没有 PUSH 数据。
3. 截断 PUSH 仍按完整宽度推进 pc，缺失立即数右侧补零。

下一课不再用一个具体数字代表栈槽位，而用一组可能值。
