# evm-abstract-lab

这个实验室用 Rust 实现一个 EVM 分析器，帮助你理解：**合约指令怎样计算，执行可能走哪些分支，一个值来自哪里，以及调用另一个合约后状态怎样变化。** EVM 是以太坊执行合约字节码的虚拟机。

分析器根据给定的代码和初始事实，计算可能发生的执行。例如，不知道调用输入时，一条分支可能产生 1，另一条产生 2；分析结果用集合 `{1,2}` 同时记录两种可能。教程用中文讲解，提供[理论与实现两条路线](docs/learning-routes.md)，结合手算指令、实际输出和源码理解同一组问题。

## 先运行一个例子

本地或 Remote SSH 环境需要安装 Nix 并启用 `nix-command` 和 `flakes`。GitHub Codespaces 按[声明式容器配置](.devcontainer/devcontainer.json)自动安装并初始化 Nix；首次创建与已有环境的重建步骤见[开发环境说明](docs/development.md#github-codespaces)。

以下命令在仓库根目录执行；首次构建可能下载依赖，例子本身不需要 RPC、钱包或资金。

```bash
nix run . -- explain --file examples/straight-line.hex
```

它分析一个计算 `2 + 3`、把结果写入内存并返回的短程序，依次显示三种输出：

| 输出 | 回答的问题 | 先找什么 |
| --- | --- | --- |
| 反汇编 | 每段字节是什么指令？ | `0004: ADD`：加法指令位于字节偏移 4 |
| CFG（控制流图） | 执行顺序和分支是什么？ | `σᵖ₀ \| B₀`、`stack in []`、`stack out []`：一个块，入口和出口栈都为空 |
| SSA（静态单赋值） | 一个值在哪里产生、被谁使用？ | `%2 = CONST 0x5 ; origin=ADD`：保留加法结果的名字，原始依赖记录在值映射中 |

状态、块、帧和效果使用[统一符号](docs/notation.md)，SSA 值名仍用 `%编号`。不知道 `σᵖ₀`、`%2` 或空栈是什么意思，直接读 [00：第一遍运行与输出解读](docs/00-start.md)。它包含完整的逐指令栈表和环境报错处理。

再看有分支的程序：

```bash
nix run . -- cfg --file examples/diamond.hex --context-depth 0
```

这里显式设置 `--context-depth 0`，让两条路径在相同栈高的汇合块合并。默认值是 128，会按最近的不同跳转来源区分状态；参数的含义见[第 05 课](docs/05-sensitivity.md)。

在 `pc=0x000e` 的状态找到 `stack in [{0x1, 0x2}]`。外层是栈，内层是**一个栈槽的可能值集合**；不是栈上同时有两个值。这一步连接具体执行与抽象分析。

小集合只是描述可能值的一种方式。候选太多时，默认分析还会保留固定的位、数值范围和同余性质；例如，即使不能逐个列出所有偶数，也能知道最低位为零。[第 02 课](docs/02-domain.md)从集合手算过渡到这些性质，并用 `--numeric-model finite-set` 比较仅保存有限集合的结果。

位约束用完整的 64 个十六进制位置表示：一个位置全部已知时显示 `0`–`f`，全部未知时显示 `*`，部分已知时用四个二进制位显示，如 `[01**]`。其中 `[01**]` 只占一个十六进制位置，表示从高到低的两位确定为 0、1，另外两位未知；位模式中的所有未知位置都保留，不使用省略号。整个值没有数值约束时显示 `⊤`（Top）；有限候选仍显示为 `{0x1, 0x2}`。有其他数值约束时，`bits=` 后的全未知位模式仍保留 64 个 `*`。[第 12 课的位模式](docs/12-product-domains-facts.md#怎样读十六进制位模式)给出逐位读法。

<a id="推荐阅读顺序"></a>

## 按路线或模块阅读

先完成 [00：运行与输出](docs/00-start.md) 的第一个实验。想系统学习，可以选择下面的理论或实现路线；只想解决当前问题，可以直接进入[专题模块](docs/modules.md)。模块决定学习范围，两条路线提供不同视角，正文和实验共用。章节编号用于查找，不要求从 00 一直顺读到 16。

| 路线 | 适合带着什么问题阅读 | 推进方式 |
| --- | --- | --- |
| [理论路线：从语义与近似走到实现](docs/routes/theory.md) | 摘要、合并、固定点和 SSA 为什么这样定义？这些定义能保证什么？ | 先建立概念与具体例子，再检查仓库怎样实现、在哪里近似或停止 |
| [实现路线：跟着输入、状态与结果读代码](docs/routes/implementation.md) | 一条命令经过哪些模块，哪个类型保存状态，输出从哪里产生？ | 沿 CLI、机器状态、转换函数、调用与 SSA 追踪，再解释对应理论与不变量 |

[模块总览](docs/modules.md)提供九个可独立进入的专题。比如 [Memory & Storage](docs/modules/memory-storage.md) 分成字节读写、slot 读写、别名与状态归属；[SSA](docs/modules/ssa.md) 分成值名、φ、执行前缀与验证；[RPC 与摘要](docs/modules/rpc-summaries.md) 分成固定快照、补查、复用与生命周期扩展。每页都标出最少前置知识、核心单元和按需深入的部分。

[路线总览](docs/learning-routes.md)按同一组模块连接理论与实现入口。每课顶部也有所属模块和两条路线的链接；[例子索引](examples/README.md)用于选实验，[第 07 课](docs/07-exercises.md)用于核对理解，[参考资料](docs/references.md)用于继续读规范和论文。

如果你正在读 `recorded prefix stack`、`partial φ` 或 `OperandsConsumed`，直接进入 [04：部分 SSA 的逐步实验](docs/04-ssa.md#7-未完成时按需查看部分-ssa)，观察同一程序在执行 work 不足与足够时的栈、指令结果和覆盖差异，再回到所选路线。

未知目标、重复回调与共享状态的完整例子见 [18：世界未知部分的抽象执行](docs/18-unknown-world.md)。它连接符号状态、已知代码执行、交互摘要和可见的覆盖报告；[已知代码自身递归](docs/18-unknown-world.md#已知代码自身递归也进入同一过程表)也使用共同更新的过程关系。

库调用者可以组合[整机状态策略](docs/state-policies.md)，选择实际分区键、完整输入合并、加速调度和过程摘要。该页提供公开 API 示例，并链接普通执行、递归与未知回调的具体执行见证测试。

[第 21 课：上下文与收敛](docs/21-context-and-convergence.md)进一步解释状态身份、状态增长与收敛，并讨论局部上下文和分阶段复用等设计候选。

[代码与初始事实分层](docs/18-layered-analysis.md)介绍如何先只提供代码，再向同一快照注入观察并继续分析。可检查的数学性质及其与生产代码的对应边界见[形式化证明](docs/proofs/README.md)。

要用 Rust 组合自己的数值域或独立分析组件，读[公开 Trait 协作指南](docs/trait-cooperation.md)。它从可运行的离线客户端开始，说明强类型事实、自动依赖调度、代际收缩和生产 EVM 反馈怎样共用同一个内核与工作预算。

## 从单段代码到多个合约

单段 `.hex` 适合学习局部计算。跨合约的 `explain` 和 `analyze` 入口使用**世界文件**（world JSON）：把多个账户的代码、初始存储和其他已知事实放在一起，通过 `--evm.to` 指定 root frame 的目标。下面显式固定 caller、value 和 calldata 来重放教学场景；省略它们时使用符号输入。

```bash
nix run . -- explain \
  --world examples/worlds/call-return-branch.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x
```

`explain` 默认串联实际捕获代码的反汇编、简明 CFG 与独立结果概要、赋值式跨合约 SSA。需要完整帧记录和效果链时加 `--verbose`；完整报告仍由 `analyze` 提供，结构化数据使用 `analyze --format json`。

反汇编与 SSA 的基本块指令列表共用顶格的 `B₀ @ 0x0000:` 标题，指令 pc 写作 `0000:`，数字列与标题中 `0x` 后的数字对齐。SSA 的 σᵢ 状态元数据和入口 φ 在 Bᵢ 标题前独立显示；σᵢ 与 Bᵢ 不是同一种编号。单字节码与世界教学 SSA 共用赋值式指令正文，世界视图另保留代码、帧、状态账户及转移身份；完整视图继续保留所有帧与原始效果证据。示例与读法见[第 04 课](docs/04-ssa.md)。

这个离线实验里，A 调用 B，B 返回 32 字节的数值 1，A 据此选择分支并写自己的存储。输出的 `Call` / `Return` 是调用和返回边，`outcome` 是入口执行结束时的可能结果。模型也保留 gas 不足等失败可能，成功轨迹不等于全部抽象结果。逐步解读见[第 09 课](docs/09-cross-contract.md)。

`analyze` 默认文本及 `explain --verbose` 的完整报告按完成状态、快照、身份引用、执行状态、转移、入口结果、调用摘要、诊断和未完成前沿分区。先看 `Analysis`，再沿 `Transitions` 的 `σᵢ` 编号追踪调用；`Outcomes` 的每个 `Oᵢ` 分别携带自己的返回字节和最终账户状态。`References` 列出地址 `A₀`、hash `H₀` 等短引用对应的完整值，JSON 和 DOT 仍保留完整身份。[文本阅读路线](docs/09-cross-contract.md#默认文本怎样读) 解释各分区及字节表示。

**调用摘要**保存已完成子调用在特定输入下的结果和执行子图，供之后前提相同的调用复用。`Call summaries` 中的保存数量 `published` 与复用次数 `hits` 是两件事；只调用一次时可以保存结果而没有命中。[第 10 课](docs/10-snapshots-summaries-creation.md) 从两次调用的例子开始解释。

```mermaid
flowchart TD
    A[世界文件：账户代码与初始事实] --> C[从入口开始分析]
    B[入口参数：调用者与输入字节] --> C
    C --> D[进入被调用账户]
    D --> E[返回或回滚后继续调用者]
    E --> F[执行图、可能结果与账户状态]
    F --> G[SSA：追踪值与状态效果的来源]
```

世界文件中的 `fork` 选择同一套协议规则。本项目默认 Osaka，支持 Cancun / Prague / Osaka；选择它不会自动验证链上区块属于哪个升级阶段。[第 08 课](docs/08-forks.md) 演示同一字节码在两个 fork 下怎样得到不同结果。

## 命令与输出格式

| 命令 | 输入 | 用途与格式 |
| --- | --- | --- |
| `disasm` | `--hex` 或 `--file` | 只解码指令；text / JSON |
| `cfg` | `--hex` 或 `--file` | 局部抽象栈与控制流；text / JSON / DOT |
| `ssa` | `--hex` 或 `--file` | 默认构建完整原生机器图上的归一化栈 SSA，并提供单程序状态映射；text / JSON；`--allow-partial-ssa` 可查看未完成分析的指令证据 |
| `explain` | `--hex` / `--file`，或 `--world` / 显式 `--rpc` + `--evm.to` | 反汇编、教学 CFG、赋值式 SSA；world/RPC 可加 `--verbose` 展开完整证据 |
| `analyze` | `--world` 或显式 `--rpc`，以及 `--evm.to` | 跨合约图、返回结果、账户状态；text / JSON / DOT；`--ssa` 增加并验证 SSA |

`ssa`、`explain` 可以显式加 `--allow-partial-ssa`；`analyze` 需同时加 `--ssa`，并选择 text 或 JSON。分析为 `Incomplete` 时，这个开关显示带覆盖说明的**部分 SSA**，仍保留全部前沿并退出 2；分析已 `Converged` 时输出经过验证的完整归一化 SSA。部分 SSA 的默认文本突出指令与值流，保留未完成步骤、故障、陈旧或未执行状态、未覆盖边与全部前沿；常见的 Completed/Dispatched 注释与效果链放在 world/RPC `explain --verbose` 或 JSON 中查看。部分 SSA 的指令阶段、未覆盖入边与单程序状态编号映射见[第 04 课](docs/04-ssa.md#7-未完成时按需查看部分-ssa)，常量传播与原始依赖的关系见[SSA 值归一化](docs/ssa-normalization.md)。

用 `nix run . -- analyze --help` 查看全部参数。调用环境统一使用 `--evm.*`：`--evm.to` 确定执行 root frame 的合约，省略 caller、value、calldata 时分别覆盖未知调用者、任意 U256 金额、未知长度与内容的输入；origin 默认与 caller 是同一个输入。显式 `--evm.calldata 0x --evm.value 0` 才表示空数据、零金额。交易与区块环境、索引 hash 和 gas 上界的全部参数见[第 13 课](docs/13-evm-environment.md)；精度与预算参数见[第 05 课](docs/05-sensitivity.md)和[第 06 课](docs/06-boundaries.md)。

CLI 默认选择 `--numeric-model product`：`ProductAlgebra` 的实际载体 `NumericValue` 组合常量集合、KnownBits（固定位）、Interval（区间）、Congruence（同余）和非零保证。`--numeric-model finite-set` 选择独立的 `FiniteSetAlgebra`，实际载体为 `FiniteConstantSet`。两者共用 `ValueAlgebra<D>` 的来源、角色、值身份和表达式处理，执行状态始终保留 `EvmValue<D::Element>`。

应用构造数值策略后，将 `AnalysisInput`、借用的代数、`EnginePolicies` 和组件交给 `AnalysisEngine::run`。单程序 CFG 是原生结果的显式 `EvmAnalysis::from_execution` 投影，保留底层图和状态 ID 映射。该投影、SSA 和所需计算共用独立的有限后处理账本，应用默认 `DEFAULT_POSTPROCESS_WORK=2_000_000_000`；它与执行预算分别计量。CLI 与 Web 的 `--max-constants` 默认 512，接受平台能表示的任意正 `usize`，不会按配置预分配集合；`--reduction-rounds` 默认 16，`--max-facts` 默认 4096。数值与关系限制冻结在代数的 `DomainSpec` 中。CLI、HTTP 和 Web 的应用默认策略统一声明在 [`application_defaults`](crates/evm-abstract-protocol/src/application_defaults.rs)；库默认策略保持独立。

机器状态另外保存关系约束。JUMPI 的后继应用分支条件，通过局部表达式与数值事实推理排除已证明矛盾的路径，并将已证明的数值上界投影回执行值。未证明矛盾的路径继续保留。两种数值模型 默认都启用这层能力；`--no-relations` 用于关闭它的对照。表达式节点、深度和关系数量有独立上限，真实的取消、配置与精度边界仍保留类型化原因。`cfg`、`ssa`、`analyze`、`explain` 与 Web 的 `max_work` 默认 2000000000，覆盖执行、数值与符号工作，并默认启用保守根终止覆盖；仍可显式设置预算。详见[第 12 课](docs/12-product-domains-facts.md)和[第 15 课](docs/15-symbolic-relations.md)。

CLI 的数量参数 `--evm.value` 和 `--slot ADDRESS:SLOT` 中的 SLOT 接受无前缀十进制或带 `0x` / `0X` 前缀的十六进制，范围为 `0` 到 `2^256−1`；`--block-number` 接受相同进制写法，范围为 `0` 到 `2^64−1`。十进制只用数字 `0`–`9`，允许零和前导零；例如 `001` 仍表示 1。可以写 `--evm.value 1000`（单位 wei）、`--block-number 26000000`、`--slot 0x0000000000000000000000000000000000000200:0`。地址、block hash 和 calldata 仍按各自的十六进制字节格式输入；world JSON 的 `chain_id`、余额、nonce、storage 键和值仍使用原有的 `0x` 十六进制格式。

显式选择 `--rpc` 后，只指定 `--evm.to` 即可分析符号调用输入。chain ID 从 RPC 自动读取；省略区块参数时，只在启动时读取一次 `latest`，随后固定返回的区块 hash。也可指定互斥的 `--block-hash` 或 `--block-number`；区块号同样先解析为 hash，再采集状态。分析器发现具体调用目标缺少代码，或 SLOAD 的完整有限槽集合缺少初始值时，会在同一 chain ID、block hash 下补查账户或槽；更深的调用也按需发现。下面的环境变量须已设置为实际提供者、fork 和目标地址。结果可为 `Incomplete`，例如未知输入使调用目标无法确定；这时阅读前沿而不是假定所有调用已覆盖：

```bash
nix run . -- analyze \
  --rpc "$LAB_RPC_URL" --fork "$LAB_FORK" \
  --evm.to "$LAB_ENTRY" --format json
```

RPC 分析默认从同一固定快照填入 NUMBER、TIMESTAMP、COINBASE、PREVRANDAO、GASLIMIT 与可用费用字段。显式 `--evm.*` 覆盖仍优先；caller、calldata、value 等交易输入仍按省略时的符号语义分析。详细规则见[固定快照与执行环境](docs/13-evm-environment.md#3-固定快照与执行环境)。

`--account` 仍可预先选择账户，`--slot ADDRESS:SLOT` 预先采集初始存储槽。默认发现也会为 SLOAD 补查可完整枚举的有限槽键，按状态所属账户读取；DELEGATECALL 读取代理的槽。同一地址和槽只采集一次，零值同样缓存；无限或无法完整枚举的槽键保持保守未知。RPC 使用 `{blockHash,requireCanonical:true}` 采集 code、balance、nonce 和所需 slot，完全信任选定提供者，不请求 `eth_getProof`。代码为空且其余已查询字段全零时，存在性仍为未知；非空代码或任一非零数值则表明账户存在。重组或查询错误不会使分析改用新的区块。

加 `--no-rpc-discovery` 可只使用预先选择的账户和槽：缺代码与未覆盖目标进入保守的未知交互模型，未观察的槽保留未知值；尚未建立覆盖的效果或耗尽的预算仍留下前沿。CLI 默认最多采集 4096 个账户、尝试 1000000 次请求，分别由 `--max-rpc-accounts`、`--max-rpc-requests` 设置。单次响应默认最多 64 MiB、请求超时默认 120 秒，分别由 `--max-rpc-response-bytes`、`--rpc-timeout-ms` 调整。补查成功后会从入口重新分析，各轮共用 work、transfer 和状态分配预算；JSON 的 `rpc_acquisition` 记录累计账户、槽和失败证据。完整实验与错误解读见[第 10 课](docs/10-snapshots-summaries-creation.md#可选实验从固定区块采集)。

需要可视化实际分析结果时，先导出 DOT（Graphviz 的图描述格式），再转成 SVG：

```bash
nix run . -- cfg --file examples/diamond.hex --context-depth 0 --format dot > /tmp/diamond.dot
nix develop -c dot -Tsvg /tmp/diamond.dot -o /tmp/diamond.svg
```

用浏览器打开 `/tmp/diamond.svg`。跨合约命令同样支持 `--format dot`。查看 JSON 与 SSA：

```bash
nix run . -- analyze \
  --world examples/worlds/proxy-storage.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000001000 \
  --evm.value 0 --evm.calldata 0x \
  --format json --ssa > /tmp/proxy.json
nix develop -c jq '.status, (.native.ssa | type)' /tmp/proxy.json
```

这里应得到 `"Converged"` 和 `"object"`。CLI 的组合报告固定保留 `.global.analysis` 和可选 `.native.analysis`，`selected_view` 表示默认展示选择。外层 `status`、`work`、`transfers` 是组合资格与累计计数；每张图保留自己的原始统计。Native 的 `ssa` 或 `partial_ssa` 与该图相邻；请求 SSA 不会改变图的位置。`.hybrid.local_detail` 中的指令阶段和 frontier 索引始终属于 Native 命名空间。更详细的字段解读见[跨合约报告](docs/09-cross-contract.md#默认文本怎样读)和[输入与帧的 JSON 查询](docs/10-snapshots-summaries-creation.md#当前-json-怎样记录输入与帧)。

## 怎样判断结果是否完整

默认 Hybrid 的 `status` 与退出码描述全局控制／效果关系的闭合与来源资格；可选 Native 的状态、frontiers 和 SSA 另行保留。CLI `cfg`、`ssa` 与单程序 `explain` 保留原生指令入口。

| 结果 | 含义 | CLI 退出码 |
| --- | --- | --- |
| `Converged` | 本模型的工作表完成，没有尚待分析的前沿 | `0` |
| `Incomplete` | 缺少事实、遇到模型无法处理的输入或耗尽预算；输出保留原因与停止位置 | `2` |
| 输入错误 | JSON、参数或初始 RPC 采集失败，未得到有效分析结果 | `1`；参数语法错误由 clap 报告并退出 `2` |

`⊤`（Top）表示单值数值摘要没有排除任何 256 bit 数；状态级关系仍可能限制它。Top 属于精度下降；它与 `Incomplete` 的“还有工作未完成”不同。完整 SSA 构建要求完整图；显式选择部分 SSA 只展示有执行证据支持的前缀与覆盖缺口。`Converged` 只描述声明输入范围内的抽象传播完成，不表示某地址的所有调用都已精确恢复，也不构成合约安全证明。先读输出的 `EVM inputs` 或 JSON 中的环境：raw CFG 的路径是 `.environment`，世界分析的原始输入位于 `.global.analysis.entry.environment`，Native 条件位于 `.native.analysis.entry.environment`。当前[原生 schema](crates/evm-abstract/src/analysis.rs) 为 16，[Web wire schema](crates/evm-abstract-protocol/src/lib.rs) 为 21，[域 schema 与成本版本](crates/evm-domain/src/domain/spec.rs)为 3 与 9。原生机器值始终是对象：实际载体在 `.numeric`，来源、身份和表达式分别保存；product 与 finite-set 的完整候选都在 `.numeric.constants`，省略该键表示没有有限集合约束。raw CFG 的 `.states[].entry_relations` 和世界 Native 图的 `.native.analysis.states[].entry.relations` 保存关系信息。

RPC 混合分析保留原始全局关系，并用同一固定快照和累计账本扩展可选精度。后续补查失败或额度耗尽时，已经独立闭合的基线继续有效，采集失败、Native frontiers 与各自 revision 显式保留；基线本身未获认证时仍是 `Incomplete`。初始入口采集失败属于输入错误，取消返回类型化错误。原生 RPC 库入口继续用 `RpcAcquisition` 表示尚需补查的详细义务。

[全局模型与详细路径](docs/20-global-and-local.md)说明保守控制／效果关系、原始输入 namespace、实际区域选择和来源有效期。

当前能力覆盖多账户调用、代理执行、返回数据、persistent/transient storage、嵌套回滚与重入；还包括有明确输入的 CREATE/CREATE2、[EIP-6780](https://eips.ethereum.org/EIPS/eip-6780) 生命周期和原生预编译。调用摘要缓存可以复用已完成的调用分析，同时保留可检查的图与状态效果。[第 09 课](docs/09-cross-contract.md)和[第 10 课](docs/10-snapshots-summaries-creation.md)解释各项条件。

模型处理普通 EVM 字节码，即按操作码及其立即数解码的指令流；不支持 EOF 容器格式。字节码格式与硬分叉版本是两个不同概念，普通 EVM 字节码也能使用所选版本启用的较新指令，见[第一课](docs/01-bytecode.md)。

gas 不精确计量，一般 hash 和未知环境采用保守近似，有界关系域也不提供完整路径可行性或跨交易不变量证明。RPC 仅在显式选择时采集固定区块 hash 的事实，默认按需补查具体被调用账户和 SLOAD 的有限槽键；未知调用目标、无法完整枚举的槽键与未观察的其他槽仍保持边界。采集过程完全信任选定的提供者；区块身份与初始事实指纹用于固定输入，不构成状态真实性的密码学证明。读结果前请确认[详细边界](docs/06-boundaries.md)。

## 浏览器工作台

CLI 与 Web 默认使用 `max_work = 2_000_000_000`，先建立保守的全局过程，再将剩余预算用于原始输入的 Native 细节。**Global model** 与 **Detailed paths** 保存各自的图、状态 ID 和覆盖。`root_terminal_coverage = true` 只请求 Native 阶段的[终止辅助记录](docs/19-root-terminal-coverage.md)，基线始终关闭该选项；`--no-root-terminal-coverage` 只关闭这个附件。全局闭合由实际过程依赖求得，Native 仍可保留 pending、frontiers 与部分 SSA。用 `--allow-partial-ssa` 查看当前 Native 前缀（`analyze` 还需 `--ssa`）；其完成状态独立于全局 status。

```bash
nix run .#web
```

打开命令打印的本地地址，通过 **New analysis**（窄屏为 **Input**）选择 **Address** 或 **Bytecode**。结果默认打开入口及其真实前向一跳的来源 CFG，沿调用进入被调用代码，**Back** 恢复先前的来源、上下文、视图和镜头；邻域、完整来源 CFG、区域模型总览与 Fit 可按需选择。**Sources** 按代码来源和 owner／static 等限定选择上下文；只有解码代码时仍可读 **Code**，**SSA** 则查看相容 Native 上下文中实际保留的 SSA 记录，包含仅有边界效果的记录，并保留部分覆盖或较早输入条件的说明。窄屏首次打开结果时 **Sources / Inspect** 关闭，打开后共用一个锚定面板，输入表单优先占用交互空间。

浏览器通过共享 Rust 类型向原生分析服务发送请求；抽象执行与局部关系推理在后端运行。控件操作、检查器、开发构建和数据接口见[Web 前端说明](docs/web-workbench.md)。CFG 在浏览器主线程使用纯 Rust ELK：首次卡片尺寸与预览密度按实际图区选择，镜头围绕入口及其直接备选定位；普通 resize／Back 保留当前范围的布局偏好与几何。几何边界和当前限制见[Rust ELK 布局说明](docs/elk-layout.md)。

非紧凑布局的状态栏显示运行中的已用时间，紧凑布局通过 **More → Timing** 查看；完成后保留用户总等待、后端处理及阶段分解。RPC 补读和抽象执行的多轮耗时累计，前端获取报告、解析与首次视图准备也单独记录；显示范围见[分析耗时](docs/web-workbench.md#分析耗时)。

分析链上账户时，使用 `nix run .#web -- --rpc-config http://127.0.0.1:8545` 直接传入 RPC HTTP(S) URL；后端自动创建 **Default RPC** 提供者，无需 JSON 文件。需要多个提供者或自定义名称时，`--rpc-config` 也接受 JSON 配置文件路径。浏览器同时显示提供者的名称和 RPC URL，分析请求只提交其 ID。首次打开时，有提供者则使用第一项自动分析 Ethereum 主网 USDC；未配置提供者则运行分支字节码示例，RPC 提交不可用。RPC 表单还可选择 WETH、USDT、Multicall3、Uniswap V2 Router02、V2 Pair（USDC/WETH）、V2 Factory、V4 PoolManager 或自定义入口地址，切换示例不会自动提交。配置格式见 [Web 工作台启动说明](docs/web-workbench.md#启动)，预置地址与来源见[入口示例](docs/web-workbench.md#使用控件)。

CLI 与 Web 保留全部预算和精度控件，共用同一套默认值，并允许显式覆盖；实际分析仍遵循指定的有限预算。完整默认值和 Web 类型范围见 [Web 控件说明](docs/web-workbench.md#使用控件)，CLI 参数见[精度与预算表](docs/05-sensitivity.md#7-实验时分别调整精度与预算)。直接调用 Rust 库时，`Config::default()`、`ExecutionConfig::default()`、`RelationLimits::default()` 与 `RpcInput::new()` 仍使用库自身的配置；`ExecutionConfig::default()` 的 work 预算仍为 20000000，根终止覆盖默认开启。

world/RPC 的 `analyze` 与 `explain` 可用 `--max-detail-regions N` 调整保留私有 Native 上下文的已捕获来源数，HTTP 对应 `limits.max_detail_regions`。默认 1，0 只运行 Global；更多来源继续使用原累计执行预算。请求上限与实际选中来源分别记录，RPC 的最新精度资格也与原始 Global 分开，见[全局模型与详细路径](docs/20-global-and-local.md#当前前缀的范围)。

## 开发环境与实现入口

应用 workspace 的 Rust 使用静态派发，egui/Web 适配模块有局部例外；[动态派发检查](docs/no-dynamic-dispatch.md)说明 Dylint 命令、检查范围和具体错误类型约定。

```bash
nix develop
cargo run --locked -p evm-abstract-cli -- explain --file examples/straight-line.hex
```

`nix develop` 显式提供编译工具、语言服务器、格式器和检查工具。VS Code 和插件由用户安装，仓库提供推荐与设置；接入方式、依赖版本来源和 Nix 模块职责见[开发环境说明](docs/development.md)。核心锁定文件负责不同层次：

| 文件 | 固定什么 |
| --- | --- |
| [`rust-toolchain.toml`](rust-toolchain.toml) | Rust 工具链及组件；当前配置为 1.99.0，Nix 和 rustup 共用 |
| [`Cargo.lock`](Cargo.lock) | Rust 依赖的版本与校验和 |
| [`flake.lock`](flake.lock) | Nix 环境、构建工具及输入提交 |

版本升级需要显式修改并重新验证，不会跟随浮动的 `stable` 自动变化。

| 读什么实现 | 源码入口 |
| --- | --- |
| 指令解码与基本块 | [`bytecode.rs`](crates/evm-bytecode/src/bytecode.rs) |
| 常量集合的 Top、非空集合与有界运算 | [`finite_constant_set.rs`](crates/evm-domain/src/domain/finite_constant_set.rs) |
| 数值组件、合并与算术 | [`numeric.rs`](crates/evm-domain/src/domain/numeric.rs)、[`domain.rs`](crates/evm-domain/src/domain.rs) |
| 机器值、来源与独立身份 | [`value.rs`](crates/evm-domain/src/domain/value.rs)、[`provenance.rs`](crates/evm-domain/src/domain/provenance.rs)、[`identity.rs`](crates/evm-domain/src/domain/identity.rs) |
| 持久表达式、关系环境与查询 | [`symbolic.rs`](crates/evm-domain/src/domain/symbolic.rs)、[`relational.rs`](crates/evm-domain/src/domain/relational.rs) |
| 固定位、区间、同余、来源与事实交换 | [`domain/`](crates/evm-domain/src/domain) |
| 调用帧、局部与跨合约工作表 | [`analysis/`](crates/evm-abstract/src/analysis) |
| 账户事实与账户状态 | [`world/`](crates/evm-abstract/src/world) |
| 内存、calldata 与 returndata 的抽象字节数组 | [`world/bytes.rs`](crates/evm-abstract/src/world/bytes.rs) |
| 值的命名与结构验证 | [`ssa/`](crates/evm-abstract/src/ssa) |
| 身份显示符号与状态搜索 | [`evm-abstract-notation`](crates/evm-abstract-notation) |
| CFG 空间分组与 Rust ELK 几何 | [`evm-abstract-layout`](crates/evm-abstract-layout) |
| 命令参数、世界文件解析 | [`evm-abstract-cli`](crates/evm-abstract-cli) |
| 浏览器共享数据协议 | [`evm-abstract-protocol`](crates/evm-abstract-protocol) |
| 原生分析服务与静态资源 | [`evm-abstract-server`](crates/evm-abstract-server) |
| egui 自绘控件与 Web 适配 | [`evm-abstract-web`](crates/evm-abstract-web) |

本仓库实现抽象 transfer（指令怎样更新抽象状态）、工作表、局部关系推理与栈到 SSA 的转换；通用部分复用 `revm-bytecode` 的指令元数据、`alloy-primitives` 的 U256、`petgraph` 的图算法、`revm-precompile` 的原生计算、Axum / Hyper 的 HTTP 服务，以及 Alloy 的 RPC client 与 Ethereum 类型。RPC 的有界传输适配使用 `reqwest`，固定快照和按需采集策略由分析器管理。通用取消作用域由无求解器依赖的 `analysis-control` 提供；测试使用 revm 具体执行和 proptest 性质检查。依赖资料见[参考页](docs/references.md)。

## 验证与源码导航

日常小任务使用自动受影响检查；每次 PR 合并前，再对最终提交执行完整门禁。准备提交、运行检查与记录结果的步骤见[本地检查流程](docs/local-ci.md)。

```bash
nix run --no-update-lock-file .#check-changes --
# 已提交的任务可比较任务开始前的提交或 origin/main
nix run --no-update-lock-file .#check-changes -- --base origin/main --plan
nix flake check --print-build-logs --no-update-lock-file
```

安装与运行生成经过运行性能优化的 release 产物；工作区测试使用快速的 test profile，测试通过与否由 `nix flake check` 验证。构建没有项目固定的并发上限或 CPU 预留，profile 和 Nix 缓存分层见[构建说明](docs/development.md#构建-profile-与缓存)。

完整门禁检查默认、imbl、全部 features 的工作区测试与 doctest（包含 ignored 测试）、Clippy、Rust 格式、Rustdoc、Nix 格式、TOML 语法与格式、离线文档链接、Lean 形式化模型检查，以及打包后二进制的例子、图输出和真实浏览器交互。按包、fixture 和浏览器用例缓存检查，等价 feature 配置和重复别名复用同一 derivation。开发时也可以手动指定 Cargo 范围：

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --locked
```

TOML 检查使用 Nix 中固定版本的 Taplo，规则见 [`taplo.toml`](taplo.toml)。它递归检查仓库的 TOML 文件并覆盖隐藏目录，排除构建产物和 Git 元数据；语法检查禁用联网 schema 验证。开发时检查或统一格式：

```bash
nix develop --command taplo lint --no-schema
nix develop --command taplo fmt --check
nix develop --command taplo fmt
```

VS Code 请以仓库根目录打开工作区，按[插件推荐](.vscode/extensions.json)自行安装 rust-analyzer 和 Even Better TOML，再执行 `Developer: Reload Window`。Remote SSH 在远端工作区配置插件和 Nix。仓库通过[工作区设置](.vscode/settings.json)连接语言服务器，通过[Nix 任务](.vscode/tasks.json)执行构建、测试和 Clippy；详细步骤见[开发环境说明](docs/development.md)。

TOML 编辑器设置与命令行共同读取 [`taplo.toml`](taplo.toml) 的 `[formatting]`：4 空格缩进、对齐键值、最多一个连续空行。工作区指定 [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) 为格式器并启用保存时格式化。插件默认使用自带 Taplo，CLI 的 Taplo 由 Nix 固定，两者版本可能不同。

修改格式规则时以 [`taplo.toml`](taplo.toml) 为准。完整门禁通过 Nix Taplo 的真实 LSP 请求比较格式化结果与同版本 CLI，验证项目配置；它不验证用户安装的任意插件版本。

文档链接规则由 [`lychee.toml`](lychee.toml) 固定。离线检查覆盖本地文件、引用式链接与锚点，包含隐藏目录文档，排除 `target`、`result*`、`.git`、`.direnv` 的生成文件：

```bash
nix develop --command lychee --offline --root-dir "$PWD" -- README.md '**/*.md'
```

需要检查 HTTP(S) 外链状态时，显式开启网络：

```bash
nix develop --command lychee --offline=false --scheme http --scheme https --include-fragments=none -- README.md '**/*.md'
```

联网命令不检查远端锚点；外站超时或限流会导致失败，所以它不属于可复现的离线门禁。

[MIT license](LICENSE)。

符号 memory/storage 使用不可变来源、长度、写序和 captured copy/view 根，并保留地址与路径关系。大 logical COPY 保持一个 DAG 更新；`max_memory_bytes` 限制具体提取，不限制逻辑长度。原生与 Web 的符号状态使用 flat typed graphs，当前格式版本见[版本与成本说明](docs/19-root-terminal-coverage.md)；报告导出保留节点、深度、exact payload 和取消检查，整份 JSON 输出不设置字节上限。应用的有限共享后处理账本先支付最小模型，再尝试可选丰富投影与 SSA；导出池的 work 统计不替代该账本。丰富导出拒绝或取消时仍保留已发布模型，见[渐进结果](docs/progressive-results.md)。模型、guarded join、账户/plane 和 hash 的支持范围见[符号 EVM 状态](docs/17-symbolic-state.md)。
