# 13：明确 EVM 环境与符号输入

学习入口：[模块：字节码、协议与输入](modules/execution.md) · [理论：输入范围与语义](routes/theory.md#execution) · [实现：入口参数与环境](routes/implementation.md#execution) · [选择路线](learning-routes.md)。

分析的输入范围决定图覆盖哪些调用。世界或 RPC 只提供账户代码和初始状态；调用者、调用金额和 calldata 是另一组输入。

## 1. 默认输入与目标账户

`--evm.to ADDRESS` 选择被调用合约，它是 root frame 的 ADDRESS 和状态账户。世界/RPC 输入必须提供它；单段 bytecode 的 cfg、ssa、explain 也可提供它，省略时逻辑 ADDRESS 为未知地址。

```bash
NO_PROXY=127.0.0.1,localhost,::1 no_proxy=127.0.0.1,localhost,::1 \
nix run . -- explain --rpc http://127.0.0.1:8545 \
  --evm.to 0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2
```

这个命令采用符号 caller、任意 U256 value、未知长度和内容的 calldata。未指定 origin 时，它与 caller 是同一个输入，反复读取仍保持这种关系。JSON 中 `origin:null` 表示这个 caller 关联，而非另一个独立未知地址。显式 `--evm.caller` 会同时确定默认 origin；显式 `--evm.origin` 可以覆盖 origin。给定部分字段只收窄这些字段，不会把其他字段自动补成零。

保持这组符号输入而查看已执行的值流时，可加 `--allow-partial-ssa`；前沿与退出码 2 仍保留。命令与覆盖边界见[WETH 部分 SSA 实验](10-snapshots-summaries-creation.md#按原符号输入查看-weth-的部分-ssa)。

要分析空数据、零金额、具体 caller 的调用，显式提供它们：

```bash
nix run . -- explain --world examples/worlds/call-return-branch.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.calldata 0x --evm.value 0
```

输出记录本次 EVM 环境。符号输入可能使更多分支可达，或者遇到未知调用目标、内存范围或预算前沿；这些情况保留 `Incomplete`。`Converged` 仍表示在所声明输入和模型范围内完成传播，不表示所有值都精确或合约安全。

上面的 RPC 命令可能退出 2 并留下部分结果；不要把它当作运行失败后改用空输入，也不要把 exit 2 改称收敛。若要比较符号输入和具体输入，先保存本次报告里的快照 hash，再让第二次启动使用 `--block-hash`，避免两次 `latest` 解析得到不同区块。[第 10 课](10-snapshots-summaries-creation.md#可选实验从固定区块采集)给出保留退出码和复用 hash 的命令。

### 先用离线命令检查默认关联

下面不需要节点。CALLER 跨过一个 JUMP 后与 ORIGIN 比较；默认 origin 与 caller 是同一个输入，因此结果为 1：

```bash
nix run . -- cfg --hex 336004565b321400 --format json > /tmp/evm-default.json
jq '.environment | {to, caller, origin, value, calldata}' /tmp/evm-default.json
jq '.states[-1].exit_stack[0].Constants' /tmp/evm-default.json
```

第一条查询包含 `to={"Symbolic":"To"}`、`caller={"Symbolic":"Caller"}`、`origin=null`、`value="Top"`，calldata 的 length 和 default 都为 `"Top"`。第二条输出 `["0x1"]`。原始字节码没有给定 to，所以逻辑 ADDRESS 未知；其内部状态键不会被当作用户提供的地址。

不同 origin 可以显式提供。例如，caller 与 origin 分别为下面两个具体地址时，EQ 结果为零：

```bash
nix run . -- cfg --hex 33321400 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.origin 0x0000000000000000000000000000000000001001 \
  --format json > /tmp/evm-distinct-origin.json
jq '.states[0].exit_stack[0].Constants' /tmp/evm-distinct-origin.json
```

## 2. 参数与指令

所有执行环境参数使用 `--evm.*` 前缀。cfg、ssa、explain 的单段输入与 analyze/explain 的世界输入共用这些规则；disasm 只解码指令。

| 参数 | 控制的输入/指令 | 省略时 |
| --- | --- | --- |
| `--evm.to ADDRESS` | root ADDRESS、初始代码与状态账户 | 世界/RPC 必填；单段 bytecode 未知 |
| `--evm.caller ADDRESS` | root CALLER | 未知 160 位地址 |
| `--evm.origin ADDRESS` | ORIGIN | 与 root caller 相同输入 |
| `--evm.value NUMBER` | root CALLVALUE | 未知 U256 |
| `--evm.calldata HEX` | CALLDATASIZE、CALLDATALOAD、CALLDATACOPY | 长度与字节未知 |
| `--evm.static` | root 与子帧的写限制 | 普通非 static 调用 |
| `--evm.gas NUMBER` | GAS 的初始剩余 gas 上界 | 未知 |
| `--evm.gas-price NUMBER` | GASPRICE | 未知 |
| `--evm.coinbase ADDRESS` | COINBASE | RPC 快照的 miner；无观测时未知 |
| `--evm.timestamp NUMBER` | TIMESTAMP | RPC 快照的 timestamp；无观测时未知 |
| `--evm.number NUMBER` | NUMBER、BLOCKHASH 的有效范围 | RPC 快照的 number；无观测时未知 |
| `--evm.prevrandao NUMBER` | PREVRANDAO | RPC 快照的 mixHash；无观测时未知 |
| `--evm.gas-limit NUMBER` | GASLIMIT | RPC 快照的 gasLimit；无观测时未知 |
| `--evm.chain-id NUMBER` | 执行 CHAINID | 链上 world 使用固定链身份，离线输入未知 |
| `--evm.basefee NUMBER` | BASEFEE | RPC 快照的 baseFeePerGas；未报告时未知 |
| `--evm.blob-basefee NUMBER` | BLOBBASEFEE | 固定区块的 blob fee 观测；未报告 blob 字段时未知 |
| `--evm.block-hash NUMBER:HASH` | BLOCKHASH 的历史 hash 表，允许重复参数 | 有效但未观察的项未知 |
| `--evm.blob-hash INDEX:HASH` | BLOBHASH 的索引项，允许重复参数 | 未观察的有效项未知 |
| `--evm.blob-count NUMBER` | BLOBHASH 的数量边界 | 未知数量 |

数量和索引接受 ASCII 十进制或 `0x` / `0X` 十六进制，范围为 U256；地址为 20 字节，hash 为 32 字节，calldata 为字节串。显式零与参数省略不同。同一索引给出不同 hash 会被拒绝。

PC、CODESIZE/CODECOPY、MSIZE、RETURNDATASIZE/RETURNDATACOPY 根据执行中的代码、memory 和最近子调用返回值计算；BALANCE、SELFBALANCE、EXTCODE 系列以及 storage 读取来自 World/Store。这样，固定代码和账户事实与调用输入保持各自的含义。内存扩容、MSIZE 与数据复制的具体计算见[第 14 课](14-memory-model.md)。

### 在 JSON 中查输入范围

分析结果格式的 `schema_version` 是 4，数值域策略中的 `domain_spec.schema_version` 是 2。输入环境所在位置随输出入口变化：

| 命令 | 环境路径 |
| --- | --- |
| `cfg --format json` | `.environment` |
| `ssa --format json` | `.analysis.environment` |
| `analyze --format json` | `.entry.environment` |
| `analyze --format json --ssa` | `.analysis.entry.environment` |

世界结果中，`.states[].key.frames[].address` 是状态账户，同一 frame 的 `.address_value` 是逻辑 ADDRESS，`.caller` 是有类型的调用者；具体地址表示为 `{"Concrete":"0x..."}`，符号地址表示为 `{"Symbolic":"Caller"}` 等。text/DOT/SSA 将这些身份以可读名字展示。符号作用域的内部编号不进入 JSON，跨报告比较符号名字不能建立相等关系。

## 3. 固定快照与执行环境

RPC 启动时读取 chain ID，并把所选区块解析为一个固定 hash，同时保存该区块的有类型 header 观测。省略 `--block-hash` 和 `--block-number` 时，`latest` 只解析一次；后续采集、callee 发现和分析重跑沿用同一个 hash。

`--block-number` / `--block-hash` 选择账户状态快照。核心世界入口从这份快照填入省略的 NUMBER、TIMESTAMP、COINBASE、PREVRANDAO、GASLIMIT 和可用费用字段，并保留父块 hash 供 BLOCKHASH 使用；CLI、Web、`rpc::load` 后调用 `analyze_world` 都遵循同一规则。header 报告 blob 字段时，采集固定高度的 `eth_feeHistory` 费用观测，并在前后验证 canonical hash，避免把同一 VM fork 内不同 blob 参数阶段混为一谈。缺失的可选费用观测不被伪造为零。

显式 `--evm.number`、`--evm.timestamp` 等参数优先于快照默认值；`--evm.chain-id` 也可以覆盖执行 CHAINID。这些覆盖不会改变 RPC 查询的数据来源，输出同时保留固定快照身份、原始 header 观测和有效执行环境。caller、origin、calldata、value、交易 gas price 和 blob 列表仍是交易输入，不能从区块头推定；省略时继续遵循上表的符号输入规则。

## 4. 索引 hash 与 gas

BLOCKHASH 对当前区块、未来区块和超过 256 块历史窗口的输入返回零。给定 NUMBER 后，只有范围内的表项可以返回提供的 hash；缺失的有效表项保持未知。NUMBER 未知时，查询可能处于有效或无效范围，必须保留两种可能性。

BLOBHASH 的索引超出已知数量时返回零；数量内未观察的 hash 保持未知。`--evm.blob-count 0` 表示已知空 blob 列表。一个显式 hash 观察建立该索引存在；数量未知不会自动补成零。显式数量与 hash 索引矛盾时，输入被拒绝。

下面用离线环境验证历史窗口和 blob 数量边界：

```bash
LAB_HISTORY_HASH=0x1111111111111111111111111111111111111111111111111111111111111111
nix run . -- cfg --hex 60634060644000 --evm.number 100 \
  --evm.block-hash "99:$LAB_HISTORY_HASH" \
  --format json > /tmp/evm-block-hashes.json
jq '[.states[0].exit_stack[].Constants]' /tmp/evm-block-hashes.json

LAB_BLOB_HASH=0x0111111111111111111111111111111111111111111111111111111111111111
nix run . -- cfg --hex 5f4960014900 --evm.blob-count 1 \
  --evm.blob-hash "0:$LAB_BLOB_HASH" \
  --format json > /tmp/evm-blob-hashes.json
jq '[.states[0].exit_stack[].Constants]' /tmp/evm-blob-hashes.json
```

两份结果的第一个栈槽都是所提供 hash 的 U256 值，第二个槽都是零：区块 100 不属于当前 NUMBER=100 的历史窗口，blob 索引 1 则超出 count=1。JSON 的 U256 会去掉前导零，因此 blob hash 的字串外观可能短于原始 32 字节输入。

`--evm.gas` 是初始剩余 gas 的上界。当前模型没有精确计量每条指令的 gas，因此后续 GAS 保守地落在零到该上界之间，不把每次读取错误地当作同一常量。模型仍保留可能失败结果；gas 上界没有提供具体成功执行的证明。

```bash
nix run . -- cfg --hex 5a00 --evm.gas 21000 --format json > /tmp/evm-gas.json
jq '.states[0].exit_stack[0].interval | {unsigned_lo, unsigned_hi}' /tmp/evm-gas.json
```

这里的无符号区间为 `0x0` 到 `0x5208`（21000），没有断言执行 GAS 时还剩恰好 21000。

## 5. 子调用与符号身份

CALL/STATICCALL 的子帧 caller 来自父帧 ADDRESS，子帧 calldata 来自父帧 memory。CALL 的 value 来自指令参数；STATICCALL 的 value 为零。CALLCODE 保持父帧状态账户，并使用父帧 ADDRESS 作为 caller；DELEGATECALL 继承父帧 caller 和 value。ORIGIN 是事务环境，跨帧保持不变。

每个新环境有独立的输入身份空间，克隆环境保留同一组输入；报告中的符号名字只在该报告内表示这种关系，跨报告相同的名字不证明数值相等。稳定的环境输入身份可跨基本块保留；不同输入和经过运算得到的新值不会因为数值摘要相同而被当成同一个符号。调用摘要比较完整环境及帧输入，避免在不同环境下复用结果。身份、来源和表达式现在分别记录：`identity.input.name` 是固定输入名，`provenance` 只含来源与代码角色，`expression` 描述保留的变量或派生运算；不同标签不能直接当作相等证据。机器状态另外持有关系环境，JUMPI 的 true/false 后继各自保留假设，经证明的矛盾才可删除路径。已知数值会反馈到 NumericValue，变量身份与原表达式仍保留。未知 storage 别名、动态地址、表达式规模、析取精度与资源预算继续具有各自的边界。

### 数值策略与关系策略分别选择

默认符号/关系模式对 product 和 constants-only 都生效。下面只比较同一个 CALLVALUE 在两个分支中的一致性：

```bash
LAB_RELATIONAL_HEX=3480600114600957005b80600214601257005b00
nix run . -- cfg --hex "$LAB_RELATIONAL_HEX" --context-depth 0 --format json > /tmp/evm-relations-on.json
nix run . -- cfg --hex "$LAB_RELATIONAL_HEX" --context-depth 0 --no-relations --format json > /tmp/evm-relations-off.json
jq '.program.blocks as $blocks | [.states[].key.basic_block_index | $blocks[.].start_pc]' /tmp/evm-relations-on.json /tmp/evm-relations-off.json
```

进入 `pc=0x09` 的 true 路径意味着 CALLVALUE=1，后面的 CALLVALUE=2 true 分支 `pc=0x12` 不可行。开启关系时不出现这个基本块；关闭关系后保留这条抽象可能。两种模式都可以完成传播，覆盖精度不同；`--no-relations` 没有改变输入 value 的未知范围，也不解除默认 caller/origin 的同一输入身份。

关系参数不属于 EVM 环境值，因此使用独立选项：

| 参数 | 默认值 | 控制的范围 |
| --- | --- | --- |
| `--no-relations` | 不传则开启 | 禁止表达式传播与持久路径约束；保留输入身份和数值语义 |
| `--max-symbolic-nodes` | 1024 | 表达式与一次查询可处理的节点上限 |
| `--max-symbolic-depth` | 64 | 表达式嵌套深度 |
| `--max-relations` | 128 | 一个状态可保留的约束数 |
| `--smt.provider` | `z3` | 进程内求解器，可选 `z3`、`bitwuzla`、`cvc5` |
| `--smt.rlimit` | 100000 | 每次求解检查的工作额度，各求解器的单位不同 |

求解器只使用 `rlimit`，没有墙钟 timeout。资源不足、表达式不能编码或 solver 未给出证明时，保留未知结果和对应前沿；只有已证实 UNSAT 才能剪掉分支。关系查询与表达式工作还使用整次分析共享的 `--max-work`。库模型、求解流程、摘要重命名和完整回归见[第 15 课](15-symbolic-relations.md)。

相关主题：[EVM内存与抽象字节数组](14-memory-model.md)。从 MSTORE/MLOAD 的字节读写开始，再看未知输入与偏移如何影响内存结果。
