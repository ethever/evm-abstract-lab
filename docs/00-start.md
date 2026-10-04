# 00：先看能摸到的东西

本课目标：能够给别人解释世界输入、调用图和状态结果，并在自己的机器上运行它们。先不用记住“格”“固定点”“支配”这些词。

## 第一眼看两个账户之间的执行

主要输入是一个离线 JSON 世界和入口账户。世界记录代码、初始账户状态、协议版本与事实来源；入口参数记录 caller、calldata、value 和 static 模式。

```bash
nix develop
cargo run --locked -p evm-abstract-cli -- analyze --world examples/worlds/call-return-branch.json --entry 0x0000000000000000000000000000000000000101
```

A=`0x...0101` 调用 B=`0x...0200`；B 返回 32 字节的数值 1；A 读取返回区并据此选择分支，具体成功轨迹最终 A 的 slot 0=1。寻找输出中的 `Call` 和 `Return` 边，再看 `outcome` 下的 storage。抽象图还会保留 gas 模型允许的失败分支，所以结果集合可以比这条成功轨迹更大。`depth=2` 表示 caller 仍暂停在调用栈里，当前执行的是 B。

再运行 `proxy-storage.json`，观察相同的 `code=0x...0300` 出现在不同的 `address` 下。前者告诉你字节码从哪儿读，后者告诉你 storage 写到哪个账户。两者的差别是跨合约分析的核心。[跨合约一课](09-cross-contract.md) 逐项解释调用、回滚和重入。

以下单段字节码实验把注意力缩小到一个账户，帮助理解图中的局部指令。

## 单账户适配入口：已经部署的合约代码

EVM 是栈机器。每次执行的局部状态包括当前字节位置 pc、栈、内存和许多环境信息。栈最后一个元素是栈顶。`PUSH1 02` 把数字 2 放在顶上；`ADD` 弹出两个值再放回它们的和。

本节单段命令的输入是 legacy **runtime bytecode**。Solidity 的 creation bytecode 通常先构造并返回 runtime code，两者不相同。世界执行可以在 CREATE/CREATE2 新帧中执行 initcode，并在之后调用返回的 runtime，见[第 10 课](10-snapshots-summaries-creation.md)。初学时直接使用 `examples/`；它们不需要 RPC、账户或资金。

`examples/straight-line.hex` 是：

```text
600260030160005260206000f3
```

运行：

```bash
nix develop
cargo run --locked -p evm-abstract-cli -- explain --file examples/straight-line.hex
```

你会看到三个阶段的结果。反汇编让原来的字节可读；CFG 让执行顺序可见；SSA 让每个值的来路可见。

## 第一种输出：反汇编

这个例子的指令依次是：

| pc（十六进制） | 指令 | 栈变化（底 → 顶） |
| --- | --- | --- |
| `00` | `PUSH1 2` | `[] → [2]` |
| `02` | `PUSH1 3` | `[2] → [2,3]` |
| `04` | `ADD` | `[2,3] → [5]` |
| `05` | `PUSH1 0` | `[5] → [5,0]` |
| `07` | `MSTORE` | 弹出 offset=0 与 value=5，写内存 |
| `08` | `PUSH1 32` | `[] → [32]` |
| `0a` | `PUSH1 0` | `[32] → [32,0]` |
| `0c` | `RETURN` | 返回内存中从 0 开始的 32 字节 |

注意 pc 从 `00` 变到 `02`：`PUSH1` 占两个字节。pc 是字节偏移。

## 第二种输出：CFG

没有分支，所以 CFG 只有一个可达状态，没有块间边。`in []`、`out []` 分别表示基本块进入和结束时的栈；中间曾经有值不意味着它们最终仍在栈上。

输出里的 `B` 是按字节位置编号的原始基本块，`S` 是分析过程中发现的抽象状态。一个 B 可以对应多个 S；两个编号也不必相同。例如 `S1 B2` 只是“第一个新发现的状态属于原始 B2”。

现在换成分支例子：

```bash
cargo run --locked -p evm-abstract-cli -- cfg --file examples/diamond.hex
```

寻找 `pc=0x000e` 的状态，它的入栈有 `{0x1, 0x2}`。一次具体执行只有 1 或 2；静态分析不知道 calldata，摘要同时覆盖两种可能。

## 第三种输出：SSA

```text
%0 = PUSH1 0x2
%1 = PUSH1 0x3
%2 = ADD %1 %0
```

`%2` 是某个运算结果的名字，不是第三个栈槽位。`ADD` 的参数按弹栈顺序写，栈顶先弹出。加法恰好交换顺序不影响答案；下一课会强调 SUB、DIV 等操作不能随意交换。

SSA 中 `MSTORE` 和 `RETURN` 仍然保留。这些操作影响抽象内存与返回数据，不能把副作用指令删掉，否则学习到的程序会失真。

## 环境为什么有三份锁

`rust-toolchain.toml` 固定 Rust 官方工具链，`flake.lock` 固定开发环境与 Nix 构建工具，`Cargo.lock` 固定 Rust 依赖。它们锁住的是不同层次。Nix `fromRustupToolchainFile` 直接读取 Rust 文件，避免两处版本悄悄分叉。

`nix develop` 提供 rustc、cargo、clippy、rustfmt、rust-analyzer、Graphviz、cargo-nextest 和 lychee；`nix build` 产出 `result/bin/evm-abstract`；`nix run . -- ...` 直接运行打包结果；`nix flake check` 执行门禁。文档链接的[离线与联网检查](../README.md#验证与源码导航)使用同一份 lychee 配置。

## 本课完成标志

你能定位 `ADD` 的原始 pc，说明 `{1,2}` 代表什么，解释 `%2` 为什么是名字而不是位置。下一课从“哪些字节属于指令”开始。
