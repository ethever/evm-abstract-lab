# 固定离线规模检查草稿

本阶段只准备输入、命令和验收方法，没有运行 native、SMT、RPC、构建或规模实验。分析源码基线固定为 `e3251ffc9009dd751b4d0d998e31e1830908828a`；它仍含 SMT，`commands-e325-native.json` 明确保留该基线的实际参数。未来 Layer1/Layer2 的名字和入口由对应实现确定，再添加独立命令 profile；这里不预设新 CLI flag。

`scripts/benchmark-no-smt-scale.py` 复用已有 `benchmark_known_recursion` 的进程组 guard、资源采样和 Report 统计。默认只做纯 Python 输入检查；只有显式传入 `--execute` 才启动 ELF。它不构建、不访问 RPC、不调用 shell，也不计算速度倍数。本阶段的执行和编译由协调者取得单一 validation lane 后安排。

## 输入与 profile

`manifest.json` 固定 world 原始字节 SHA256、每份 runtime bytecode 的 SHA256、账户地址、fork、entry、完整环境边界和初始 facts。`null` balance/nonce 表示未观察，`storage_unknown: true` 表示未列出的槽未知；缺少账户也未知。EVM 新帧的空栈、初始内存以及新交易的 transient 初始化属于执行规则，与初始链上事实分开记录。所有 entry 的 caller、value、calldata 内容和长度保持 Unknown；origin 使用基线的 caller 别名规则，entry 非 static，gas 上界未知。

| case | 固定输入 | 后续首轮外部 screen 计划 |
| --- | --- | --- |
| `unknown-input` | 12 字节 runtime；未知 calldata 首字写入 slot0 后读回返回 | 30s / 2 GiB RSS |
| `dynamic-jump` | 21 字节 runtime；未知目标，JUMPDEST PC3/12 返回 42/99，其他目标失败 | 30s / 2 GiB RSS |
| `loop` | 倒计数循环；每步增加初始未知的 slot0，最终读回 | 30s / 2 GiB RSS |
| `unknown-callback` | slot0 写入、LOG0、未知目标 CALL、读回；允许未知执行重复、间接、嵌套 callback | 30s / 2 GiB RSS |
| `memory-copy` | 未知长度 CALLDATACOPY + 可重叠 MCOPY，返回全部 calldata | 30s / 2 GiB RSS |
| `weth-fixedstate` | 已有捕获代码 3124 字节，1 个账户，4 个已观察槽 | 60s / 8 GiB RSS |
| `usdc-fixedstate` | 2186 字节 proxy + 23464 字节 implementation，2 个账户，2 个已观察槽 | 120s / 8 GiB RSS |

以上都是计划值，没有测量结果。guard 还包括 host MemAvailable 至少 20 GiB、每 0.25s 采样、停止后至多 5s 的 SIGTERM 宽限期，以及独立的 64 MiB Python Report 解析上限。后者不改 native 分析策略；超过时保留原始 stdout，记录未验证输出，`nativefinal` 为 null。RSS guard 为已有工具的单个 ELF 进程采样和 `wait4` 峰值，不是 cgroup 硬上限；此命令 contract 要求 ELF 不派生工作进程。CPU 选择复用已有工具的显式 affinity 规则，至多 8 个允许的 CPU，并留出至少 5 个可用 CPU；记录实际集合。

`loop` 的固定指令顺序为 `PUSH0 CALLDATALOAD; loop: JUMPDEST DUP1 ISZERO PUSH1 exit JUMPI; PUSH0 DUP1 SLOAD PUSH1 1 ADD SWAP1 SSTORE; PUSH1 1 SWAP1 SUB PUSH1 loop JUMP; exit: JUMPDEST POP PUSH0 SLOAD PUSH0 MSTORE PUSH1 32 PUSH0 RETURN`。fixture 的 loop/exit 地址必须由字节偏移检查，不能把汇编说明当作执行证据。manifest 中的 concrete completions 只是后续 witness 计划，不注入抽象初始 world，也没有执行。

WETH/USDC 使用 `../known-recursion/captured/` 已提交的文件，不从历史最终 Store 重建输入。两份 world 和两份原请求复用已提交捕获文件，由 manifest 校验其完整原始字节 SHA256。snapshot 固定为 Ethereum chain 1、block 26180958、hash `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`。manifest 保留原捕获说明所列的 source report 路径/hash；本阶段没有重新验证这些历史 report 或重新采集状态。固定文件明确枚举 balance、nonce、existence、code 和读到的槽，其他事实保持未知。

`cases` 中的 `fixed-offline-snapshot` 与 `acquisition_profiles` 中的 `original-rpc-acquisition` 是两个独立边界。后者保留原请求字节 hash、固定区块和原 RPC 策略，仅供溯源，本 harness 不执行它。离线 case 使用已捕获的 header/environment，不含之后的获取或 refinement 轮次；不能将其耗时与历史 RPC acquisition 耗时直接比较。新增离线观察、最终 world facts 或不同 transaction 输入时必须形成新的输入身份。

## 纯输入检查与后续运行

在仓库根目录检查 manifest 和基线命令，只解析 JSON 和 bytecode，不启动分析进程：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/benchmark-no-smt-scale.py \
  --manifest examples/no-smt-scale/manifest.json \
  --commands examples/no-smt-scale/commands-e325-native.json
```

命令 profile 提供每个 case 的完整 `argv`，唯一的两个占位符为第一个参数 `BINARY` 和一个 `WORLD`。它还绑定 input identity、实际 interface source commit、完整 policy、policy 到参数的映射、预期 native `config` 和允许交付最终 Report 的退出码。基线参数来自当前源码和已有捕获 manifest；所有未知输入相关 flag 都省略。harness 不翻译 policy。候选实现应新增 profile，写出真实参数与输入绑定，更新其 source commit 和预期 config，明确哪些策略已移除、变粗或不可比；不能把基线 `--no-relations` 当作完成 SMT 移除。

当前 `native-world-v1` adapter 只支持已有 native world Report JSON。候选若改变 schema，应先审查并适配 adapter；未适配输出保留原文并记未交付已验证 Report，不猜字段、不拿 progress 代替 final。`input_mapping` 是待审查声明，harness 无法证明任意 argv 正确实现该语义边界，协调者需要用入口/事实回归验证。

root 后续先构建并记录真实 ELF，提供 `evm-no-smt-scale-build-v1` JSON：`source_commit`、`source_tree`、`source_dirty`、`artifact_sha256`、实际 `build_command` argv 数组。dirty 构建还须提供完整匹配的 `benchmark_known_recursion.identity.source(root)` 为 `source_identity`，并显式使用 `--allow-dirty`；其证据始终标为 preliminary。ELF hash 与这些构建来源声明分别记录；后者由构建者提供，文件 hash 本身不证明编译来源。

取得 validation lane 后，使用已经存在的 artifact 和 build metadata，显式选择 case：

```sh
python3 scripts/benchmark-no-smt-scale.py \
  --manifest examples/no-smt-scale/manifest.json \
  --commands examples/no-smt-scale/commands-e325-native.json \
  --case unknown-input --execute \
  --binary "$BUILT_ELF" --build-metadata "$BUILD_METADATA" \
  --root "$CHECKED_SOURCE" --output "$NEW_EVIDENCE_DIRECTORY" \
  --cpus "$ASSIGNED_CPUS" --environment-note "$FROZEN_HOST_AND_CGROUP_DESCRIPTION"
```

这些变量由协调者填入真实路径/CPU；不是本阶段已执行的命令。输出目录须为不存在且位于 `--root` 之外的目录。runner 顺序运行，使用复制并核 hash 的 world；运行前后核对源码、未跟踪文件内容、脚本、输入、命令、ELF、build metadata。源码必须匹配 build 声明，command profile 也须匹配该 interface commit。多次测量另用 `--runs`，不同 artifact/profile 分目录；当前脚本不负责 A/B 调度或统计推断。

## 结果与语义验收

每个 sample 保存实际 argv/guard 及 hash、输入身份、process 启止记录、原始 stdout/stderr/resources 和 SHA256、实际 `wait4` exit（信号为负数）、CPU/wall/peak RSS，以及 harness 可观察的 phase。`native_phase` 始终 null，表示引擎内部阶段未观测。进程计时包括 ELF 执行和 native 序列化，排除构建、输入检查、Python Report 解析与 RPC acquisition；guard 停止时间包含实际回收所需的宽限期。

只有正常/错误退出、退出码在 profile 声明集合中、完整 stdout 是符合 adapter 的最终 JSON，才置 `report_delivered: true` 并填入 `nativefinal`。其余情况保留原始失败证据；外部 timeout、RSS/host guard、interrupt、monitor error 的 `nativefinal` 必须为 null，即使 stdout 恰有完整 JSON。stdout 有字节不等于交付 Report，stderr/progress 不参加最终状态统计。有效的 Incomplete Report 仍是交付，其 status、frontiers、reason 和失败输出全部保留；交付不代表闭合。

交付后统计复用已有 `metrics`：status、states/transfers/work、summary/procedure/reuse/propagation、summary stats、frontiers/reasons、diagnostics 和导出图大小，并补充 outcome kind、完整 outcome hash、entry/world/config hash、policy 是否匹配和闭合 flags。无 Report 时这些 native 统计为 null，资源与实际退出证据仍保留。summary flags 为空时 `all_closed` 为 null；flags 为 true 也不证明 root 的完整输出/效果覆盖。导出图计数是重复序列化观察，不是唯一 live DAG 节点数；opaque identity 可能跨进程变化，raw/outcome hash 不是语义等价判据。

harness exit 0 只表示所有计划 sample 交付了 config 匹配的有效 Report 且身份未变化，允许 Report 本身为 Incomplete；它不是语义验收或整个目标完成的判定。unit/fixture 检查的接入与实际 native/语义 focused 验证，须由协调者接管后按完整 checks 约定安排。

预先验收按 soundness、再 scalability 排序：

1. 未观察事实未知。区分 unknown、known-zero、empty、nonexistent；callback、CALL-family、owner、rollback 的回归全部保留。
2. 每个 concrete witness 的终止种类、全部 returndata、storage/transient、账户生命周期/余额及有序日志，须由**同一个** abstract outcome 联合覆盖；有限 witness 不构成整个实现的 soundness 证明。
3. 检查所有普通输出、效果、失败分支、原始 frontier 及 closure，不能通过删除效果、忽略未完成工作或只比 root 成功值宣称完成。
4. 允许更粗精度，但须记录扩大了哪些结果/效果集合。状态数、work、时间减少本身不证明改进；不同工作记账单位、输入/策略或 delivery 边界不能混为同一比较。没有已验证 Report 时不给速度倍数。

`scripts/test_benchmark_no_smt_scale.py` 准备了仅用合成 JSON 的 adapter 回归，覆盖 timeout/progress/无效退出、解析失败、Incomplete、policy mismatch 和 hash/fact 身份拒绝。它没有运行分析器，也不替代语义回归；本 worker 只做输入和语法检查，focused 测试留给协调者。

## 基线源码的三个规模风险

以下是 `e325` 的只读机制审查和待测假设，没有推断实测收益；对应源码接口若被候选重构，需更新观测映射。

| 风险 | 具体机制与源码 | 可测指标与输入 | soundness 约束 |
| --- | --- | --- | --- |
| structural key 与控制流分区增加 | `analysis/machine.rs: FrameKey/MachineKey` 保留 code、owner、caller、jump history；`analysis/engine.rs: ids` 和 `transfer/procedure.rs: Procedure.nodes` 保留首个 CALL continuation 的 success/failure 分区。去掉约束剪枝后，更多动态目标/loop 路径可能抵达这些 key。 | `dynamic-jump`、`loop`、USDC：states、transfers、summary state_count、frontier reason、是否在相同 guard 内交付。精确 key/partition 分布目前未直接导出，不能填 0。 | 不能合并代码/owner/回滚身份或删失败分区来降低计数；合并需证明完整 payload 覆盖。 |
| payload/immutable DAG 与普通工作仍占成本 | `transfer/procedure/payload.rs: form/semantic_eq/measure_work` 比较 frame、continuation、store 和 observation；`procedure/solver.rs: form/publish` 做 projection/join，`engine.rs` 继续对保留 payload/evidence 记账。移除求解器并未移除这些遍历或数据。 | `loop`、`memory-copy`、USDC：work、CPU、peak RSS、raw stdout bytes、`graph_sizes.serialized_nodes/largest_serialized_graph`。live unique DAG、各操作耗时无现成证据，保持未观测。 | graph export 不是活跃图规模；不能丢初始源、saved store 或有序效果换取较小 DAG。记账变化需单列，不能称同单位 work 加速。 |
| unknown callback 与输出传播组合 | `transfer/procedure/environment.rs` 先保留 Return/Revert/Failure，再枚举 protected owner 的普通 callback；`procedure/solver.rs: publish/deliver` 将 output generation 传播给 subscribers，允许下一轮 callback。分支增多可增加关系更新与返回传播。 | `unknown-callback`、WETH/USDC：summary/procedure kinds、relation_states、propagations/reused、summary_closed、outcome kind、frontier/reason、wall/RSS。实际 subscriber fanout/代数未导出，需 root 授权后另加观测。 | 不能限定 callback 一次、遗漏间接/嵌套调用或将失败强制成功；owner effects、日志及 rollback 必须联合覆盖，closure 必须真实达到。 |


## Wire14 请求派生与成本版本

`wire14/{weth,usdc}-original-request.json` 是原捕获请求的显式schema派生，不改历史文件。仅移除 `/limits/smt_provider`、`/limits/smt_rlimit`；RPC 的 `input.Rpc.provider_id` 原样保留。manifest逐一记录两份原始raw SHA、派生raw SHA和删除pointer，默认plan检查会验证全请求除这两项外完全相同。它保持固定block、accounts、caller/value以及任意长度/内容的Unknown calldata。此处没有执行请求，也不把该HTTP profile伪装成offline CLI输入。

基线为native8/wire13/cost2，候选为native9/wire14/cost3。Cost3删除SMT额度预收费，新旧logical work不能直接比率换算成性能。候选实际source/ELF和命令profile仍须在集成release后冻结；本stage没有宣称候选可执行或跑过基准。

## 默认未知输入下的保守续行需求（只读设计，未实施）

1. **符号预编译输入。** 当前 `analysis/transfer/precompile.rs` 先要求输入和工作界可具体化，失败停在 `PrecompileInput`。若默认未知calldata在WETH/USDC路径反复到达这些点，SMT移除后仍可能只得到更早Partial。一个待审slice应同时覆盖原生成功、失败、returndata长度和内容：固定宽度原生有按fork明确的结果长度；identity应保留输入view/长度关系；modexp等可变长度不能猜统一小上界。现有 `ByteArray::unknown_checked` 与summary length可表达宽上界，`copy_return_data`/CALL续行仍须遵守原有失败和未覆盖前沿；它们目前没有自动建立各预编译的语义包络。须按实际热点和验收决定实现，不能把未知输入当空串。
2. **逻辑内存与物化分离。** 现有ByteState的Copy/Slice/Ite及持久root可保留大符号范围，`COPY_FACT_LIMIT`只限制展示facts；但 transfer/ranges.rs 会把memory policy之外的输入留成frontier。若要不经早Partial而可伸缩，需要显式区分逻辑足迹、有限观测cells/序列化上限和真实EVM失败。不得将策略cap当成协议最大内存、丢掉copy/alias/overlap/overflow效果，或直接删除超限分支。待授权方案可在物化预算不足时维持保守默认+逻辑写入/区间效果，但还需证明读写与后续返回覆盖；此stage不改core。

这些是使默认未知输入能继续的候选语义需求，不是已测热点或性能收益。只有集成候选在固定输入上真正交付Report、保留完整效果/失败/前沿，并通过联合包含性验收，才有资格比较wall/CPU。当前harness仅支持CLI stdout；现有matching server需独立private HTTP/fixture adapter后才能测，不能用server ELF冒充CLI或把网络获取与offline耗时混比。
