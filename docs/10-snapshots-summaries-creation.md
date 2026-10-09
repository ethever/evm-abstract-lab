# 10：复用调用结果、部署代码与固定快照

学习入口：[模块：RPC、快照与调用摘要](modules/rpc-summaries.md) · [理论：快照、复用与代码生命周期](routes/theory.md#rpc-summaries) · [实现：RPC、摘要与创建](routes/implementation.md#rpc-summaries) · [选择路线](learning-routes.md)。

读完[第 9 课](09-cross-contract.md)，你已经知道调用会产生返回字节和共享状态变化。本课逐个回答四个问题：相同调用能不能复用分析结果？新合约的代码从哪里来？SELFDESTRUCT 何时删除账户？没有普通字节码的预编译怎样执行？最后把这些实验连接到固定链上快照。

[摘要](#1-调用摘要复用完整结果关系)、[创建](#2-create先执行构造代码再安装运行时代码)、[销毁](#3-selfdestruct转账和删除发生在不同时间)、[预编译](#4-预编译没有普通字节码也有调用帧)四个实验均使用离线合成事实，根帧地址为 A=`0x...0101`，命令明确给出 caller、零 value 和空 calldata，默认预算可完成。省略这些参数会得到符号输入，不等同于这组具体约束。所有命令在仓库根目录执行，需要 `jq`。返回结果仍包含保守 gas 模型允许的失败可能；下文会区分具体成功轨迹与抽象输出。

直接阅读时使用 `explain --world ... --evm.to ...`，默认得到捕获代码的反汇编、简明 CFG 与栈、分开的入口结果和已验证图的 TAC/SSA。需要完整快照、帧上下文、摘要证据和原始效果 SSA 时，追加 `--verbose`；`analyze` 的默认文本与 `analyze --ssa` 仍提供完整报告，以下 `analyze --format json` 命令用于查询当前 schema 4 的字段。显示方式不改变分析语义；这些入口接受相同的调用环境、摘要开关、数值域和执行预算参数。`--verbose` 仅用于世界或 RPC 的 `explain`，单程序 `explain --hex` / `--file` 继续显示反汇编、CFG 与栈 SSA。

所有反汇编与 SSA 基本块指令列表共用顶格的 `B# @ 0xPC:` 标题，下面的 pc 不带 `0x`，数字列随 B 编号宽度与标题对齐。SSA 先显示独立的状态元数据与入口 φ，再显示 B 标题、指令和对齐的 `stack out`。单程序与世界教学 SSA 共用赋值式指令正文；世界视图保留 C、F、state owner、context，以及按 T 标记并带 F/slot 的 φ。完整视图另保留所有帧元数据、原始指令字段和效果链，字节码行与效果行也使用这套布局。编号与证据的读法见[第 04 课](04-ssa.md)和[第 09 课](09-cross-contract.md#默认文本怎样读)。

## 1. 调用摘要：复用完整结果关系

先设想 A 连续两次调用 B，B 的代码和输入没变，B 读到的账户状态也没变。第一次已完成给定抽象输入下 B 的可达执行与可能出口分析。**调用摘要（call summary）**把这份完成的分析保存下来，第二次先核对输入前提，再复用结果，减少重复计算。它保存在分析器中，是执行过程和结果的结构化记录。

[`summary-reuse.json`](../examples/worlds/summary-reuse.json) 把这个问题变成可运行实验：A 两次 CALL B。B 只读自己的 slot 0=7，将它编码为 32 字节返回。两次调用输入相同，但 A 第一次请求复制 0 字节输出，第二次请求复制 32 字节。

```text
第一次：A CALL B → 分析 B 的指令、返回和状态效果 → 保存完整关系
第二次：A CALL B → 核对前提相同 → 将已认证的 B 子图接到新 caller 上
```

一次具体成功执行中，B 返回前 31 字节为零、最后一字节为 `07` 的数据。分析器还保留模型允许的失败可能。因此缓存保存的是“在这组抽象输入下，B 的每种结束方式，分别对应什么返回字节和 Store 效果”，而不只是一个数值 7。若有 REVERT，也要把回滚后的状态与 REVERT 数据一起保存。各输出之间的关系不能拆开重组。

这里的“完整”指子图已经闭合，所有保留的抽象出口都已记录。一条抽象输出内部仍可能含 join 后的值集合，不能从中还原每条具体路径的数值相关性。例如返回值候选与某个 slot 的候选各有两个值，不证明四种配对都能具体执行，当前表示也未必知道它们怎样逐项对应。摘要复用保留已有精度；它不会把抽象结果变成具体执行证明。

摘要还携带对应的执行子图：哪些指令状态和边导出了这些结果。这样第二次命中时，图中仍能看见 B 的调用过程，SSA 也能检查值和状态效果的来源。后文把这份完整子图及输入、输出证据称为**证书**；这里不表示链上状态证明。

### 先分清“保存”和“复用”

`analyze` 文本或 `explain --verbose` 的 `Call summaries` 分区显示摘要统计和每份记录。`published=1` 表示保存了一个可复用的完整结果；`hits=1` 才表示后来的调用复用了一个结果。`source` / JSON 的 `source_state` 指向最初分析的 callee 入口；`reused_at` 指向后来的复用入口。完整文本中各摘要还列出出口种类、返回长度和代码身份；完整的返回字节与 Store 关系在 JSON 的 `summaries[].outputs` 中。

在[第 9 课的 `returndata-copy.json`](09-cross-contract.md#3-一次调用需要保存哪些东西) 中，A 只调用 B 一次。若统计显示 `published=1`、`hits=0`，含义是第一次分析已经完成并保存，但没有第二次相同调用来使用它。这是正常情况，不能据此判断调用失败或摘要没有工作。[下面的两次调用实验](#第一步分别开启和关闭摘要)才用于观察复用。

### 第一步：分别开启和关闭摘要

```bash
nix run . -- analyze \
  --world examples/worlds/summary-reuse.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --format json > /tmp/summary-on.json

nix run . -- analyze \
  --world examples/worlds/summary-reuse.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --format json --no-summaries > /tmp/summary-off.json

jq '.status, .summary_stats,
    [.summaries[] | {source_state, state_count, edge_count, reused_at}]' \
  /tmp/summary-on.json
jq '.status, .summary_stats.hits, .summaries' /tmp/summary-off.json
```

两次都应为 `Converged`。开启摘要时 `hits > 0`、`imported_states > 0`；关闭时 `hits=0`，证书列表为空。

| 字段 | 怎样读 |
| --- | --- |
| `hits` / `misses` | 找到 / 没找到前提完全相等的完整摘要的次数 |
| `published` | 已保存为可复用证书的完整摘要数量 |
| `rejected_incomplete` | 输入变化或子图未闭合而未能保存的候选数量 |
| `imported_states` | 复用时加入新调用位置的子图状态数 |
| `source_state` | 最初认证此摘要的 callee 入口状态 |
| `state_count` / `edge_count` | 证书覆盖的状态数 / 边数 |
| `reused_at` | 后来复用此证书的 callee 入口状态编号 |

证书来自正常工作表已经完成的分析，包含更深调用。命中时仍导入实际 callee 指令和调用/返回边，所以完整图仍可构建并核对 SSA。

### 第二步：确认复用没有改变最终关系

图编号可能不同，比较时只取入口结果的种类、返回字节和 Store：

```bash
jq -S '[.outcomes[] | {kind, data, store}] | unique' \
  /tmp/summary-on.json > /tmp/summary-on-relations.json
jq -S '[.outcomes[] | {kind, data, store}] | unique' \
  /tmp/summary-off.json > /tmp/summary-off-relations.json
cmp /tmp/summary-on-relations.json /tmp/summary-off-relations.json
```

`-S` 排序对象字段，`unique` 去掉重复关系；`cmp` 无输出且退出 0，表示这个实验的最终关系一致。它是针对该样例的核对，不是任意输入的等价性证明。

### 什么条件下允许命中

摘要只在**一次固定 world 分析内**缓存，不跨分析持久保存。复用要求输入事实相等，不能只看合约地址或函数 selector（calldata 起始 4 字节，常用来选择函数）：

例如，B 第一次读取 slot 0=7、第二次读取 slot 0=9，虽然地址、代码和空 calldata 都相同，旧结果也不能直接拿来用。caller、value、静态模式或交易/区块环境改变时也一样；这些事实可能影响执行。当前实现比较完整抽象输入，而不是推测“B 大概只读了 slot 0”后忽略其他状态。

本课的**代码 hash**是代码字节的 Keccak 摘要；**初始事实指纹（fingerprint）**绑定整组初始事实。快照身份绑定其声明的来源，具体格式见[第 5 节](#5-固定快照同一个名字不代表同一组事实)。**ORIGIN（最外层交易发起者）**在省略 `--evm.origin` 时与根帧 caller 共享同一输入，可由该参数单独覆盖；选定后在嵌套调用中保持不变。帧的 CALLER 则按 CALL 系列规则推导，不会因为根 caller 为符号输入就把所有子帧 caller 都设成新的未知值。

| 必须相等的前提 | 防止什么错误 |
| --- | --- |
| fork、快照身份、初始事实指纹、当前代码 hash | 把不同规则、不同区块或改变后的代码混用 |
| 完整不可变 EVM 环境，包括根调用输入、origin、交易/区块标量、gas 上界、hash 表和符号作用域 | 省略会影响执行的输入，或把独立环境的同名符号当成同一个变量 |
| callee 帧的逻辑 ADDRESS、caller/static/value/calldata、保存点与完整 Store | 忽略 storage、transient、余额、日志、nonce、代码或生命周期变化 |
| 入口关系约束及值表达式 | 在较弱或不同的分支假设下复用依赖旧条件的结果 |
| 剩余调用深度和冻结的分析策略 | 复用时得到额外深度，或改变数值域、facts 交换、关系资源、内存、跳转历史及费用策略 |

A 的暂停帧和 A 所拥有的输出复制继续信息不属于 callee 输入，所以本例的输出长度 0/32 不妨碍命中。callee 的其他帧事实和回滚保存点仍需相等。登记候选后，输入若经 join 扩大，就不能按旧前提发表证书；需要以更新后的输入重新登记、完成分析并认证。callee 未完成或预算中断时也不能发表完整证书。[第 12 课](12-product-domains-facts.md)会解释为什么同一字节码用组合域和 constants-only 得到的精度可能不同；摘要输入也绑定这份完整策略，不能仅按常量容量判定兼容。

摘要中的入口帧是相对于子图而言的：B 在 A→B 的全图里是子帧，在单独保存的 B 子图里成为入口。保存时保留 B 的执行数据与回滚点，外层返回契约不属于摘要输入与子图；复用时使用当前 caller 的继续信息，再把 B 接回调用栈。更深的子帧仍需保留各自的继续信息。这样既能复用 B 的行为，又能把这次结果复制到 A 新指定的输出区。状态归一化会清除临时复制身份；不可变环境输入的稳定身份则保留，并随完整环境参与摘要限定。独立环境各有自己的符号作用域；同一环境的克隆与 RPC 重跑保留原作用域。JSON 中同名的 `Caller` 只是报告内的标签，不能跨独立报告据此证明相等。callee 保存成相对入口时，也不会把其 memory 派生的 calldata 当成原根调用的 calldata。关系约束同样保留；重放时，输入绑定的符号继续指向同一输入，摘要内部新产生的 fresh 叶则统一改名，避免不同调用误用同一个未知运行时值。

摘要的查找比较、快照 hashing、认证、复制和图导入都消耗同一份 `--max-work`；导入状态也计入全局状态预算。命中不会重置预算。`SummaryWork` 前沿表示这些操作未完成，状态为 `Incomplete`，完整 SSA 验证器不会接受未闭合图。显式选择部分 SSA 可以查看当前执行证据与未覆盖部分，仍保留该前沿；它不会使未完成 callee 获得完整摘要证书。实现与回归见 [`summary.rs`](../crates/evm-abstract/src/analysis/summary.rs)、[`summaries.rs`](../crates/evm-abstract/tests/summaries.rs)。

### 当前 JSON 怎样记录输入与帧

```bash
jq '{schema_version,
     root: {state_owner: .entry.address, to: .entry.environment.to,
            caller: .entry.environment.caller, origin: .entry.environment.origin},
     root_frame: (.states[0].key.frames[0] | {address, address_value, caller}),
     summary: (.summaries[0].input
               | {environment, relations, frame: .frame.state.key})}' /tmp/summary-on.json
```

分析 `schema_version` 为 4。world/RPC 的根输入在 `.entry.environment`；根 `.entry.address` 和帧 `.address` 是具体状态账户，`.address_value` 是逻辑 ADDRESS，`.caller` 是带类型的地址输入。例如已知地址写成 `{"Concrete":"0x..."}`，默认根 caller 写成 `{"Symbolic":"Caller"}`。这两类输入不能都按裸地址字符串读取。

本实验 `.entry.environment.origin` 为 `null`，表示使用 caller 的默认别名，不表示未知而独立的 origin；显式 `--evm.origin` 会记录地址输入。`.summaries[].input.environment` 仍是全局根调用、交易和区块环境，callee 自己的 caller/value/calldata 在 `.summaries[].input.frame.state` 中，入口关系在 `.summaries[].input.relations`。上面的环境 caller 是外部地址 `0x...1000`，而 B 帧 caller 是 A=`0x...0101`。

单程序 `cfg --format json` 在 `.environment` 记录同一环境模型；`ssa --format json` 则在 `.analysis.environment`。world/RPC 的 `analyze --format json --ssa` 也使用外层 `.analysis` 包装，根环境在 `.analysis.entry.environment`。域策略版本仍在 `domain_spec.schema_version`，当前为 2；它与分析 JSON 的 schema 4 各自描述不同结构。帧在执行状态中的路径是 `.states[].entry.call_stack.root.state` 与 `.states[].entry.call_stack.children[].state`，不是旧的 `.entry.frames`。

环境类型与输入作用域见 [`world/environment.rs`](../crates/evm-abstract/src/world/environment.rs)，JSON 边界见 [`analysis.rs`](../crates/evm-abstract/src/analysis.rs)，帧结构见 [`machine/frame.rs`](../crates/evm-abstract/src/analysis/machine/frame.rs)。

## 2. CREATE：先执行构造代码，再安装运行时代码

部署涉及两段不同的代码：

| 名称 | 何时执行 | 返回值的用途 |
| --- | --- | --- |
| **initcode（构造代码）** | CREATE / CREATE2 创建新账户时 | RETURN 的字节被安装为新合约代码 |
| **runtime（运行时代码）** | 创建成功后的普通 CALL | RETURN 的字节交给 caller |

它们通常不同。成功 CREATE 给 caller 压栈的是**新地址**，不会把已安装的 runtime 当作 caller 的 returndata。

[`create-runtime.json`](../examples/worlds/create-runtime.json) 中，A 初始 nonce=0。**nonce**是账户的序号；CREATE 用创建者地址和创建前的 nonce 推导目标地址。本例目标为 `0xea53a153a9a04fd632b2486d84732feb3b71afb7`。

### 第一步：确认创建需要的事实

打开 fixture，可以看到目标账户明确声明 `existence:"absent"`、代码为空、nonce/余额为零、完整零 storage。这个声明提供“目标可创建”的事实。JSON 没列出目标只表示未知，会留下 `Creation(UnknownCollision)`，不能猜目标不存在。

**碰撞**指目标已有非零 nonce 或非空代码等阻止创建的条件。例子还声明零地址 absent，供模型允许的创建失败路径使用：CREATE 返回零，A 随后可能 CALL 零地址。

### 第二步：运行并区分两种帧

```bash
nix run . -- explain \
  --world examples/worlds/create-runtime.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x

nix run . -- analyze \
  --world examples/worlds/create-runtime.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --format json --ssa > /tmp/create.json

jq '[.analysis.states[] | .key.frames[-1]
     | select(.address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7")
     | {address, address_value, caller, mode, code_hash}] | unique' /tmp/create.json

jq '[.analysis.outcomes[].store.account_observations[]
     | select(.address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7")
     | {existence, nonce, balance, code_size, code_hash}] | unique' \
  /tmp/create.json
```

这里用了 `--ssa`，JSON 根对象是 `{"analysis":...,"ssa":...}`，查询因此从 `.analysis` 开始。不加 `--ssa` 时，`states` / `outcomes` 就在根对象中。

第一个查询应找到同一地址的 `InitCode` 与 `Runtime` 两种 `mode`，并带不同代码 hash。第二个查询看最终账户事实：某些失败可能仍为 absent；成功部署的账户为 present、nonce=`0x1`、code_size=8。

`explain` 的 `Execution code` 也分别列出实际捕获的 InitCode 与后续 Runtime 指令，使用各自的 hash 和模式；它从分析时保存的代码字节反汇编，不会用初始 world 中的空代码替代新安装的 runtime。同一账户地址可以对应多个代码版本，代码地址也可能与使用代码的状态账户不同。两个代码版本都可以有 B0，必须结合 C、mode 和 hash 判断身份。默认 CFG 通过代码目录编号连接到抽象图；`--verbose` 另保留每个捕获帧的入口、出口引用。这些引用都不是具体部署交易的逐指令轨迹。SSA 的返回转移还区分 CREATE 地址结果和普通 CALL 的成功位。

`account_observations` 是方便阅读的账户汇总，不包含 storage slot；slot 仍在 `store.persistent.slots`。nonce 和余额是抽象值，JSON 中确定的数值仍写成含 `Constants` 的对象；已知的 `code_size` 则是整数，本例为 8。代码 hash 为零表示已确认 absent；代码为空但账户存在时，hash 是空字节的 Keccak，两者不同。

### 第三步：跟着成功生命周期读结果

```mermaid
flowchart TD
    A["A 的 nonce：0 → 1"] --> B["新账户帧执行 InitCode；nonce=1"]
    B --> C["initcode RETURN：给出 runtime 字节"]
    C --> D["检查代码限制，在 Store 中安装 runtime"]
    D --> E["CREATE 给 A 返回新地址"]
    E --> F["A CALL 新地址，执行 Runtime"]
    F --> G["runtime 返回 32 字节数值 42"]
```

本例安装的 runtime 是 `602a5f5260205ff3`，共 8 字节，hash 为 `0x30962a84ef989ca0f724a5b2ec94f9cbf6a731752ce2c0be5333bf96e460c9fd`。原始 world 中目标仍是 absent；Store 保存执行时安装的新代码，称为**代码覆盖层（code overlay）**。后续 CALL 从当前 Store 解析代码，而不是回到原始快照。

可进一步看 A 最终返回的字节：

```bash
jq '[.analysis.outcomes[] | select(.kind == "Return")
     | {length: .data.length,
        last_byte: (.data.bytes["31"] // .data.default)}] | unique' \
  /tmp/create.json
```

`data` 是抽象字节数组：`length` 表示长度的抽象值，`bytes` 列出显式保存的位置，其余位置由 `default` 表示。32 字节用 `0x20` 表示；数值 42 的最后一字节是 `0x2a`。这里可能看到 `{0,42}` 的字节集合，包含 runtime 成功返回与 CALL 失败留下零输出的可能，不能读成所有路径都返回 42。

### CREATE2 和失败路径的前提

CREATE2 用创建者地址、**salt（显式给定的 256 位值）**、initcode hash 推导地址；CREATE2 也需要已知且可表示的创建者 nonce 来处理序号变化。**endowment** 是创建时转给新账户的金额。

| 缺少或无法表示的事实 | 对应 `Creation` 前沿 |
| --- | --- |
| 创建者逻辑地址未知（单字节码未提供 to） | `UnknownCreator` |
| 创建者 nonce | `UnknownNonce` / `NonceOverflow` |
| CREATE2 salt | `UnknownSalt` |
| initcode 或其返回的 runtime 字节 | `UnknownInitCode` / `UnknownRuntimeCode` |
| 目标 nonce / 代码不足以判断碰撞 | `UnknownCollision` |
| 金额或非零转账所需余额 | `UnknownEndowment` |

有限数值会枚举候选，initcode/runtime 必须能提取为具体字节。构造帧的初始 storage/transient 属于新账户，代码通过长度、前缀与解码检查后才安装。静态限制、调用深度、[EIP-3860](https://eips.ethereum.org/EIPS/eip-3860) 的 initcode 限制和 runtime 代码限制也影响结果。

执行到增加 nonce、进入 initcode 后，initcode 失败会回滚新账户效果，**保留此次增加的创建者 nonce**；更外层帧 REVERT 时则恢复其更早保存点，连 nonce 一起撤销。因余额不足等在增加 nonce 之前发生的失败不会增加它。创建/调用失败可能与成功路径同时出现在图中。[`creation.rs`](../crates/evm-abstract/tests/creation.rs) 使用独立 revm 轨迹核对这些关系。

## 3. SELFDESTRUCT：转账和删除发生在不同时间

[`created-selfdestruct.json`](../examples/worlds/created-selfdestruct.json) 在同一次执行中：A 用 CREATE2、salt=5、endowment=7 部署新合约 C，再 CALL C。C 写 slot 0 后，将余额给受益人 `0x...0200`，执行 SELFDESTRUCT。

这里的 C 地址是 `0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb`，runtime 为 `60015f55610200ff`，8 字节。成功执行按时间分为：

| 时间点 | C 的代码与状态 | 可观察结果 |
| --- | --- | --- |
| CREATE2 成功 | 代码已安装，余额 7 | `created=true` |
| C 执行 SELFDESTRUCT | 余额转给受益人，标记待删除 | `pending_destruction=true`；代码和 storage 暂时保留 |
| 返回 A | C 仍可被查询和 CALL | EXTCODESIZE 得到 8，EXTCODEHASH 得到 runtime hash |
| 最外层成功结束 | 完成删除，清除临时标志 | C absent，代码/nonce/余额/storage 归零；受益人余额 7 |

### 第一步：查看中途的待删除标记

```bash
nix run . -- analyze \
  --world examples/worlds/created-selfdestruct.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --format json > /tmp/destroy.json

jq '[.states[]
     | select(.entry.store.pending_destruction["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"] == true)
     | .id]' /tmp/destroy.json
```

应得到非空状态编号列表。它证明图保留了“已经 SELFDESTRUCT，但最外层尚未结束”的状态。

### 第二步：查看返回 caller 后的代码观察和最终账户

```bash
jq '[.outcomes[] | select(.kind == "Return")
     | .store.persistent.slots]
    | unique' /tmp/destroy.json

jq '[.outcomes[] | {kind,
      accounts: [.store.account_observations[]
       | select(.address == "0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"
             or .address == "0x0000000000000000000000000000000000000200")
       | {address, existence, balance, nonce, code_size, pending_destruction}]}]
    | unique' /tmp/destroy.json
```

A slot 1 保存读到的代码大小 `0x8`；slot 2 保存 runtime hash `0xacc79ff75f811227da02d9de7061e749e8403447b54eaf6e47fceb2ddfefb04e`。fixture 随后还会再次 CALL C 的代码。第二个查询把同一 outcome 的两个账户放在一起：完整销毁路径中 C absent、受益人得到 `0x7`；未执行成功销毁的路径可能保留 C。不要把中途 `pending_destruction` 或某个最终 outcome 当成全部路径。

本实验室支持的 fork 都采用 [EIP-6780](https://eips.ethereum.org/EIPS/eip-6780)：只有**同交易创建的账户**才在这种情况下删除。预先存在的账户执行 SELFDESTRUCT 时，代码和 storage 保留；向不同受益人转移余额，受益人为自身时保留原账户余额。祖先 REVERT 会恢复转账、slot、代码和待删除状态。把 SELFDESTRUCT 实现成“立即清空代码”会漏掉 A 恢复后仍可执行的代码。

## 4. 预编译：没有普通字节码，也有调用帧

**预编译（precompile）**是在特定地址提供的原生功能，执行由客户端内建实现完成。使用哪些地址与规则由 fork 决定；不需要在 world 中伪造普通合约 runtime。

先看最容易理解的 identity 预编译 `0x4`：它原样返回输入字节。[`identity-precompile.json`](../examples/worlds/identity-precompile.json) 让 A 把数值 42 写在 memory[0..32)，CALL `0x4`，把输出复制到 memory[32..64)，再 RETURN 后一段：

```text
A memory[0..32)：32 字节数值 42
      ↓ CALL 输入
identity 原样返回 32 字节
      ↓ CALL 输出复制
A memory[32..64)：32 字节数值 42 → A RETURN
```

```bash
nix run . -- analyze \
  --world examples/worlds/identity-precompile.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
  --format json --ssa > /tmp/native.json

jq '[.analysis.states[].key.frames[-1].mode] | unique' /tmp/native.json
jq '.analysis.edges,
    ([.analysis.outcomes[] | select(.kind == "Return")
      | {kind, length: .data.length,
         last_byte: (.data.bytes["31"] // .data.default)}] | unique)' \
  /tmp/native.json
```

模式列表含 `{"Precompile":"0x0000000000000000000000000000000000000004"}`，图中有 `Call` / `Return` 边。成功返回包含 `length={0x20}`、最后一字节 `0x2a`；抽象值可能同时含零，原因是 CALL 的保守失败可能没有填充输出区。完整图仍能构建 SSA；没有提供的预编译账户存在性/余额事实仍保持未知。

预编译的 SSA 状态仍有帧、入口 φ、转移和效果证据，但没有普通字节码指令。文本保留原生执行原因，不会借用 caller 的 B 标题或造一个 pc。空代码、无效嵌套委托和代码末尾的合成继续位置同样显示自身原因，只有实际字节码块才使用 B 标题与 pc 列。

密码学执行复用锁定的 [`revm-precompile 43.0.3`](https://docs.rs/revm-precompile/43.0.3/revm_precompile/)。分析器先检查输入资格并预留保守工作量，再进入后端：

| 情况 | 处理方式 |
| --- | --- |
| 输入字节、长度具体且可表示 | 调用原生实现，传播真实返回或执行失败 |
| 输入未知，或 modexp 声明的长度不可表示 | 留下 `PrecompileInput` 前沿 |
| 累计工作不足 | 留下 `Work` 前沿，先停止再避免执行昂贵原生操作 |
| 后端无法完成调用 | 留下 `Precompile` 模型前沿，区别于预编译本身的普通执行失败 |

这支持具体原生返回流，没有补齐精确 gas 或全部执行环境关系。实现见 [`transfer/precompile.rs`](../crates/evm-abstract/src/analysis/transfer/precompile.rs)。

## 5. 固定快照：同一个名字不代表同一组事实

**快照（snapshot）**提供代码、初始 storage、余额、nonce 和存在性事实。固定链上快照中的“固定”首先指 chain ID 与 block hash：分析前与按需补查得到的事实都属于同一个区块。已观察的事实保持不变，尚未观察的账户可以继续加入。每轮分析使用一组固定初始事实；执行指令改变的是 Store。

先查看[第 1 节](#1-调用摘要复用完整结果关系)使用的离线快照：

```bash
jq '.world | {fork, provenance, identity, fingerprint}' /tmp/summary-on.json
```

它的 `identity.kind` 为 `offline`，标签是 `summary-reuse:v1`。fixture 在 world 顶层明确写：

```json
{"identity": {"kind": "offline", "label": "summary-reuse:v1"}}
```

上面是一个字段片段，放在 `fork`、`accounts` 等字段旁边。旧 JSON 只有 `provenance` 时也归为 offline；来源说明会原样保存。

### 身份、代码 hash、指纹各负责什么

| 字段 | 绑定的范围 | 不足以单独证明什么 |
| --- | --- | --- |
| `provenance` | 作者填写的来源描述 | 不能建立链或区块身份 |
| `identity.kind="offline"` + `label` | 一组明确未锚定链上的合成事实 | 不宣称这些事实来自链上 |
| `identity.kind="chain"` + `chain_id` + `block_hash` | 声明事实属于某条链的一个确切区块 | 不验证提供者返回的状态真实性 |
| 每账户 `code_hash` | 原始代码字节 | 不绑定 storage、余额、nonce |
| `fingerprint` | fork、identity、完整初始账户事实 | 一致性标识不是链状态的密码学证明 |

world JSON 中链上身份的 `chain_id` 使用 `0x` 十六进制格式，`block_hash` 始终是完整 32 字节 hash。RPC CLI 自动读取 chain ID；可用 `--block-hash` 指定完整 hash，或用互斥的 `--block-number` 指定区块号。两者都省略时，启动时读取一次 `latest`。区块号与 `latest` 都先解析为 hash，后续状态查询固定使用这个 hash；发现 callee、链头前进或多轮重跑都不会重新选择区块。`--block-number` 接受十进制或 `0x` / `0X` 十六进制，范围为 `0` 到 `2^64−1`；world JSON 的身份格式不变。fork 仍单独选择，身份不会替你选择执行规则。`--block-number` / `--block-hash` 选择账户状态快照；`--evm.number` 只覆盖 NUMBER，`--evm.chain-id` 只覆盖 CHAINID，不改变 `.world.identity` 中实际采集的链和区块。省略 `--evm.chain-id` 时，链上 world/RPC 的 CHAINID 取快照身份中的 chain ID；没有链身份的离线输入则保持未知。省略 NUMBER、TIMESTAMP、COINBASE、PREVRANDAO、GASLIMIT 和可用费用字段时，核心入口从固定快照的区块头与费用观测填入默认值；父块 hash 也保留为 BLOCKHASH 观测。显式执行环境覆盖仍优先，交易 caller、calldata、value 等仍保持符号输入；区块快照不等于具体交易重放。

链上 JSON 提供代码时必须同时提供匹配的 `code_hash`。runtime 按原始代码字节计算 Keccak；[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 委托标记按原始 23 字节计算；已确认 absent 的账户 hash 为零。输入可附带预期 fingerprint；解析器拒绝指纹失配、重复地址/slot、冲突代码 hash 和不合法的 absence 事实。

`existence` 分为 `unknown`、`present`、`absent`。**空代码与账户不存在不是同一事实**。省略 nonce/balance/existence 表示未知；未列 slot 默认未知。只有确实知道所有未列 slot 为零，才能声明 `storage_unknown:false`；RPC 只采集请求的 slot，其余保持未知。

### 可选实验：从固定区块采集

这一段需要你自己的 RPC，不是离线例子的必要步骤。将 `LAB_RPC_URL`、`LAB_FORK`、`LAB_ENTRY` 分别设为实际提供者 URL、所选区块的执行规则和目标地址。省略 caller、origin、value 和 calldata 时使用符号调用输入，origin 默认与 caller 共享同一身份；需要固定某次调用时使用 `--evm.*`，参数见[第 13 课](13-evm-environment.md)。这里的入口应是该区块上的实际账户；前面合成例子的 `0x...0101` 不代表链上部署。

**第一步，只指定入口，观察发现的账户和槽。** 本次启动读取一次 `latest` 并固定其 hash，默认 RPC 模式会在该区块补查分析中发现的具体 callee 和 SLOAD 所需的有限槽键：

```bash
LAB_RPC_STATUS=0
nix run . -- analyze \
  --rpc "$LAB_RPC_URL" \
  --fork "$LAB_FORK" \
  --evm.to "$LAB_ENTRY" \
  --format json > /tmp/rpc-discovery.json || LAB_RPC_STATUS=$?
printf 'analysis exit=%s\n' "$LAB_RPC_STATUS"

case "$LAB_RPC_STATUS" in
  0|2)
    jq '.status, .world.identity, .rpc_acquisition,
        [.frontiers[] | {from, pc, reason}]' /tmp/rpc-discovery.json
    LAB_BLOCK_HASH=$(jq -er '.world.identity
      | select(.kind == "chain") | .block_hash' /tmp/rpc-discovery.json)
    ;;
  *)
    unset LAB_BLOCK_HASH
    printf '初始分析失败；检查 stderr，先不要运行后续对照。\n' >&2
    ;;
esac
```

先核对退出码：0 表示 `Converged`，2 表示已有 `Incomplete` 报告，两者都可以读取 JSON；其他退出码不能按有效报告继续。只有成功读出链上 hash 后，才执行[后面的对照](#rpc-without-discovery)。结果中的 `world.identity` 保存自动读取的 chain ID 与本次固定的 hash。[后面的对照实验](#rpc-without-discovery)使用刚保存的 `LAB_BLOCK_HASH`，使两次分析读取同一个区块。若已选定区块，可在第一条命令里直接加 `--block-hash "$LAB_BLOCK_HASH"` 或 `--block-number 26000000`；这两个参数不能一起传入。

入口自动加载。CALL、STATICCALL、DELEGATECALL、CALLCODE 能确定具体目标时，缺少该账户的代码事实就会触发补查；入口或 callee 的 [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 委托代码也可发现对应实现账户。解析仍只跟随一层委托。预编译由 fork 规则处理，已知代码、已知空代码和确认 absent 的账户都可以复用已有事实。

SLOAD 的槽键有完整有限候选集合时，缺少初始值会触发 `MissingStorage {address,slot}`。分析器在轮次之间用 `eth_getStorageAt` 补查这些槽，然后从入口重跑；例如代理从具体 slot 0 读取实现地址，获得槽值后可继续发现实现代码。读取按当前帧的状态账户定位；DELEGATECALL 或 CALLCODE 的实现代码仍读取代理的槽，[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 委托执行也保留委托账户的状态身份。

只加载入口不保证 `Converged`；符号 calldata/value 可能使更多路径可达，未知范围、调用目标或累计资源都可能留下前沿。无限或无法完整枚举的槽键保持保守未知，不会尝试穷举 256 位槽空间；由这样的值形成的调用目标仍可能留下 `UnknownTarget`。动态获取账户会观察其 code、balance 和 nonce；其余未观察且尚未被有限 SLOAD 请求的初始槽仍未知。

**第二步，跟着 A→B→C 理解补查后的重跑。** 假设 A、B 代码中的调用地址具体，预算足够，并且 RPC 能返回全部所需账户：

```text
第 1 轮：只有 A 的初始事实 → 遇到缺少 B 代码的调用
补查 B：在同一 block hash 采集账户观察，加入采集缓存
第 2 轮：用 A、B 的初始事实，从 A 入口重新分析 → 发现 C
补查 C：仍使用同一 block hash
第 3 轮：用 A、B、C 的初始事实，从 A 入口重新分析
```

一轮可能同时发现多个账户或槽，所以上面是理解顺序的例子，不是固定轮数公式。同一轮缺少的有限槽集合按地址和槽键去重后采集。每次成功补充事实后，图、Store、调用检查点和摘要都由入口重新建立。A 在 CALL 前做过的写入会重新执行，后来的重入仍读当前 Store。这样能把 B 的区块初始余额与 A 给 B 转账后的余额放在各自正确的位置。

采集缓存保存的是区块事实。B 在获知 C 的代码或初始槽值后 REVERT，撤销的是 B 与更深调用的事务效果，获取的初始事实仍可供 A 后续调用使用。同一账户和同一 `(address,slot)` 不重复采集，返回零也是已观察值。SLOAD 先考虑当前 Store：已经强写的槽直接读事务值；弱写可能与初始值合并，补查不能用链上原值覆盖这些写入。[第 2 节](#2-create先执行构造代码再安装运行时代码) CREATE 已安装的 runtime 属于当前 Store 的代码覆盖层，后续 CALL 使用该 runtime；创建碰撞所需的初始存在性等事实仍需预先提供。

此时再读 `rpc_acquisition`：

| 字段 | 怎样读 |
| --- | --- |
| `rounds` | 启动的分析轮数，包含被预算中断的最终轮 |
| `fetched_accounts` | 按需新增成功的地址数组；入口与预选账户不计入 |
| `failed_accounts` | 按需采集失败的地址数组；本次分析不自动重试这些账户 |
| `fetched_storage` | 按需成功获取的 `{address,slot}` 数组；显式 `--slot` 的初始采集不计入 |
| `failed_storage` | 按需采集失败的 `{address,slot}` 数组；本次分析不自动重试这些槽 |
| `failures` | 按地址配对的错误类型与固定快照上下文；槽错误另带 `slot`，后续预算中断也保留这些证据 |
| `requests` | 初始与后续采集尝试的 HTTP 请求总数，包含身份校验和失败请求 |
| `states_created` | 各轮累计分配的机器状态数，包含已被后续轮次替换的图 |

最终 `world` 保存来自受信任提供者的初始观察，`states` / `edges` / `outcomes` 属于最后一轮分析；累计工作和请求数则覆盖全部轮次。新账户或槽事实增加后 fingerprint 随之改变，旧一轮的摘要不会沿用到新一轮。

<a id="rpc-without-discovery"></a>

**第三步，关闭发现，做初始事实的对照。** 保留同一个目标地址、固定区块与 `--evm.*` 输入约束，再加 `--no-rpc-discovery`；若[第一步](#可选实验从固定区块采集)省略 calldata，两次都分析未知输入集合，而不是某次已指定的 calldata：

```bash
: "${LAB_BLOCK_HASH:?先完成第一步并保存固定的链上 hash}"
LAB_SELECTED_STATUS=0
nix run . -- analyze \
  --rpc "$LAB_RPC_URL" \
  --block-hash "$LAB_BLOCK_HASH" --fork "$LAB_FORK" \
  --evm.to "$LAB_ENTRY" --no-rpc-discovery \
  --format json > /tmp/rpc-selected.json || LAB_SELECTED_STATUS=$?
printf 'analysis exit=%s\n' "$LAB_SELECTED_STATUS"

case "$LAB_SELECTED_STATUS" in
  0|2) jq '.status, [.frontiers[] | {from, pc, reason}]' /tmp/rpc-selected.json ;;
  *) printf '初始分析失败；检查 stderr，不能按有效 JSON 继续。\n' >&2 ;;
esac
```

`--no-rpc-discovery` 同时关闭动态账户和槽采集。未预选的槽按未知值分析，不产生按需补查请求；这可能使后续调用目标未知。如果入口调用了尚未选择的普通代码账户，这次会留下 `MissingCode`、退出 `2`，与[第 9 课的离线缺代码实验](09-cross-contract.md#7-输入缺失和预算停止也要读出来)有相同边界。若入口没有这类调用，也可能直接完成；不能预设任意入口都缺代码。

你仍可用 `--account` 选择分析前获取的账户，用 `--slot` 选择初始存储槽。假设已确定一个被调用账户，将其地址设为 `LAB_CALLEE`，可以在上面的命令中增加：

```text
--account "$LAB_CALLEE" --slot "$LAB_CALLEE:0"
```

这两个参数都可重复，`--slot` 自动加入所属账户，并在分析前获取该初始值；之后 SLOAD 复用它，不再重复请求。slot 索引 `0` 是十进制，也可写为 `0x0`；地址仍是 20 字节十六进制。默认发现可继续补查其他有限 SLOAD 槽；关闭发现后，未预选槽保持未知。code 为空、balance 与 nonce 为零，并且已查询的 slot 也全为零时，RPC 观察不足以区分空账户与不存在账户，`existence` 保持 `unknown`，不会将其余 slot 视为零。非空代码或任一非零 balance、nonce、slot 表明账户存在。离线 world 仍可明确声明已知的 absent 事实。数值的十进制写法只改变 CLI 输入方式，world JSON 格式与发给 RPC 的编码不变。

### 为什么补查也必须固定区块

启动时从 RPC 读取 chain ID，并将所选区块解析为 hash。code/balance/nonce/storage 请求均使用 [EIP-1898](https://eips.ethereum.org/EIPS/eip-1898) 的选择器 `{blockHash,requireCanonical:true}`；初始账户、后来发现的账户和槽使用同一个 hash。RPC 必须支持该选择器。提供者不支持、固定区块因重组失去 canonical 身份或请求失败时，采集停止；不会重新读取 `latest`，也不会改用新的 hash。`eth_getStorageAt` 的响应必须是完整 32 字节数据，短数量、超长数据或缺少结果都不能当成有效值。补查批次全部请求和最终身份检查成功后，观察才加入 world。

初始采集失败时还没有有效分析结果，CLI 退出 `1`。分析已开始后，仍被需要的补查失败在最后一轮图的对应调用或 SLOAD 留下 `RpcAcquisition` 前沿，退出 `2`。其 `failure` 保存 `context`、错误 `kind` 和 `message`，可定位 chain/block/method/account/slot；槽采集的错误即使发生在身份检查，也保留 `context.slot`。额度错误还给出 `resource` 与 `limit`。如果后续轮次在到达该指令前耗尽执行预算，图显示资源前沿，累计 `failures` 仍保存此前的采集错误。JSON-RPC、HTTP、网络、超时、缺少结果或区块身份失配都需要按这个边界判断，不能把剩余成功 outcome 当成完整结果。

槽批次有多个键时，任一请求失败都会丢弃整个批次。`failures[].slot` 与 `failed_storage` 标明无法安装的观测键；`failure.context.slot` 标明实际失败的 storage 请求。例如读取 slot 0 成功、slot 1 失败，两项受影响观测的错误上下文都可指向 slot 1。错误发生在链或区块校验时没有单独的 storage 请求槽，控制流程则将受影响槽补入上下文。

默认 `--max-rpc-accounts 256` 限制初始与动态账户总数，`--max-rpc-requests 16384` 限制全部 HTTP 尝试，包括身份检查与失败请求。迟发采集耗尽这些额度也留下 `RpcAcquisition`。所有重跑轮次另共用同一 `--max-work`、`--max-transfers` 和 `--max-states`；以前的图虽然被替换，已耗工作和状态分配仍计费，因此最终图的 `states` 长度可能小于 `states_created`。达到执行预算后保留对应工作或资源前沿。默认每请求超时 15 秒，响应最多 4 MiB；库 API 可选择其他有界限制。

所有状态查询在启动解析后始终使用固定 hash。槽批次采集前后还用 `eth_getBlockByNumber` 查询该 hash 对应的固定高度，要求当前 canonical hash 与启动身份一致；因此即使节点仍能用旧 hash 返回区块，也能在安装前识别该高度发生的重组。这个高度检查不会重新选择分析区块；失败后不回退到 `latest` 或改用新 hash，也不会把缺失响应填成空代码、零余额。**当前完全信任选定的 RPC 提供者。** 账户与槽位观察直接来自普通状态查询，不请求 `eth_getProof`，也不依赖节点的历史 proof window。代码 hash 根据获取的原始代码字节计算；区块身份与 fingerprint 用于记录和固定输入，不证明提供者返回的状态真实，也不能让 `Converged` 成为任意合约安全证明。

库入口同样显式选择 RPC：`RpcInput::new(endpoint, fork)` 默认使用 `RpcBlock::Latest`；将 `input.block` 设为 `RpcBlock::Number(number)` 或 `RpcBlock::Hash(hash)` 可选择确切区块。session 启动时解析身份，后续采集与分析重跑始终沿用已固定的 hash。

完整 `analyze` 文本、`explain --verbose`、JSON 和 DOT 都保留 snapshot identity、fingerprint、帧模式/hash 和摘要信息。完整文本中，`Snapshot` 查输入身份，`Outcomes` 查每个最终结果的账户和字节，`Call summaries` 查保存与复用；默认 `explain` 按[第 9 课的分区](09-cross-contract.md#默认文本怎样读)阅读代码、CFG、值流和结果概要。JSON 还保留初始 world、完整域策略、摘要输入/输出、执行中的 Store 和 RPC 累计采集记录；DOT 的蓝色证书节点标出认证来源和复用位置。实现入口是 [`world/snapshot.rs`](../crates/evm-abstract/src/world/snapshot.rs)、[`world/rpc/session.rs`](../crates/evm-abstract/src/world/rpc/session.rs)、[`analysis/rpc.rs`](../crates/evm-abstract/src/analysis/rpc.rs)；本地 HTTP 的实际 CLI 对照在 [`tests/cli/rpc.rs`](../crates/evm-abstract-cli/tests/cli/rpc.rs)，有限槽、缓存与迟发错误回归在 [`tests/cli/rpc/storage.rs`](../crates/evm-abstract-cli/tests/cli/rpc/storage.rs)。

直接阅读同一固定 RPC 分析，可以把上面的 `analyze` 换成 `explain`，并去掉 `--format json` / `--ssa`：

```bash
nix run . -- explain \
  --rpc http://127.0.0.1:8545 \
  --evm.to "$LAB_ENTRY"
```

这条最小命令在启动时固定一次 `latest`，并使用符号调用输入；不预设其结果为 `Converged`。明确空 calldata、零 CALLVALUE 等约束时，分别添加 `--evm.calldata 0x`、`--evm.value 0`，不能靠省略参数表达这些值。重现已有分析时，加 `--block-hash "$LAB_BLOCK_HASH"`；按区块高度选择时，用 `--block-number`。两种选择互斥。

默认 RPC `explain` 同样使用教学视图；在上面的命令末尾追加 `--verbose` 可查看完整 RPC 采集记录、快照与机器报告及原始 SSA。`explain` 复用 `analyze` 的 RPC 获取与按需发现策略，包括 `--no-rpc-discovery`、`--max-rpc-accounts` 和 `--max-rpc-requests`。这些开关及 `--account` / `--slot` / `--block-hash` / `--block-number` 仅用于 RPC；离线世界已经携带 fork，不能另传 `--fork`。世界和 RPC 入口要求 `--evm.to`，四种来源 `--world`、`--rpc`、`--hex`、`--file` 互斥。

解释中只有实际帧捕获的代码被列入执行目录：委托代码按 code address 区分，CREATE initcode 与安装后的 runtime 还按代码 hash 和模式区分。只观察到的初始代码有独立标注，不等同于执行证据。RPC 补查或执行预算未完成时，默认视图继续打印部分 CFG、每个已知 outcome、所有诊断与 frontier，并省略完整 SSA，退出码为 2；`--verbose` 保留同一部分分析的完整报告。简化显示不会把 `Incomplete` 改成成功，也不会重新选择 fork、区块或重置预算。

### 按原符号输入查看 WETH 的部分 SSA

[第 13 课](13-evm-environment.md#1-默认输入与目标账户)的 WETH 命令保持 caller、value、calldata 为符号输入。只增加部分 SSA 开关即可阅读已经产生的值流，无需先把输入改成具体调用：

```bash
PARTIAL_SSA_STATUS=0
NO_PROXY=127.0.0.1,localhost,::1 no_proxy=127.0.0.1,localhost,::1 \
nix run . -- explain --rpc http://127.0.0.1:8545 \
  --evm.to 0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2 \
  --allow-partial-ssa || PARTIAL_SSA_STATUS=$?
printf 'exit=%s\n' "$PARTIAL_SSA_STATUS"
```

这段需要本机 RPC 上的实际 WETH 代码与状态。启动时仍固定一次 `latest` 的区块 hash，后续采集沿用该 hash；复现某次报告时加 `--block-hash` 选择报告中的同一 hash。开关不补齐未知目标、不放宽预算，也不缩小 caller、value、calldata 的范围。若分析仍为 `Incomplete`，默认文本先读指令与值流、已支持转移、异常标记与全部前沿，退出码仍为 2；若收敛，则显示完整 SSA 并退出 0。

要展开所有指令阶段与效果链，在这条 world/RPC `explain` 命令上加 `--verbose`；核对同一分析输入时同时指定报告中的 `--block-hash`。默认隐藏的 Completed/Dispatched 注释与效果链只是显示详略，Started/OperandsConsumed/Faulted、Stale/Unexecuted、延后或开放的边与全部前沿继续可见。根帧的 RETURN 结果读 `Outcomes`，子帧的返回则沿 Return 转移追踪。

需要 JSON 时，将命令改为 `analyze`，同时加 `--ssa --allow-partial-ssa --format json`；未完成结果为 `{"analysis":...,"partial_ssa":...}`，状态和 RPC 采集记录在 `.analysis` 内；JSON 保留完整 progress、效果及覆盖字段。部分 φ 不声称覆盖所有调用路径，旧执行输入失效时也不会继续显示其旧指令正文。完整字段读法见[第 04 课](04-ssa.md#7-未完成时按需查看部分-ssa)。

关系约束也是摘要输入的一部分。复用完整 callee 图时，调用输入中的符号保持绑定，callee 内部新产生的未知值会重新命名；悬挂 caller 中上一次调用的结果不会因此变成这次调用的同一个变量。关系的合并和资源规则见[第 15 课](15-symbolic-relations.md)。
