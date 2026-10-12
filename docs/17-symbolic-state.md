# 17：来源、长度与不可变 EVM 状态

先从一段字节码开始：`366000803700` 读取 CALLDATASIZE，把当前 calldata 从偏移 0 复制到 memory 偏移 0，再 STOP。省略 calldata 时，输入的长度和内容未知；`--evm.calldata 0x` 明确给出空输入。未知长度具有一个符号表达式，复制保存来源和范围，不能把尚未展开的字节解释成空数组。

```bash
nix run . -- explain --hex 366000803700
nix run . -- cfg --hex 366000803700 --format json
```

输入长度没有上界时，结果还须保留超过声明内存策略的输入。正常范围内的复制和超策略前沿可以同时存在；增大内存策略改变覆盖范围，不提供 calldata 的真实长度。`cfg --format json` 的输入路径是 `.environment`，世界分析的对应路径是 `.entry.environment`。

## 一次写入产生新状态，旧读取仍绑定旧状态

`ByteArray` 的内容根是 `ByteStateId`；length、显式数值字节和 default 是配套的数值事实与观察。`SlotStateId` 表示某个账户、某个 storage plane 的初始状态和有序更新。两种状态都是不可变 DAG：写入产生新根，读取捕获当时的根。后面的写入不会修改早先读出的 word。

| 节点 | 保留的关系 |
| --- | --- |
| Input / Exact / Zero | 来源身份、有限长度、固定观察、精确内容或已知全零 |
| WriteByte / WriteWord | 旧状态、符号地址、值、写入宽度和顺序 |
| Copy | 旧目标、目标偏移、复制前的 source view、逻辑长度 |
| Slice / View | 旧来源、起始偏移、边界与零补 |
| Ite | 同一个 nonzero selector 对应的两个完整替代 |
| ByteRead / WordRead / SlotRead | 读取时的不可变状态与地址 |
| Keccak | 被哈希的完整、有长度的内容序列 |

`MSTORE(p,x); MSTORE8(p+31,0xaa); MLOAD(p)` 通过有序 word/byte 更新表达最后一个字节的覆盖。`SSTORE(k,7); SSTORE(q,9); SLOAD(k)` 则保留写序：q=k 时为 9，q≠k 时为 7。条件尚未证明时保留其关系，不能任挑一个 solver 赋值充当确定答案。

来源身份不是内容差异证明。独立来源可能具有相同内容；同一来源的完整复制则可共享内容关系。固定离线世界的未知槽身份随该世界的 clone 保持，独立世界即使标签相同也不是同一个输入。RPC 世界绑定固定 chain/block/fork；新获取的初始观察不能把当前交易写入替换成链上旧值。

## EVM 语义由分析器负责，SMT 检查后果

求解器接收 typed Word256、数组 select/store、条件与函数应用。分析器仍负责帧、调用、范围、零补、字节顺序和状态生命周期。SMT 不自动决定“哪个账户”“哪个 fork”或“复制是否已经执行”。

新 memory 全零，MSTORE/MLOAD 使用 32 个大端字节，MSTORE8 取最低 8 位。View 保留长度，普通源读取按语义补零。访问范围使用数学意义的加法、溢出检查与零大小分支；栈 ADD 等指令继续使用 Word256 回绕。不能用回绕后的 `offset+size` 掩盖范围越界。

`max_memory_bytes` 约束访问所需的**逻辑 footprint**，包含 word-rounded MSIZE；它不要求为所有字节分配内存，也不表示 COPY 必须逐字节执行，更不是进程峰值 RAM 配额。大型逻辑复制仍是一条 Copy 关系。需要具体代码或预编译数据时，单独进行有界 materialization。

RETURNDATACOPY 检查完整 returndata 的源范围，零大小仍遵守源偏移规则；CALL 输出只覆盖实际返回前缀，不把其余请求区间清零。MCOPY 捕获复制前的源，因此重叠区域保持 memmove 语义。某个操作数只有数值 Top、没有可信长度表达式时，表示层仍可报告 `ByteRange::UnknownSize`；这与保留了符号长度的正常 COPY 不同。

求解结论区分 SAT、UNSAT、UNKNOWN、resource limit、cancelled 和 solver error。只有证明能排除路径或收窄数值；SAT 模型的某个值不构成唯一值或链上观察。可选 value 查询未完成但完整保守传播能继续时，`RelationalPrecisionLost` 记录精度损失。真正未覆盖的执行、没有保留下来的条件、构造失败或共享工作耗尽则保留 frontier。

## 帧与账户分别绑定

CALL/STATICCALL 使用目标账户的 storage；CALLCODE/DELEGATECALL 使用调用方的 storage owner，代码仍来自实际目标。不同帧 memory 独立；子帧 calldata 是父 memory 指定 view，不能自动绑定为父 calldata 的整个输入。

Persistent 与 transient 有不同根和 namespace。Transient 在交易入口全零，随同账户嵌套调用共享当前状态，并遵守回滚和交易终结。失败子调用恢复其入口检查点及后代效果；父调用进入子帧之前的修改仍存在。保存点同时持有符号根、账户生命周期和其他可回滚效果。

CREATE/CREATE2 需要执行实际 initcode，再验证生成的 runtime。符号字节 view 本身不提供可执行 opcode：initcode/runtime、creator/nonce/salt、目标代码或预编译输入需要具体化时，当前语义继续使用 typed creation/target/precompile 边界。完整符号账户生命周期的支持范围须以相应实际指令测试为准。

KECCAK 对同一完整内容保持同值。不同来源、不同地址、不同内容均不自动证明哈希不同；不加入 injectivity 或无碰撞假设。可有界提取的真实内容使用真实 digest，并把该观察约束回同一个历史 Keccak 表达式。

当前 hash equality 编码最多为 64 个具体长度字节构造标准化内容数组；独立地址得到的相同小内容可使用同一函数签名。原始 source view、可直接降低的 native array 和有非回绕 guard 的完整 Copy 另有对应支持。一般 lazy view 未能比较内容时使用 `HashSupport::Uncompared`：相同 immutable view 保持稳定，跨 view 的内容相等仍未证明，记录 `RelationalPrecisionLost(Unsupported(KECCAK256))` 并完整保守继续。不能把这一精度边界写成所有哈希目标已经验证。

## 汇合、固定点与摘要来源

整机 guarded join 为栈值、地址、byte/slot 根、continuation、rollback store 和完整路径公式使用同一个 selector。这样可以表示“p=0x80 时对应那份 memory，p=0xc0 时对应另一份”，避免逐字段交叉配对。路径公式保留 inherited formula 与当前 constraints，二者都属于完整状态。

与自身或已表示的完整替代合并可以被吸收。重复相同覆盖应通过规范化达到稳定根；不能仅因内部版本号增加就判定内容改变。持续增长的真实状态仍受 alternatives、节点、深度和共享工作策略约束。无法完成的 join 保存完整 incoming payload，`StateJoin` frontier 不构成不可达证明。

摘要重放共享一张 rename map，遍历隐藏在 state source、view、rollback 和返回数据中的 fresh token。固定输入身份保持绑定，调用中新产生的未知来源每次重放使用新身份。重命名和导入按完整变换提交，失败不能安装一半状态。

## 原生 JSON 6 与 Web 协议 9

Native analysis schema 为 6；域策略自身的版本独立。原生 `expression`（原始 ExprId 序列化）、byte root、slot root 与 path 采用 flat graph：节点按依赖顺序排列，root 指向本图索引。消费者必须迁移，旧递归 JSON 没有隐式兼容。原始 graph 序列化默认节点工作 65,536、深度 256、exact payload 16 MiB；payload 与表达式节点预算分开，因此正常 24 KiB 代码不会被隐含的 1024-byte 上限截断。

原生 graph 的 `identity_scope` 与来源 namespace 保留同一进程内的 Fresh/输入身份；不能用某份报告的 local root 索引与另一份报告比较。原生 CLI JSON 通过 `render::json` 的流式 writer 输出，整份 UTF-8 输出默认上限为 64 MiB，每次追加前检查预算与取消；应用可用 `JsonLimits` 与 `with_control` 明确选择策略。超过预算返回 `JsonError::Bytes`，包含已写字节、下一次写入所需字节和额度；取消返回 `JsonError::Cancelled`，保留已计算的分析事实。原生 pending Arc 快照仍可能重复其程序和状态，整份 writer 的预算约束这些重复输出；Web 使用下面的共享目录。直接调用第三方 serde 序列化函数不自动获得整份 writer 预算。

世界指纹保留原 v2 编码和全部初始观察；`World::fingerprint` 返回 `Result<B256, FingerprintError>`。取消、值编码失败和长度前缀溢出分别是 `Cancelled`、`Encoding`、`LengthOverflow`，不生成替代 digest。`render::world::evidence` 在报告输出前有界计算该指纹并返回可失败的借用视图，原生 JSON 通过 `JsonError::Fingerprint` 传播；文本和 DOT 明确显示 unavailable 和错误并停止渲染。Server 使用 `ErrorDetails::Fingerprint` 保存类型与编码坐标，取消映射为 Cancelled，其余失败映射为 Export。调用方须在适用的 `Control.scope` 内计算 evidence，随后 writer 仍逐次检查取消与整份输出预算。

Web/server 的单份 `AnalysisReport.symbolic.nodes` 共享所有 scalar、byte、slot、view 和 path。引用按语义分为 `ExpressionRef`、`ByteStateRef`、`SlotStateRef`、`ByteViewRef`、`PathRef`；依赖指向较早节点。`ValueInfo.expression` 是 scalar ref；`ByteArraySnapshot.symbolic` 是 view ref；账户 `storage_root`、`transient_root` 分开。状态局部 `symbolic_facts` / `storage_facts` / `transient_facts` 还保存已被指令消耗的 symbolic operand 数值信息，不能跨快照错误复用。来源 token、namespace、非 singleton 常量集、位、signed/unsigned bounds、nonzero、congruence、provenance 和 equality identity 均保留。

`MachineSnapshot.path` 表达完整状态条件。`Frontier.range` 保留 offset/size/limit、condition 和 nonzero side；其 payload 与一般 join 的 `Frontier.payload` 都引用 `pending_machines`。Program、store、byte source 仍在原共享目录中，不在每个 frontier 递归倾倒完整分析。

导出独立计量实际 DAG 节点、exact bytes 与遍历/池化工作，默认 65,536 节点、深度 256、16 MiB exact payload、64,000,000 work。策略和实耗分别在 `metadata.export_limits`、`metadata.export_usage`。HTTP 整份报告 JSON 还受 `max_output_bytes` 的 64 MiB 策略约束，包含全部数值事实和目录；编码的 `OutputBytes` failure 保留 required/written、图实耗和原策略，缓存中的分析事实不变。超限或取消报告 typed export failure，不把成功分析改写成数值 Top。UI 校验版本、节点 sort、依赖顺序和 pool refs 后安装结果；DAG 与 byte/slot 行按可见区渲染，exact payload 只显示有界预览。

MemoryPolicy 前沿在 UI 明确标记“策略下尚未展开”，与 EVM Failure、REVERT 分开。`ByteRange` 描述表示层错误，`StateJoin` 描述完整替代无法合并；正常 alias 的未定精度不能显示成已证明成功。

## 验证边界与证据

三个 native provider 的 Array/UF 基础接口、原生 resource/cancellation 及 malformed input 边界由 `embedded-smt`、`bitwuzla-native` 的测试覆盖。基础接口通过不自动证明所有 EVM 路径通过；实际 opcode、上下文、rollback、summary、收敛与 revm 对照需要独立验收。

协议回归应从真实 COPY/storage alias 分析开始，经过 typed report、JSON 解析、前端引用校验与 SSA，另测大共享图、range prestate、预算和取消。最终 SHA、fork、输入假设、provider/rlimit、节点/状态/求解成本、内存与耗时由本 issue 的集成证据统一记录；本教程不填入未测量性能数字。
