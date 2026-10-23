# 17：来源、长度与不可变 EVM 状态

先从一段字节码开始：`366000803700` 读取 CALLDATASIZE，把当前 calldata 从偏移 0 复制到 memory 偏移 0，再 STOP。省略 calldata 时，输入的长度和内容未知；`--evm.calldata 0x` 明确给出空输入。未知长度具有一个符号表达式，复制保存来源和范围，不能把尚未展开的字节解释成空数组。

```bash
nix run . -- explain --hex 366000803700
nix run . -- cfg --hex 366000803700 --format json
```

输入长度未知时，COPY 仍保留符号来源和完整范围；超过字节物化上限不排除合法的成功执行。内存访问另检查逻辑可表示范围，并保守覆盖 gas 不足的 Failure，不能把未实际展开的字节当成零或空输入。`cfg --format json` 的输入路径是 `.environment`，世界分析的对应路径是 `.entry.environment`。

## 一次写入产生新状态，旧读取仍绑定旧状态

`EvmByteArray<N>` 的内容根是 `ByteStateId`；length、显式数值字节和 default 是配套的数值事实与观察。`SlotStateId` 表示某个账户、某个 storage plane 的初始状态和有序更新。两种状态都是不可变 DAG：写入产生新根，读取捕获当时的根。后面的写入不会修改早先读出的 word。

| 节点 | 保留的关系 |
| --- | --- |
| Input / Exact / Zero | 来源身份、有限长度、固定观察、精确内容或已知全零 |
| WriteByte / WriteWord | 旧状态、符号地址、值、写入宽度和顺序 |
| Copy | 旧目标、目标偏移、复制前的 source view、逻辑长度 |
| Slice / View | 旧来源、起始偏移、边界与零补 |
| Ite | 同一个 nonzero selector 对应的两个完整替代 |
| ByteRead / WordRead / SlotRead | 读取时的不可变状态与地址 |
| Keccak | 被哈希的完整、有长度的内容序列 |

`MSTORE(p,x); MSTORE8(p+31,0xaa); MLOAD(p)` 通过有序 word/byte 更新表达最后一个字节的覆盖。`SSTORE(k,7); SSTORE(q,9); SLOAD(k)` 则保留写序：q=k 时为 9，q≠k 时为 7。条件尚未证明时保留其关系，不能任挑一个候选值充当确定答案。

来源身份不是内容差异证明。独立来源可能具有相同内容；同一来源的完整复制则可共享内容关系。固定离线世界的未知槽身份随该世界的 clone 保持，独立世界即使标签相同也不是同一个输入。RPC 世界绑定固定 chain/block/fork；新获取的初始观察不能把当前交易写入替换成链上旧值。

## EVM 状态语义与局部推理

typed Word256 表达式、byte/slot 根、条件与有序更新继续保留。局部推理利用已保留的身份和数值事实收窄上界；帧、调用、范围、零补、字节顺序和状态生命周期仍由 EVM 模型负责。图上存在一份表达式并不自动得到精确读取结论。

新 memory 全零，MSTORE/MLOAD 使用 32 个大端字节，MSTORE8 取最低 8 位。View 保留长度，普通源读取按语义补零。访问范围使用数学意义的加法、溢出检查与零大小分支；栈 ADD 等指令继续使用 Word256 回绕。不能用回绕后的 `offset+size` 掩盖范围越界。

逻辑内存足迹与字节物化分开：空访问不扩容；非空访问要求 `offset + size` 及向上取整到 32 字节的末端可以表示，即末端不超过 `U256::MAX & !31`。MCOPY 对来源和目的范围都检查这条条件，并从访问前的同一内存根读取。`max_memory_bytes` 用于具体字节物化和缓存策略，不再限制合法的逻辑长度或 MSIZE，也不是进程峰值 RAM 配额。大型复制可以保留一条 Copy 关系，不要求逐字节展开；CREATE、预编译或哈希需要具体数据时，另按各自提取路径和额度处理。

RETURNDATACOPY 检查完整 returndata 的源范围，零大小仍遵守源偏移规则；CALL 输出只覆盖实际返回前缀，不把其余请求区间清零。MCOPY 捕获复制前的源，因此重叠区域保持 memmove 语义。某个操作数只有数值 Top、没有可信长度表达式时，表示层仍可报告 `ByteRange::UnknownSize`；这与保留了符号长度的正常 COPY 不同。

局部检查区分已证明矛盾、未证明矛盾，以及真实的配置、表示或取消边界。只有证明才能排除路径或收窄数值，普通的宽上界可以仍为 Top。可选数值精化遇到真实界限，但条件已完整保留且保守传播能继续时，`RelationalPrecisionLost` 记录原因。真正未覆盖的执行、没有保留下来的条件、构造失败或共享工作耗尽则保留 frontier，取消仍停止后续工作。

## 帧与账户分别绑定

CALL/STATICCALL 使用目标账户的 storage；CALLCODE/DELEGATECALL 使用调用方的 storage owner，代码仍来自实际目标。不同帧 memory 独立；子帧 calldata 是父 memory 指定 view，不能自动绑定为父 calldata 的整个输入。

Persistent 与 transient 有不同根和 namespace。Transient 在交易入口全零，随同账户嵌套调用共享当前状态，并遵守回滚和交易终结。失败子调用恢复其入口检查点及后代效果；父调用进入子帧之前的修改仍存在。保存点同时持有符号根、账户生命周期和其他可回滚效果。

CREATE/CREATE2 需要执行实际 initcode，再验证生成的 runtime。符号字节 view 本身不提供可执行 opcode；无法具体化所需 initcode/runtime、creator/nonce/salt 等事实时，使用相应的 typed creation 边界。预编译有不同的处理路径：[`precompile.rs`](../crates/evm-abstract/src/analysis/transfer/precompile.rs) 先尝试有界具体执行，输入未知或可选具体计算过贵时改用保守摘要，覆盖成功、回滚和失败；Identity 的成功数据保留输入，其余返回长度与内容按预编译种类概括。真实工作耗尽、构造失败或原生后端错误仍保留相应边界，不能把所有未知预编译输入都写成未完成。完整符号账户生命周期的支持范围须以相应实际指令测试为准。

KECCAK 对同一完整内容保持同值。不同来源、不同地址、不同内容均不自动证明哈希不同；不加入 injectivity 或无碰撞假设。可有界提取的真实内容使用真实 digest，并把该观察约束回同一个历史 Keccak 表达式。

当前 hash equality 编码将最多 64 个具体长度字节打包成 512 位内容，不足部分补零，并把原始长度作为同一函数的独立参数；独立地址得到的相同小内容可使用同一函数签名。原标准化数组在这些字节以外恒为零，所以此表示保留相同的完整内容相等关系，同时避免逐字节 store 链和数组外延相等查询。word 读取仍保留源边界与数学地址溢出的零补，不读取范围外的内容。原始 source view、可直接降低的 native array 和有非回绕 guard 的完整 Copy 另有对应支持。一般 lazy view 未能比较内容时使用 `HashSupport::Uncompared`：相同 immutable view 保持稳定，跨 view 的内容相等仍未证明，记录 `RelationalPrecisionLost(Unsupported(KECCAK256))` 并完整保守继续。不能把这一精度边界写成所有哈希目标已经验证。

## 汇合、固定点与摘要来源

整机 guarded join 为栈值、地址、byte/slot 根、continuation、rollback store 和完整路径公式使用同一个 selector。这样可以表示“p=0x80 时对应那份 memory，p=0xc0 时对应另一份”，避免逐字段交叉配对。路径公式保留 inherited formula 与当前 constraints，二者都属于完整状态。

两边完全相同的无条件约束只保留一份，剩余约束仍由同一个 selector 选择。例如 `ite(s, C AND L, C AND R)` 编码为 `C AND ite(s, L, R)`；只在某个分支成立的条件不能提到 selector 外面。这减少重复编码，并让读取编码继续使用两边共同成立的范围保证。

与自身或已表示的完整替代合并可以吸收。覆盖判断核对完整载荷，必要时用有界查询检查标量和路径保证；UNKNOWN 不能证明覆盖。重复相同覆盖通过规范化减少根的增长，不能仅因内部版本号增加就判定内容改变。

普通循环反馈由 [`Engine::successor`](../crates/evm-abstract/src/analysis/engine.rs) 根据实际图中的回路识别；同一入口出现重复增长后，[`EntryAlternatives::join_loop`](../crates/evm-abstract/src/analysis/machine/join.rs) 将其提升为 [`LoopBound`](../crates/evm-abstract/src/analysis/state/loop_bound.rs)。它持有数值整机上界，并与过程保留失败重试共用 [`BoundedState`](../crates/evm-abstract/src/analysis/state/bounded.rs) 的冻结布局、规范化及独立物化机制。后续输入都更新这份上界；重试准入和结果发布仍由各自调用方负责。反复到达同一个 pc 本身不足以宣告固定点。

首次扩大用已有入口和新输入确定布局：包括帧与 continuation 的结构、选定的账户/槽键、各帧 memory/calldata/returndata 的字节坐标，以及每个回滚 Store 的独立布局。后续出现的新位置不继续增加坐标，而由 [`EvmStoreSummary::project_to_layout`](../crates/evm-abstract/src/world/store/summary/layout.rs) 和 [`EvmByteArraySummary::project_to_layout`](../crates/evm-abstract/src/world/bytes/summary/layout.rs) 归入默认上界。Store 的布局外写入需要扩大相应 owner 或全局默认值；未知 owner/key 的写入还必须覆盖所有可能别名的已选单元和默认区域。字节数组未保留的单元为任意字节，不能解释成零。内容与逻辑长度分别概括，代码与账户生命周期还受结构键约束。

投影从 byte/slot 根取得数值上界，随后舍弃旧表达式、写入历史、来源身份、路径公式及跨组件关联。当前实现同时忘掉可选栈别名类和 storage 观察关联，避免仍引用已被抹去的根。保留的是数值约束、控制结构和可能效果的包络；日志摘要不是精确的有限事件序列，`logs_unknown` 也不表示没有日志。这里不能沿用普通 guarded join 对所有字段精确配对的读法。

[`payload::instantiate`](../crates/evm-abstract/src/analysis/state/payload.rs) 为每个缓冲区、当前 Store 和各帧保存的 Store 分别实例化；未知部分使用独立的新来源。历史读取按原先捕获的版本解释；投影后不能把它与新当前根强制设为相同。初始快照事实也不能覆盖执行中的写入。独立化会丢失关联，但不能把当前状态与回滚状态合成一份：失败返回仍须恢复保存的版本。

`LoopBound::update` 先将输入限制到冻结布局，join 后比较规范化的**完整上界**，再决定是否扩大并实例化。比较不使用新生成的 `SourceId` / `ExprId`，也不把某个数值组件相等当作覆盖。只有新上界与执行载荷都构造成功、结构键检查通过，入口才安装结果并清除历史完整 cases；上界扩大后需重新调度。摘要证书导入已有循环入口时也走这份上界。会话将这种根抽象记为 `AllInitialState` 依赖，不能继续只依赖原先少数已观察槽。

独立来源实例化若因表达式或状态节点保留额度被拒绝，当前实现只尝试一次更粗的上界：标量保留无符号区间，字节内容、槽值及部分账户数值可失去更多信息，日志可变为未知。成功时存下的是这份更大的上界，后续覆盖不能使用先前被拒绝的精确版本。进入粗规范化模式后，后续投影与 join/widen 也先统一到同一模式再比较，避免重放时恢复已放弃的细节并反复增长。取消、共享工作耗尽、结构不符或真实范围错误不作这种重试。最终构造失败时保留 incoming payload 与对应 typed frontier；不能以丢掉未处理输入来关闭循环。

摘要重放共享一张 rename map，遍历隐藏在 state source、view、rollback 和返回数据中的 fresh token。固定输入身份保持绑定，调用中新产生的未知来源每次重放使用新身份。重命名和导入按完整变换提交，失败不能安装一半状态。

## 原生 JSON 12 与 Web 协议 17

此前 Native 10 新增可选的原始根终止覆盖；wire 15 引入同一真实 data/store 池，并为普通 Store 也导出全局 unscoped `storage_root` / `transient_root`、nonce/code/existence/lifecycle 默认值。当前 Native 13 / wire 18 保留这些字段，新增 `WorkLimit`、`Cancelled`、`KernelFailure` reduction reason。`code_default: null` 是 Unknown；共用 base template 的不同 owner 不因此共享槽值。wire 的 `InputIdentity.scope`、offline namespace、revision 和 input scope 使用严格规范十进制 `DecimalU64` 字符串，完整保留 `u64::MAX`，统计计数不变。字段迁移、绑定与联合语义见[根终止覆盖](19-root-terminal-coverage.md)。

Native analysis schema 为 12；域策略自身的版本独立。原生 `expression`（原始 ExprId 序列化）、byte root、slot root 与 path 采用 flat graph：节点按依赖顺序排列，root 指向本图索引。消费者必须迁移，旧递归 JSON 没有隐式兼容。`GraphLimits::default()` 的序列化额度为 65,536 节点工作、深度 256、16 MiB exact payload。payload 单独计量，24 KiB 精确代码来源无需按每个 byte 消耗一个表达式节点。这与分析时库默认的 1024 节点额度不同；扩大导出额度不能修复此前的分析或局部表达式遍历超限。

原生机器值只有 `.numeric` 必填；未知来源、空身份、无表达式与 false 标志可省略，实际非默认元数据仍保留。product 与 finite-set 的默认数值 Top 都写成空对象。product 仅省略 Top/false 组件，每个非默认组件仍完整保留。来源图的 `WordObservation` 使用独立观察编码；Web `ValueInfo` 是展示投影，不能用它替换实际载体的原生数据。

原生 graph 的 `identity_scope`、Fresh 编号及 Input scope/symbol 保留同一进程中的身份。source 的 `token` 是本图中的表达式引用，须先解析它指向的身份，再结合 namespace 判断来源；不能直接比较两个图的 token 或 root 索引，也不能用同名打印叶跨报告绑定输入。原生 CLI JSON 通过 `render::json` 的流式 writer 输出，整份 UTF-8 输出不设置字节上限，每次追加前检查取消。`pretty` 与 `compact` 沿用当前线程的取消作用域；应用可用 `with_control(value, control)` 或 `compact_with_control(value, control)` 明确传入控制。取消返回 `JsonError::Cancelled` 并记录已写字节，保留已计算的分析事实；值序列化失败继续返回 `JsonError::Serialization`。原生 pending Arc 快照仍可能重复其程序和状态，Web 使用下面的共享目录。

世界指纹保留原 v2 编码和全部初始观察；`EvmWorld::fingerprint` 返回 `Result<B256, FingerprintError>`。取消、值编码失败和长度前缀溢出分别是 `Cancelled`、`Encoding`、`LengthOverflow`，不生成替代 digest。`render::world::evidence` 在报告输出前有界计算该指纹并返回可失败的借用视图，原生 JSON 通过 `JsonError::Fingerprint` 传播；文本和 DOT 明确显示 unavailable 和错误并停止渲染。Server 使用 `ErrorDetails::Fingerprint` 保存类型与编码坐标，取消映射为 Cancelled，其余失败映射为 Export。调用方须在适用的 `Control.scope` 内计算 evidence，随后 writer 仍逐次检查取消。

Web/server 的单份 `AnalysisReport.symbolic.nodes` 共享所有 scalar、byte、slot、view 和 path。引用按语义分为 `ExpressionRef`、`ByteStateRef`、`SlotStateRef`、`ByteViewRef`、`PathRef`；依赖指向较早节点。`ValueInfo.expression` 是 scalar ref；`ByteArraySnapshot.symbolic` 是 view ref；账户 `storage_root`、`transient_root` 分开。状态局部 `symbolic_facts` / `storage_facts` / `transient_facts` 还保存已被指令消耗的 symbolic operand 数值信息，不能跨快照错误复用。来源 token、namespace、非 singleton 常量集、位、signed/unsigned bounds、nonzero、congruence、provenance 和 equality identity 均保留。

`MachineSnapshot.path` 表达完整状态条件。`Frontier.range` 保留 offset/size/limit、condition 和 nonzero side；其 payload 与一般 join 的 `Frontier.payload` 都引用 `pending_machines`。Program、store、byte source 仍在原共享目录中，不在每个 frontier 递归倾倒完整分析。

导出独立计量实际 DAG 节点、exact bytes 与遍历/池化工作，默认上限为 65,536 节点、深度 256、16 MiB exact payload。策略和实耗分别在 `metadata.export_limits`、`metadata.export_usage`；其中 `export_usage.work` 仅作诊断统计，没有累计工作硬上限，也不会因 work 总量拒绝导出，执行分析的 `AnalysisLimits.max_work` 继续生效。Web 协议 17 的 `ExportLimits` 只声明节点、深度和 exact payload 策略，HTTP 整份报告 JSON 不设置字节上限。编码取消仍报告 typed `Cancelled` export failure，保留 `required` / `written`、图实耗和原策略，缓存中的分析事实不变。图或 exact payload 超限仍报告相应 typed export failure，不把成功分析改写成数值 Top。UI 校验版本、节点 sort、依赖顺序和 pool refs 后安装结果；DAG 与 byte/slot 行按可见区渲染，exact payload 只显示有界预览。

应用的局部 CFG 投影、SSA 构建与验证以及请求的渲染计算，另使用 `DEFAULT_POSTPROCESS_WORK = 2_000_000_000` 的有限共享后处理账本。它在实际复制或查询之前付费，与执行 `max_work` 分开，在这些阶段之间不重置；`postprocessing_max_work` 与 `postprocessing_work` 记录该边界。导出池已有的 `export_usage.work` 只是统计，不能充当这份预付账本，也不表示所有外部序列化都已被计费。后处理 Work/Cancelled 保留为类型化输出失败，不能把未完成 SSA 改成完整结果。

`LogicalMemory` 范围证据使用 word-rounded U256 可表示上界，和物化策略分开；可表示域内的精确 gas 充足性仍未证明，成功与保守 Failure 分支可以同时存在。历史 `MemoryPolicy` 证据继续按旧 footprint 策略解释，不能当作 EVM Failure。`ByteRange` 描述表示层错误，`StateJoin` 描述完整机器输入无法合并；正常 alias 的未定精度不能显示成已证明成功。

未知执行过程的闭合关系、摘要返回边与过程覆盖目录见[第 18 课](18-unknown-world.md)。关系表会显式进行数值投影，保留保守上界；这种投影与原有完整图摘要的身份重命名有不同精度边界。

Native schema 13 与 Web schema 18 的 LOG 来源身份同时包含 emitter、code account、captured code hash、Init/Runtime 和 pc。初始化与部署后代码即使在同一地址和偏移发出 LOG，也保留不同来源；Web 日志面板按完整身份分开，展示捕获的代码 hash。Native `source_kind` 为 `init` / `runtime`，Web 共享枚举为 `Init` / `Runtime`。

## 验证边界与证据

原生 solver 及其接口已移除。表达式、数值事实、byte/slot 写序、路径与取消分别保留独立回归；这些检查不自动证明所有 EVM 路径都被精确覆盖。实际 opcode、上下文、rollback、summary、收敛与 revm 对照需要按具体模型分别验收。

教程中的同址 memory/storage、有限别名、完整 word 往返、重叠复制与矛盾分支例子，各自核对一项具体关系。未知 memory 地址或 calldata 长度本身不会强制产生物化策略前沿；是否完成要读取实际 `status`、outcomes 与 frontiers。`LoopBound` 还需分别验证投影覆盖原载荷、join/widening 覆盖旧新输入、独立实例化覆盖摘要，以及“投影 → 实例化 → 再投影”得到稳定的规范上界；单次规范相等或清除历史 cases 不能替代这些义务。固定布局不保证任意 EVM 循环都完成；历史读取、回滚、日志及摘要重放仍须按实际图、输入与分析额度分别检查。库默认 1024 节点与 CLI 默认 16384 节点也不是同一种验证条件，不能由简单例子的通过推断全部默认策略已验证。

协议验收还须从真实 COPY/storage alias 分析经过 typed report、JSON 解析、前端引用校验与 SSA，并覆盖大共享图、range prestate、预算和取消。旧 SMT 实现的专项源码 SHA、fork、输入假设、provider/rlimit、节点/状态/求解成本、内存与耗时保留于历史的 [Issue #111 集成验收](issue111-validation.md)；最终提交的完整 Nix 门禁结果另记于实施 PR。
