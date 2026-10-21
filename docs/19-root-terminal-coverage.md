# 根执行的保守终止覆盖

CLI 与 Web 默认采用快速保守策略：`max_work = 2_000_000_000`，`root_terminal_coverage = true`，在同一预算内为实际原始根输入准备独立的终止覆盖记录。Web 在 **Execution budget** 中默认勾选 **Prepare conservative root terminal coverage**；共享请求字段为 `limits.root_terminal_coverage`，必须是布尔值。CLI 用 `--no-root-terminal-coverage` 关闭附加覆盖，仅做详细分析；原有不带值的 `--root-terminal-coverage` 仍可使用。Rust 库的 `ExecutionConfig::default()` 同样开启覆盖，但保留自身 20000000 的 work 预算。

先分别看两个结果：

| 记录 | 回答的问题 |
| --- | --- |
| `status`、frontiers、pending payload、执行记录和部分 SSA | 详细抽象执行实际展开了哪些状态、指令和转移，还有哪些工作未完成？ |
| 可选 `root_terminal_coverage` | 对绑定的原始根输入，所有可能终止的返回数据与最终事务效果是否由所附保守关系覆盖？ |

附有记录时，详细分析仍可为 `Incomplete`，CFG 与 SSA 仍可部分完成。原始 frontier、pending machine、SSA 覆盖、执行过的 PC、CFG 边与普通 `outcomes` 保留原义；CLI 仍返回 **2**。查看部分 SSA 时显式加 `--allow-partial-ssa`，`analyze` 还需 `--ssa`。图上的终止计数只来自详细执行证据。证书中的三个备选项不增加原生状态、边、已观察结果或已执行指令，也不闭合 frontier。

详细分析已 `Converged` 时，即使默认开启覆盖也可以没有附加记录；界面与文本不应因此显示 **Root terminal certificate not attached**。详细分析为 `Incomplete` 且没有记录时，只能说明未附加证书，不能从 `None` 推断某个未报告的原因。wire 报告显式使用 `null`；原生 JSON 省略未附加的可选字段。

## 从同一份原始输入读三个备选项

`RootTerminalCoverage.scope` 为 `AllInputStates`。`input` 绑定实际原始根输入，`outcomes` 固定按 **Return、Revert、Failure** 排序，每项只有 `kind`、`data` 和 `store`。wire 的后两项分别引用真实 `byte_arrays`、`stores` 池；它们不含终止状态 ID 或终止 PC。`trigger` 中的 mandatory-retention state/PC 指向被中断的详细指令，不是额外构造的终止位置。

例如 runtime `6003565b602a5f5260205ff3` 先跳到第二个块，再返回 word 42。专门的受控覆盖用例把 transfers 设为 1，使详细传播停在真实 transfer frontier；其余默认策略保持不变。附加的 Return 项允许任意返回字节及保守效果，不能据此声称已经观察到该程序返回 42。详细执行实际抵达 RETURN 后产生的普通 outcome 才携带对应的执行证据。

三个 tuple 共同解释如下：

| 种类 | 数据 | 事务效果 |
| --- | --- | --- |
| Return | 完整的任意长度与内容的抽象 byte view | static 与非 static 根均使用独立的全账户效果上界，再执行真实事务收尾。static 输入绑定继续为 true，但这个粗包络可能允许额外效果与日志。 |
| Revert | 完整的任意长度与内容的抽象 byte view | 恢复原始 saved checkpoint，再执行真实的根事务收尾规则。 |
| Failure | 精确空数据 | 同样使用根回滚与事务收尾；保留 owner 3 的特殊 touch 规则。 |

Store 的 `finalize_transaction_checked` 路径负责 transient 清理、touch 清理及相应账户生命周期规则。回滚恢复的对象包括账户字段、代码、persistent/transient 根和日志；最终清理可能继续改变账户存在性及其关联字段，因此不能要求最终 Store 与尚未收尾的 checkpoint 逐字段相等。static 的详细 EVM 写限制仍按原规则执行；覆盖记录选择更宽的 Return 上界，以包含未列出空账户因 touch 而被清理的情况。它不保证 static Return 的余额、代码或其他非 touch 字段等于 initial Store，也不把包络允许的额外效果当成可行见证。

一个 tuple 内的完整数据和完整 Store 必须在**同一输入赋值**下联合解释，不能从不同项各取一个有利投影。三个种类均被保守允许，不证明每种结果都可行，也不提供具体执行见证或程序必然终止的结论。

## 绑定决定覆盖范围

记录的权威来自 Core 在真实根初始化位置捕获的不可变输入；序列化的展示投影只说明它，不能扩大它。数值摘要或字节展示变宽时，`AllInputStates` 仍只针对那份实际原始输入。它不对一个仅由显示字段重新拼成、更大的输入集合自动成立。

绑定保留以下信息：

- snapshot：完整 chain ID/block hash，或 offline label **及独立进程 namespace**；同名 offline label 不足以识别同一快照。
- fact revision、不可变 world storage-source expression，以及原始 symbolic input scope。
- fork、完整有效环境、完整执行配置和 `DomainSpec`，包括成本、来源和表达式/关系策略。
- 原始 Runtime frame：代码账户/hash、物理 storage owner、逻辑 ADDRESS、CALLER、static、入口块索引 0、空栈和空 jump history。
- 捕获的完整 program、lift 后的 calldata/CALLVALUE、原始 saved Store 与 entered initial Store。

代码账户、逻辑 ADDRESS 与物理 storage owner 必须分别读取。仅提交字节码而未提供地址时，逻辑 ADDRESS 可以仍为符号 `To`，内部物理 owner 则为 `0x1111111111111111111111111111111111111111`；这是一种有效绑定。当前账户的 code 若未知，不能从 original World 补回“已知 runtime”。捕获的正在执行的 program 与后续账户 code 效果是不同信息。

补事实会产生新 revision，并从入口建立新的执行与覆盖权威。snapshot、revision、source、输入、fork、环境或策略变化后，旧证书不能冒充新请求的证书；相同 hash、显示摘要或相同 offline label 都不足以授权复用。

## Store 的缺省字段也是语义

wire 16 的普通 Store 与证书 Store 使用同一 `StoreSnapshot`。原字段继续保留，另有：

| 字段 | 含义 |
| --- | --- |
| `storage_root`、`transient_root` | 未列出账户使用的真实 **unscoped base template**，类型为 `SlotStateRef`。读取时再应用 owner scope。两个 owner 共用 template 不等于它们的槽值相等。 |
| `nonce_default`、`balance_default` | 未列出账户的实际抽象 nonce、余额。 |
| `code_default` | `Option<AccountCode>`；`null` 表示 Unknown，允许 runtime、delegation 或 empty 的可能，不能解释为 Empty。非空值完整区分 Runtime 字节、Delegation 目标、Empty 与 Unknown。 |
| `existence_default` | Unknown、Present 或 Absent。 |
| `created_default`、`pending_destruction_default`、`touched_default` | 未列出账户的生命周期/touch 保证；`null` 保留两种可能。 |

Return 的全效果包络可以没有显式账户：persistent、余额、nonce、代码和存在性等由这些默认项及根表示，不是空世界。初始 Store、回滚 Store 与全效果包络各自导出自己的实际默认值；不要给所有 Store 填一组展示常量。账户级 storage/transient 根与全局 template 同时保留，历史读与当前更新不能混淆。

## wire 15 的身份迁移

当前原生分析 schema 为 **11**，Web wire schema 为 **16**，域 schema 仍为 **2**，逻辑成本版本为 **6**。此前 native 10 / wire 15 新增可选覆盖记录；wire 15 还引入请求开关和上述 Store 字段，同时把 `InputIdentity.scope` 改为 `DecimalU64`。新 snapshot offline namespace、fact revision、input scope 也采用同一个类型。当前报告新增 `WorkLimit`、`Cancelled`、`KernelFailure` reduction reason，与局部 `RoundLimit` 区分；cost 6 更新共享 kernel、规则和独立数值查询的逻辑收费。成本版本属于完整输入绑定与摘要策略，不能只替换版本号复用旧记录；跨成本版本的原始 work 不是同一单位。

这些 opaque 身份只能编码为规范十进制**字符串**：`"0"`、`"9007199254740993"`、`"18446744073709551615"` 均有效。JSON number、空串、符号、空格、小数、指数、前导零（如 `"01"`）和超过 `u64::MAX` 的值必须拒绝。不要先经过 JavaScript `Number`，也不要为身份 alpha-renaming 丢失 namespace 的复用关系。状态/转移/work/耗时等统计计数仍保留原来的整数编码；此次迁移只改变这些 opaque identity 字段。

共享类型严格检查必填字段和未知字段。当前客户端应构造完整 wire 16 请求，普通用户策略显式使用 `root_terminal_coverage: true`；复现旧基准策略时显式使用 `false`。旧 wire 14 报告还缺少 Store 默认项和字符串身份，不能只改版本号后安装。历史捕获报告、原始请求和基准 profile 继续按原版本保存。

## 预算与验证范围

准备阶段使用**同一个**执行预算，先完成并支付三个备选项的构造、真实收尾、保留与发布所需工作，然后开始详细传播。准备本身会消耗 work，因此同样配置下可能减少剩余的详细分析工作。准备不会增加额度、返还预算、重置账本或另开执行轮次。2000000000 是可覆盖的用户默认值，不是新增的公开硬上限。最终只发布已经准备并支付的记录；取消仍由原始执行的取消身份约束。

语义验收要求：原始输入集合 `Γ(I)` 中每个具体输入的每个终止结果，都属于同种类 tuple 的联合解释。记完整终止关系为 `Terminal`，要求：

```text
Terminal(Γ(I)) ⊆ Γ(Return, dataR, storeR)
                ∪ Γ(Revert, dataV, storeV)
                ∪ Γ(Failure, empty, storeF)
```

允许精度下降，但不能遗漏任何原始输入的终止数据、账户效果、回滚或收尾。这个包含关系的生产对应还需逐项核对真实原始输入、owner/source/history、任意完整 byte view、Store 全字段、static/touch/rollback/finalization，以及一次性已付费的 capability 与取消行为。协议引用校验只验证结构和一致性，不证明整个 Rust 执行器或完整 EVM 数学语义。

2026-10-18，公开证明提交 `2c0181de2a9da64818b4eaf1b490f0594102e71b` 的 `checks.x86_64-linux.lean-proofs` 实际退出 **0**：15 个公开模块的 296 个目标全部通过，包含新增 `RootEnvelope` 的 20 个与 `RootCoverageCapability` 的 16 个目标；旧 join 负例按预期退出 1，公理依赖仅为 `Classical.choice`、`Quot.sound`、`propext`。相同证明源码已集成于 `e5f13b82c16798431ad0bf7cbb7cfdb7ef3dc2f3`。

这些[局部 Lean 模型](proofs/README.md)证明 finalized whole-world 构造、共享原始输入见证的回滚与未来 owner-3 touch、历史扩展，以及带输入/取消守卫的一次性预付发布状态机。证书 `denotes` 使用 `finalizedTerminalEnvelope`/`FinalizedGamma`；旧 Return=True 构造原语不构成这份证书证明。守卫来自调用方 Bool 观察，真实 Rust Gamma、source/owner/log/ByteArray 表示、计费、allocator 资源上界与 EVM step/callback/CFG 闭包仍需[手工生产对应](proofs/correspondence.md)，不能据此声称完整 Rust/EVM soundness 或 A2 已全部证明。

2026-10-18，干净的最终集成提交 `9a1a0903345ab695a0471846c0625a5b56c0da93` 完整执行 `nix flake check --print-build-logs --no-update-lock-file --keep-going`，实际退出 **0**，耗时 **551.799297467 s**。匹配 server、Wasm/JS 资源的浏览器回归 **38/38** 全部通过，包含 `root-terminal-coverage`：实际表单 POST 的默认开启与 2B work、显式关闭、详细 `Converged` 且无证书、三项 pool 引用及输入绑定、原有未完成详情和独立 tuple 浏览。该结果绑定此实测提交；合并前按[本地检查流程](local-ci.md)核对最终提交与产物（PR 记录最终结果）。

同提交、同一冻结 CLI ELF 的九次最终离线 native 测量见[最终集成实测](../examples/no-smt-scale/README.md#最终集成实测)。默认 2B/ON 的 WETH/USDC 详细分析仍为 `Incomplete`、native exit 2，保留全部原始 frontier；所附 `AllInputStates` / Work 记录固定包含 Return / Revert / Failure 三个联合上界，不改变详细 status、pending 或 SSA 覆盖。固定 1T/OFF 对照与原 whole-report guard、独立流式校验的边界分别列明。

规模实验继续保留[固定七个 case 与原策略](../examples/no-smt-scale/README.md)：work 为 1000000000000，覆盖关闭。[命令生成器](../scripts/generate-no-smt-scale-commands.py)为每个 case 显式加入 `--no-root-terminal-coverage`；可选的 `--request-output-dir` 将 WETH/USDC 的 wire 16 请求写到独立目录，显式设置 `limits.root_terminal_coverage: false`，其余输入与限制不变。历史捕获、hash 和结果不重写。新增保守覆盖不代表这些输入已经详细闭合；性能结论仍需相同输入、完整策略和交付边界的实际测量。
