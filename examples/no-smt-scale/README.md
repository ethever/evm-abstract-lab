# 固定离线规模检查

本目录固定输入、命令策略、外部资源 guard 和报告验收边界，用于比较已有 SMT 基线与当前 no-SMT 分层抽象解释实现。[命令生成器](../../scripts/generate-no-smt-scale-commands.py)生成当前 CLI profile；[benchmark](../../scripts/benchmark-no-smt-scale.py)默认只核对输入，显式传入 `--execute` 才顺序启动已有 ELF。两者都不构建、不访问 RPC；benchmark 不翻译策略、不调用 shell、不计算速度倍数。

基线源码为 `e3251ffc9009dd751b4d0d998e31e1830908828a`，版本为 **native8 / wire13 / cost2**；当前接口为 **native12 / wire17 / domain3 / cost7**。native 与 wire 分别是原生 JSON 和 Web 协议版本，cost 是逻辑工作计费版本。Cost3 移除了 SMT 额度预收费；Cost4 调整共享回放证据与规范数值上界的累计收费；Cost5 标记类型化标量依赖和操作数克隆；Cost6 更新共享 kernel、规则与独立数值查询的工作模型；Cost7 使用借用式数值操作数与显式 opcode 报价。跨 cost 版本的原始 work 不是同一单位，**不能把不同 cost 版本的 work 相除来表示提速**。性能结论须绑定实际源码、ELF、完整策略和经校验的报告。

## 固定输入与事实边界

[`manifest.json`](manifest.json)固定 world 原始字节 SHA256、每份 runtime bytecode 的 SHA256、账户地址、fork、entry、完整环境和初始 facts。所有 case 使用 Osaka、Runtime 入口。`null` balance/nonce 表示未观察，`storage_unknown: true` 表示未列出的槽未知；缺少账户也未知，不能读成零、空代码或不存在。新帧的空栈、零长度且零填充的 memory、空 returndata，以及新交易的 transient 初始化属于执行规则，与初始链上事实分开记录。

所有 entry 的 caller、value、calldata 内容和长度保持 Unknown；origin 使用 caller 别名规则，entry 非 static，gas 上界未知。五个 `code-only` case 的入口地址为 `0x0000000000000000000000000000000000000101`；WETH 入口为 `0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2`，USDC 为 `0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48`。code-only 的区块字段未知，fixedstate 使用捕获的 header 和父区块 BLOCKHASH。

| case | 固定输入 | manifest 中的 wall / RSS guard |
| --- | --- | --- |
| `unknown-input` | 12 字节 runtime；未知 calldata 首字写入 slot0 后读回返回 | 30s / 2 GiB |
| `dynamic-jump` | 21 字节 runtime；未知目标，JUMPDEST PC3/12 返回 42/99，其他目标失败 | 30s / 2 GiB |
| `loop` | 倒计数循环；每步增加初始未知的 slot0，最终读回 | 30s / 2 GiB |
| `unknown-callback` | slot0 写入、LOG0、未知目标 CALL、读回；允许重复、间接、嵌套 callback | 30s / 2 GiB |
| `memory-copy` | 未知长度 CALLDATACOPY + 可重叠 MCOPY，返回全部 calldata | 30s / 2 GiB |
| `weth-fixedstate` | 捕获代码 3124 字节，1 个账户，4 个已观察槽 | 60s / 8 GiB |
| `usdc-fixedstate` | 2186 字节 proxy + 23464 字节 implementation，2 个账户，2 个已观察槽 | 120s / 8 GiB |

这些是 guard 配置，不是测量结果。另有 host MemAvailable 至少 20 GiB、每 0.25s 采样、SIGTERM 后 5s 仍未退出则 SIGKILL，以及独立的 64 MiB Python Report 解析 guard。RSS 来自单个 ELF 进程的采样和 `wait4` 峰值，不是 cgroup 硬上限；运行契约要求 ELF 不派生工作进程。显式 CPU affinity 至多选择 8 个允许的 CPU，并留出至少 5 个可用 CPU，证据记录实际集合。

`loop` 的固定指令顺序为 `PUSH0 CALLDATALOAD; loop: JUMPDEST DUP1 ISZERO PUSH1 exit JUMPI; PUSH0 DUP1 SLOAD PUSH1 1 ADD SWAP1 SSTORE; PUSH1 1 SWAP1 SUB PUSH1 loop JUMP; exit: JUMPDEST POP PUSH0 SLOAD PUSH0 MSTORE PUSH1 32 PUSH0 RETURN`。loop/exit 地址须按 fixture 的字节偏移核对，不能把汇编说明当作执行证据。manifest 的 `planned_witnesses` 是独立 concrete completion，不注入抽象初始 world，也不替换 Unknown 输入。

WETH/USDC 使用 [`../known-recursion/captured/`](../known-recursion/captured/)已提交的初始 world 和原请求，不从最终 Store 重建输入。snapshot 固定为 Ethereum chain 1、block 26180958、hash `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`。manifest 校验两份 world 和两份原请求的完整原始字节 SHA256；其中历史 source report 路径/hash 仅作捕获溯源，不能代替当前运行报告。固定文件明确枚举 balance、nonce、existence、code 和读到的槽，其他事实保持未知。

`cases` 的 `fixed-offline-snapshot` 与 `acquisition_profiles` 的 `original-rpc-acquisition` 是两个边界。离线 case 不含后续获取或 refinement，不能与历史 RPC acquisition 耗时直接比较。新增离线观察、最终 world facts 或不同 transaction 输入，必须形成新的输入身份。

## 生成与核对命令

在仓库根目录核对固定输入及历史基线 profile；不传 `--execute` 时只解析 JSON 和 bytecode，不启动分析进程：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark-no-smt-scale.py \
  --manifest examples/no-smt-scale/manifest.json \
  --commands examples/no-smt-scale/commands-e325-native.json
```

[`commands-e325-native.json`](commands-e325-native.json)保留 e325 的真实 SMT 参数，不能直接交给当前 no-SMT CLI。当前 CLI 保留 `analyze --world`、域/状态/表示限制及有界本地关系传播，删除 `--smt.provider`、`--smt.rlimit`；`--no-relations` 会改变策略，不能用它冒充 SMT 移除。

对选定源码 checkout 生成独立 profile，再做输入检查。`CHECKED_SOURCE` 指向实际构建使用的源码，`COMMAND_PROFILE` 放在该 checkout 之外：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/generate-no-smt-scale-commands.py \
  --interface-source "$CHECKED_SOURCE" \
  --output "$COMMAND_PROFILE"

PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark-no-smt-scale.py \
  --manifest examples/no-smt-scale/manifest.json \
  --commands "$COMMAND_PROFILE"
```

生成器检查 native12/wire17/domain3/cost7 和 CLI 关系参数，记录 interface source commit 及 CLI、数值选择、关系配置和版本文件的 SHA256。它将历史数值选择迁移为 `--numeric-model product` / `numeric_model`，从预期执行 config 中移出数值限制，并分别记录 `expected_native_numeric_model` 与完整 `expected_native_domain_spec`。应用独立的 20 亿后处理工作上限另存为 `expected_native_postprocessing_max_work`，不改动原 1T 执行额度。它删除基线 policy、policy mapping、预期 config 和 argv 中的两项 SMT 设置，并在预期 config 中记录 `root_terminal_coverage: false`，向新生成的 argv 显式追加 `--no-root-terminal-coverage`，保留原 1T/OFF 对照策略；这不是当前 CLI/Web 的默认值。每个 case 的输入 hash、world、header、未知输入及其余限制保持不变。当前生成的 profile 接受最终 Report 的退出码为 `[0, 2]`：CLI 在输出 Incomplete Report 后返回 2；历史基线 profile 保持 `[0]`。

每份 profile 提供完整 `argv`；仅有首参数 `BINARY` 和一个 `WORLD` 两个占位符，同时绑定 input identity、interface commit、完整 policy、policy 到参数的映射、执行预算 `expected_native_config`、实际代数限制 `expected_native_domain_spec`、模型 `expected_native_numeric_model`、后处理上限 `expected_native_postprocessing_max_work` 和允许的 Report 退出码。`input_mapping` 仍须对照入口与事实语义审核；harness 不证明任意 argv 实现了声明边界。生成器也不证明 ELF 来源或源码干净，最终构建提交确定后须重新生成匹配 profile。

## 使用已有 ELF 运行

生成 profile 的 `expected_native_config.root_terminal_coverage` 显式为 false，配合显式 OFF 参数保留固定七项的原 1T 对照策略；已保存的历史 profile、预算、guard 和 captured input 不变。研究覆盖开启或新默认时另建 profile；从当前固定 profile 派生 ON 对照须先移除互斥的 OFF 参数，再显式加入 `--root-terminal-coverage`，独立记录完整请求与构建身份，不能覆写历史基线。独立证书的语义及 `Incomplete → exit 2` 见[根终止覆盖](../../docs/19-root-terminal-coverage.md)。

构建 metadata JSON 的 `schema` 为 `evm-no-smt-scale-build-v1`，另包含实际 `source_commit`、`source_tree`、`source_dirty`、`artifact_sha256` 和 `build_command` argv 数组。`--binary` 必须是已有可执行 ELF，不能是构建命令或 wrapper。ELF hash 与构建来源声明分别记录；文件 hash 本身不证明编译来源。

填写实际路径、CPU 和主机/cgroup 说明，显式选择 case：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark-no-smt-scale.py \
  --manifest examples/no-smt-scale/manifest.json \
  --commands "$COMMAND_PROFILE" \
  --case unknown-input --execute \
  --binary "$BUILT_ELF" --build-metadata "$BUILD_METADATA" \
  --root "$CHECKED_SOURCE" --output "$NEW_EVIDENCE_DIRECTORY" \
  --cpus "$ASSIGNED_CPUS" --environment-note "$FROZEN_HOST_AND_CGROUP_DESCRIPTION"
```

`--root` 的 commit/tree/dirty 必须匹配 build metadata，profile 的 interface commit 必须匹配 build 的 source commit，ELF SHA256 必须匹配 `artifact_sha256`。dirty 构建还须提供完整匹配的 [`identity.source(root)`](../../scripts/benchmark_known_recursion/identity.py)为 `source_identity`，并显式加 `--allow-dirty`；这类证据始终为 preliminary，不能作为冻结构建比较。

输出目录必须尚不存在且位于 `--root` 之外。runner 复制并核 hash 后使用 world，运行前后核对源码、未跟踪文件内容、脚本、输入、profile、ELF 和 build metadata。多次测量用 `--runs`，多个 case 重复传 `--case`；不同 artifact/profile 使用独立目录。复现基线时换用 e325 checkout、对应 ELF/build metadata 和历史 profile。工具不负责 A/B 调度或统计推断。

## 当前语义实现范围

**符号预编译已有保守包络。** [`precompile.rs`](../../crates/evm-abstract/src/analysis/transfer/precompile.rs)按固定 fork 的 provider 表选择实现。输入可物化且可选计算获准时执行 native；输入未知、超过具体提取上限或可选 crypto 计算过大时可走摘要，不再仅因符号输入而停在 `PrecompileInput`。成功包络中 identity 保留完整输入 source view、长度和 facts；EcRec/P256Verify 保留 0 或 32 字节；其他已列固定输出使用 32/64/128/256 字节未知内容；ModExp 等可变输出不套用 `max_memory_bytes` 或虚构小上界。

包络同时保留任意长度/内容 returndata 的 Revert 和空 returndata 的 Failure；预编译本身是纯操作，touch、root/child rollback 由公共调用结束路径处理。父帧保存完整 returndata，CALL 输出只复制 `min(requested, actual)`，未覆盖后缀保留。摘要按保留的表示计费，不按符号逻辑字节数分配或收费；实际 Work、表示保留、内存或 backend 边界仍须报告。对应回归定义见 [`precompile/tests.rs`](../../crates/evm-abstract/src/analysis/transfer/precompile/tests.rs)，调用完成见 [`calls.rs`](../../crates/evm-abstract/src/analysis/transfer/calls.rs)。

**逻辑内存已有 lazy 范围表示。** [`EvmByteArray<N>`](../../crates/evm-abstract/src/world/bytes.rs)用 U256 长度、捕获的 source view 和有序 Write/Copy/Slice/ITE 根表达读写、别名与重叠复制，不逐字节展开大范围；展示 facts 的上限不截断完整根。CALLDATACOPY、MCOPY、RETURN/REVERT、调用输入/输出等使用这条路径。`max_memory_bytes` 限制具体内容提取，不是逻辑足迹、EVM 协议最大内存或 host RSS；需要具体 initcode 等内容的操作仍有各自边界。

[`ranges.rs`](../../crates/evm-abstract/src/analysis/transfer/ranges.rs)先保留 size=0 不扩容分支，非空正常范围要求 `offset <= B`、`size <= B - offset`，其中 `B = U256::MAX & !31`，避免终点和 word-rounded MSIZE 回绕。`LogicalMemory` 域内保留正常执行与可能的 gas Failure，不精算可负担性；域外非空访问的 Failure 依据见 [`ranges/logical.rs`](../../crates/evm-abstract/src/analysis/transfer/ranges/logical.rs)，范围限于固定 fork 的默认 gas 规则。普通访问不再因超过 `max_memory_bytes` 产生 `MemoryPolicy`；RETURNDATACOPY 的源越界仍是指令 Failure。真实 Work、Relations、ByteRange、StateJoin、Memory 或 guard 保留失败仍可留下 frontier；有 range witness 时保留原操作数、条件及效果前载荷。详见[内存模型](../../docs/14-memory-model.md)与[逻辑内存回归定义](../../crates/evm-abstract/tests/logical_memory.rs)。

分层库接口已提供 `CodeCatalog`、固定 snapshot 的 `StateFacts` 和 `AnalysisSession`：补事实生成不可变版本并从入口重建执行，复用目前限于受保护的纯栈块 memo。CLI/world/RPC 共用同一引擎；本 harness 测量一次离线 CLI 调用，不度量跨轮复用收益。用法和边界见[分层分析](../../docs/18-layered-analysis.md)。

## HTTP / Web 与请求派生

[`wire14/weth-original-request.json`](wire14/weth-original-request.json)和 [`wire14/usdc-original-request.json`](wire14/usdc-original-request.json)是原捕获请求的显式 schema 派生。仅移除 `/limits/smt_provider`、`/limits/smt_rlimit`，保留 RPC 的 `input.Rpc.provider_id`、固定 block、accounts、caller/value 及任意长度/内容的 Unknown calldata。manifest 记录原始/派生 raw SHA256 和删除 pointer；默认输入检查验证除这两项外请求完全相同。它们仍是 RPC acquisition 请求，不是 offline CLI 输入。

当前 server/Web 使用 wire17。上述 wire14 文件保留原派生字节与 hash；为保留历史 1T/OFF 策略而发送当前请求时，需将历史 `limits.domain_profile` 显式迁移为 `limits.numeric_model: "Product"`，并添加 `limits.root_terminal_coverage: false`；使用新默认则另存相应完整请求，不能改写这些捕获。新生成的请求文件名为 `weth-wire17-request.json` / `usdc-wire17-request.json`。活跃请求中的旧选择器或 SMT 字段都会被明确拒绝。HTTP 任务、报告投影、传输和浏览器解析有各自状态；任务 `Completed` 不等于报告 `Converged`，须读取实际 status/frontiers、效果和 export 证据。HTTP 测量需要匹配 server 构建、完整请求/策略及 wire adapter；本 harness 的 `native-world-v1` 只消费 CLI stdout，不能用 server ELF 代替 CLI 或混比 RPC 与离线耗时。协议、任务接口和时间边界见 [Web 工作台](../../docs/web-workbench.md)。

## 报告交付、计时与验收

每个 sample 保存实际 argv/guard 及 hash、输入身份、进程启止记录、原始 stdout/stderr/resources 及 SHA256、实际 `wait4` exit（信号为负数）、CPU/wall/peak RSS。计时包括 ELF 执行和 native 序列化，排除构建、输入检查、Python Report 解析及 RPC acquisition；外部停止包含实际回收时间。`phase` 只是 harness 观察，`native_phase` 始终为 null，不能据此推断引擎内部热点。

| 观察到的结果 | harness 记录与解释 |
| --- | --- |
| 正常/错误退出，退出码获 profile 接受，完整 stdout 通过最终 JSON adapter | `report_delivered: true`，填入 `nativefinal`；另核对执行 config、实际代数限制、模型和后处理上限是否匹配 policy。Incomplete 仍可交付，交付不代表闭合。 |
| 进程退出获接受，但 stdout 超过 64 MiB Python 解析 guard | `phase: output_unvalidated_parse_guard`、`report_delivered: false`、`nativefinal: null`；保留原始 stdout/hash，分析 status 尚未由 harness 验证。 |
| timeout、RSS/host guard、interrupt、monitor error、无效退出或无效 JSON | 保留失败和资源证据，`nativefinal: null`；即使外部停止时 stdout 恰有完整 JSON，也不视为交付。 |

64 MiB guard 属于这个 Python consumer，与 `max_memory_bytes` 无关，也不是 native JSON 总大小限制。[原生 JSON 编码器](../../crates/evm-abstract/src/render/json.rs)不设置整份输出字节上限；表示导出仍有节点、深度、exact payload 与取消边界。超过解析 guard 不能推出分析 Incomplete 或 Converged；须校验保存的完整原报告及有效 config，才可给出分析状态或性能结论。未适配 schema、stdout 有字节或 progress 都不能替代最终 Report。

交付后的统计包括 status、states/transfers/work、summary/procedure/reuse/propagation、summary stats、原始 frontiers/reasons、diagnostics、导出图大小、outcome kind、完整 outcome hash、entry/world hash、执行/数值/后处理策略 hash、后处理实耗、policy 是否匹配及 closure flags。无 Report 时 native 统计为 null，资源和实际退出证据仍保留。summary flags 为空时 `closure.all_closed` 为 null；全 true 也不能单独证明 root 的完整输出/效果覆盖。图计数是重复序列化观察，不是唯一 live DAG 节点数；opaque identity 可跨进程变化，raw/outcome hash 不证明语义等价。

**wall/RSS 必须伴随经校验的原报告、Converged/Incomplete、全部 frontier、summary closure 和完整 effects 一起审阅。** 早停、少交付效果或更早 Incomplete 不能称为提速；无已验证 Report 不给速度倍数。wall/RSS 比较须使用相同输入与交付边界，说明完整策略和精度差异；跨 cost 版本的 work 分列，不能按同单位换算。harness exit 0 只表示计划 sample 均交付了完整策略匹配的有效 Report 且身份未变，允许 Report 为 Incomplete，不是语义验收结论。

语义验收先于 scalability：

1. 区分 unknown、known-zero、empty、nonexistent；保留 CALL-family、storage owner、persistent/transient、历史读取与 rollback 身份。
2. 每个 concrete witness 的终止种类、全部 returndata、storage/transient、账户生命周期/余额及有序日志，须由**同一个 abstract outcome 联合覆盖**，不能分别挑选互不相容的投影。
3. 未知调用须覆盖重复、间接、嵌套重入，不能限定一次 callback、把失败强制成功或遗漏回滚。保守局部效果及 protected-owner callback 路径见 [`procedure/environment.rs`](../../crates/evm-abstract/src/analysis/transfer/procedure/environment.rs)；普通输出、失败、完整效果、原始 frontier 与 closure 都要检查。
4. 允许更粗精度，但须记录扩大了哪些结果/效果集合；不能通过删除效果或忽略未完成工作降低计数。有限 witness 和局部回归不构成整个 EVM 实现的 soundness 证明。[Lean 证明文档](../../docs/proofs/README.md)限定已检查的局部模型及其[生产对应契约](../../docs/proofs/correspondence.md)，Rust 执行器、worklist 和更丰富的数值域仍有独立对应义务。

## Screen16 实测结果（2026-10-18）

最终默认选择为快速保守分析：CLI/Web 的 `max_work=2B`，库的 `max_work=20M`，两者均启用根终结覆盖；需要单独验收详细分析时使用显式 OFF。下面保留的是 Screen16 冻结源码和原固定 profile 的结果，不是最终默认配置的同输入验收。表中的 2B 仅改变原 profile 的工作预算，其他分析限制仍是捕获值，不能当作新 CLI/Web 默认的实测数据。最终接口的固定策略 OFF 复测须显式传 `--no-root-terminal-coverage`，不改原始捕获或历史 argv。

冻结源码为干净提交 `e5f13b82c16798431ad0bf7cbb7cfdb7ef3dc2f3`（native10 / wire15 / cost3），CLI ELF SHA256 为 `e52e1086610c290347cf44f66e06b012baeecdca51dde088399a20730c774f7a`；输入 [`manifest.json`](manifest.json) 的原始字节 SHA256 为 `fba0c9099319e6dbe8d646731912e22ca9777885d496679a737f61f8e3ae51f0`。复现本节时须使用该冻结提交自身的命令生成器、离线 runner 及其独立 OFF/ON profile；当前[命令生成器](../../scripts/generate-no-smt-scale-commands.py)服务于当前接口，不能把它生成的显式 OFF 参数投给不支持该参数的旧 ELF。沿用原 captured world、请求和事实边界：WETH/USDC 分别仅有 1/2 个已观察账户，caller/value/calldata 保持 Unknown，无额外 RPC 或事实注入。

每行均为同一 ELF 在 CPU 40 上顺序执行的一次离线 screen，无并行的所属 native、构建或浏览器任务。wall 从 ELF exec 到进程回收，包含 native JSON 序列化，排除 acquisition、网络、排队、构建及之后的 Python 解析、流式校验、传输和浏览器处理。原 wall/RSS、host free、轮询和报告 guard 均不变，所有运行均无外部停止。

**OFF 七项。** OFF 是 Screen16 源码的旧默认，固定 profile 仍显式设置 `max_work=1T` 等原限制，并非全部 CLI 参数取默认值。

| case | native exit | wall（s） | peak RSS（KiB） | 原 64 MiB whole-report harness 验收 |
| --- | --- | --- | --- | --- |
| `unknown-input` | 0 | 0.0107106 | 22508 | 已交付；Converged，0 frontier |
| `dynamic-jump` | 0 | 0.0116429 | 22508 | 已交付；Converged，0 frontier |
| `loop` | 0 | 0.0243218 | 22508 | 已交付；Converged，0 frontier |
| `unknown-callback` | 0 | 0.1070145 | 38088 | 已交付；Converged，0 frontier |
| `memory-copy` | 0 | 0.0064634 | 47348 | 已交付；Converged，0 frontier |
| `weth-fixedstate` | 0 | 3.469958 | 357956 | parse guard 拒绝；`report_delivered=false` |
| `usdc-fixedstate` | 2 | 99.600695 | 1278392 | parse guard 拒绝；`report_delivered=false` |

原七项 harness 汇总仍退出 1，上述两项 parse guard 失败及 `report_delivered=false` 保持不变。两份大型 OFF 原报告的独立严格流式补充校验已完成，校验器实际均退出 0；在不变的每值 64 MiB 上限内，完整检查 JSON 语法、EOF、重复键及每条顶层数组记录。WETH 为 Converged，137 states / 482 transfers / 0 frontier，1 个 summary 的 `closed=true`；USDC 为 Incomplete，231 states / 13714 transfers / 825 frontier（803 Work + 22 SummaryWork，无 Expr frontier），1 个 summary 的 `closed=false`。原报告 SHA256：WETH `7aaeb22388ec8edb1f4bfaaf6d79f1616d0ad58b2990f08a2fef791da54fcec3`；USDC `931df3accaa2e5891f566f9c2344e71a6424ad4649172024abf8254c1c6e3fec`。补充仅针对上述冻结源码的历史 1T/OFF 报告，wall/RSS 沿用原 native 测量，未重新测量；JSON 校验不构成图语义或完整 Rust EVM 证明，当前 2B/ON 默认的同输入验收见[最终集成实测](#最终集成实测)。CLI 的 exit 2 是 Incomplete 约定，不是崩溃；退出码本身不能替代完整 JSON 校验。

**ON 五项。** 固定策略比较仅向原 argv 追加 `--root-terminal-coverage`，对应有效 config 为 true；USDC 预算曲线另仅将 `max_work` 改为 20M / 200M / 2B，其余输入、限制（含 `max_transfers=10M`）和 guards 不变。下表详细结果来自独立的严格流式补充：五份 native10 JSON 均完整校验至 EOF。补充 runner 实际退出 0 表示交付/结构检查完成；原 whole-report 标志保持原值，V2 scanner 的每记录 64 MiB 上限与整份报告 guard 分别记录。

| case | max_work | wall（s） | peak RSS（KiB） | native exit / 详细 status | states / transfers / frontier | 根覆盖附件 | 原 whole-report 交付 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `usdc-fixedstate` | 1T | 100.294006 | 1275448 | 2 / Incomplete | 231 / 13714 / 833 | Some（Work） | false |
| `weth-fixedstate` | 1T | 3.511306 | 357536 | 0 / Converged | 137 / 482 / 0 | None | false |
| `usdc-fixedstate` | 20M | 0.023174 | 55016 | 2 / Incomplete | 1 / 1 / 2 | Some（Work） | true |
| `usdc-fixedstate` | 200M | 0.050982 | 55016 | 2 / Incomplete | 13 / 10 / 4 | Some（Work） | true |
| `usdc-fixedstate` | 2B | 0.789721 | 210100 | 2 / Incomplete | 138 / 126 / 14 | Some（Work） | false |

USDC 1T 的 833 个 frontier 为 **811 Work + 22 SummaryWork**，1 个 procedure summary 的 `closed=false`；三个小预算的 frontier 均为 Work。WETH 独立的详细报告为 Converged，1 个 procedure summary 的 `closed=true`，未附根覆盖记录。

四份 USDC 附件均为 `AllInputStates`、Work 触发，固定包含 Return / Revert / Failure 三个联合上界。完整附件只在 `/input/config/max_work` 和一致、单射的图 `identity_scope` UUID 重命名后相同（各 31 份图记录、各 1 个 SerializationScope）；没有删除字段、表达式、fact、source ID、数组或字节。这支持所发出上界独立于已访问状态子集；上界有意较宽，可允许不可达 tuple，与详细执行观察到的 outcomes 分开，不增加状态、PC、SSA 或改变 status。语义依据仍是[实际构造器](../../crates/evm-abstract/src/analysis/engine/terminal.rs)、回归定义及[局部证明模型与手工对应](../../docs/proofs/correspondence.md)，JSON 校验及附件一致性不证明 Rust Γ 包含关系或完整 EVM 语义。

不同预算的耗时体现精度与预算的取舍，不能解释为执行器加速或同精度收益；探索更多状态时 frontier 增多也不能单独说明精度下降。20M 的约 0.023s 仅是上述完整进程边界，USDC 仍为 Incomplete，不表示整个 CFG 闭合或 RPC/Web 响应时间。OFF/ON 的单次 WETH 差异同样不支持统计加速结论。

Runtime2 的数值精度损失与尚未恢复的 Multicall self-call 细节仍单列为已知问题；Router 的非空 typed pending 覆盖及独立 UI 图压力回归也由各自用例验收。

## 最终集成实测

2026-10-18，producer 为干净提交 `9a1a0903345ab695a0471846c0625a5b56c0da93`，tree 为 `d69fc764e7cf58d6d701e8336bbe5e9b5b82a534`（native10 / wire15 / cost3），运行前后均干净；同一冻结 CLI ELF 的 SHA256 为 `6209803ece98e1a92109ba633ffdd681efb21c64cc4900b400bdbb332778b77a`。汇总 `acceptance-summary.json` 的 SHA256 为 `c0b02cce31ae0d4b8ea3901c3dc5745a60c531179eb4dac0cb0ee0c45e5863e8`，绑定完整运行记录及原报告 hash。

各进程绑定 CPU 40 并顺序执行；期间未并行本任务的构建、浏览器或分析进程，共 **9 个独立 native 进程**：五个 code-only 仅复测固定 1T/OFF，未测其默认策略；WETH/USDC 各测当前默认 2B/ON 与固定 1T/OFF。原始 world 字节、caller/value/calldata 的 Unknown 边界及初始 facts 不变。wall 包含 native JSON 输出，排除后续解析、RPC、Web 与构建；补充校验不新增 native 测量，也不重复计入 wall。

| case | policy | native wall（s） | peak RSS（KiB） | status / native exit | states | transfers | frontiers | 根覆盖附件 | 报告字节数 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `unknown-input` | fixed-1T-OFF | 0.010867 | 21,500 | Converged / 0 | 1 | 1 | 0 | None | 213,075 |
| `dynamic-jump` | fixed-1T-OFF | 0.007564 | 21,500 | Converged / 0 | 3 | 3 | 0 | None | 370,227 |
| `loop` | fixed-1T-OFF | 0.020849 | 21,500 | Converged / 0 | 7 | 13 | 0 | None | 945,706 |
| `unknown-callback` | fixed-1T-OFF | 0.105730 | 37,376 | Converged / 0 | 27 | 79 | 0 | None | 9,314,605 |
| `memory-copy` | fixed-1T-OFF | 0.006041 | 46,000 | Converged / 0 | 1 | 1 | 0 | None | 86,638 |
| `usdc-fixedstate` | default-2B-ON | 0.793679 | 211,600 | Incomplete / 2 | 138 | 126 | 14 | Some（AllInputStates / Work） | 167,848,568 |
| `usdc-fixedstate` | fixed-1T-OFF | 99.677762 | 1,278,308 | Incomplete / 2 | 231 | 13,714 | 825 | None | 550,217,349 |
| `weth-fixedstate` | default-2B-ON | 0.385788 | 94,312 | Incomplete / 2 | 56 | 47 | 10 | Some（AllInputStates / Work） | 70,620,745 |
| `weth-fixedstate` | fixed-1T-OFF | 3.491552 | 357,288 | Converged / 0 | 137 | 482 | 0 | None | 256,024,873 |

默认两项的 frontier 全为 Work（USDC 14、WETH 10）；详细分析仍为 Incomplete，附件固定为 Return / Revert / Failure 三个联合上界，不隐藏 frontier，不增加已执行 PC、状态或 SSA 覆盖。固定 1T/OFF 的 USDC 保留 825 个 frontier（803 Work + 22 SummaryWork）且无证书；WETH 为 Converged、0 frontier、无证书。

五份 code-only 报告通过原 64 MiB whole-report guard。四份 WETH/USDC 原报告均超过该 guard，原 whole-report guard 与 `report_delivered` 仍为 **false**；四份均为 `strict_stream_valid=true`；执行这四项的补充 runner 实际退出 0，保持每条顶层记录 64 MiB 上限。补充 runner 的退出 0 不代表原 consumer 通过，也不证明 Rust/EVM 数学语义。导出仍大，原交付边界未改变。

这些是单次离线 screen；默认 2B/ON 与固定 1T/OFF 是不同精度策略，不能据此声称同精度加速或详细执行闭合。
