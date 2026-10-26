# 固定离线规模检查

本目录固定输入、命令策略、外部资源 guard 和报告验收边界，用于比较已有 SMT 基线与当前 no-SMT 分层抽象解释实现。[命令生成器](../../scripts/generate-no-smt-scale-commands.py)生成当前 CLI profile；[benchmark](../../scripts/benchmark-no-smt-scale.py)默认只核对输入，显式传入 `--execute` 才顺序启动已有 ELF。两者都不构建、不访问 RPC；benchmark 不翻译策略、不调用 shell、不计算速度倍数。

基线源码为 `e3251ffc9009dd751b4d0d998e31e1830908828a`，版本为 **native8 / wire13 / cost2**。选定 checkout 的当前接口以[原生 schema 常量](../../crates/evm-abstract/src/analysis.rs)、[Web schema 常量](../../crates/evm-abstract-protocol/src/lib.rs)和[数值配置及计费版本](../../crates/evm-abstract/src/domain/spec.rs)为准，并由匹配的命令生成器核对。native 与 wire 分别是原生 JSON 和 Web 协议版本，cost 是逻辑工作计费版本。跨 cost 版本的原始 work 不是同一单位，**不能把不同 cost 版本的 work 相除来表示提速**。性能结论须绑定实际源码、ELF、完整策略和经校验的报告。

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

使用选定源码 checkout 自身的生成器生成独立 profile，再做输入检查。`CHECKED_SOURCE` 指向实际构建使用的源码，`COMMAND_PROFILE` 放在该 checkout 之外：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 "$CHECKED_SOURCE/scripts/generate-no-smt-scale-commands.py" \
  --interface-source "$CHECKED_SOURCE" \
  --output "$COMMAND_PROFILE"

PYTHONDONTWRITEBYTECODE=1 python3 "$CHECKED_SOURCE/scripts/benchmark-no-smt-scale.py" \
  --manifest "$CHECKED_SOURCE/examples/no-smt-scale/manifest.json" \
  --commands "$COMMAND_PROFILE"
```

生成器检查选定源码的 native/wire/domain/cost 版本和 CLI 关系参数，记录 `interface.versions`、interface source commit 及 CLI、数值选择、关系配置和版本文件的 SHA256；版本不匹配时拒绝生成。它将历史数值选择迁移为 `--numeric-model product` / `numeric_model`，从预期执行 config 中移出数值限制，并分别记录 `expected_native_numeric_model` 与完整 `expected_native_domain_spec`。应用独立的 20 亿后处理工作上限另存为 `expected_native_postprocessing_max_work`，不改动原 1T 执行额度。它删除基线 policy、policy mapping、预期 config 和 argv 中的两项 SMT 设置，并在预期 config 中记录 `root_terminal_coverage: false`，向新生成的 argv 显式追加 `--no-root-terminal-coverage`，保留原 1T/OFF 对照策略；这不是当前 CLI/Web 的默认值。每个 case 的输入 hash、world、header、未知输入及其余限制保持不变。当前生成的 profile 接受最终 Report 的退出码为 `[0, 2]`：CLI 在输出 Incomplete Report 后返回 2；历史基线 profile 保持 `[0]`。

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

server/Web 的版本以所选源码的 [Web schema 常量](../../crates/evm-abstract-protocol/src/lib.rs)为准。上述 wire14 文件保留原派生字节与 hash；需要 HTTP 请求时，在生成 profile 的命令加 `--request-output-dir "$NEW_REQUEST_DIRECTORY"`，将派生请求放在仓库外，文件名由匹配版本的生成器决定。为保留历史 1T/OFF 策略，生成器将 `limits.domain_profile` 显式迁移为 `limits.numeric_model: "Product"`，并添加 `limits.root_terminal_coverage: false`；使用新默认则另存相应完整请求，不能改写这些捕获。活跃请求中的旧选择器或 SMT 字段都会被明确拒绝。HTTP 任务、报告投影、传输和浏览器解析有各自状态；任务 `Completed` 不等于报告 `Converged`，须读取实际 status/frontiers、效果和 export 证据。HTTP 测量需要匹配 server 构建、完整请求/策略及 wire adapter；本 harness 的 `native-world-v1` 只消费 CLI stdout，不能用 server ELF 代替 CLI 或混比 RPC 与离线耗时。协议、任务接口和时间边界见 [Web 工作台](../../docs/web-workbench.md)。

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

## 历史结论与复查

2026-10-18 的离线检查分别绑定 Screen16 源码 `e5f13b82c16798431ad0bf7cbb7cfdb7ef3dc2f3` 与最终集成源码 `9a1a0903345ab695a0471846c0625a5b56c0da93`（native10 / wire15 / cost3）。这些结论只属于相应版本：固定 1T/OFF 的 WETH 详细分析为 Converged，USDC 仍为 Incomplete；最终集成时旧默认 2B/ON 的两项详细分析均为 Incomplete，另附全输入 Return / Revert / Failure 上界，未增加已执行 PC、状态或 SSA。

历史 WETH/USDC 报告超过原 whole-report 解析 guard，原 `report_delivered=false` 保持有效。后续严格流式校验通过只补充报告结构检查，不能改写原 consumer 的失败，或充当 EVM 语义证明。不同预算和精度策略的结果也不能据此称为同精度加速。

逐次性能、资源与导出记录保存在仓库外；清理前的完整说明可从 Git 历史读取：

```sh
git show 3801d1d22a2341d0961b4a9a621771be2de5a375:examples/no-smt-scale/README.md
```

复查历史实现时使用其冻结提交自身的生成器、runner 和 profile。复查当前实现时按上面的流程重新生成匹配的 profile，保留原 Unknown 输入、全部 world facts、guard、失败与覆盖要求，将新输出放在仓库外。当前默认策略以实际 CLI/Web 配置为准，历史表不作为当前验收结果。
