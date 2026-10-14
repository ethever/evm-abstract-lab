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

求解结论区分 SAT、UNSAT、UNKNOWN、resource limit、cancelled 和 solver error。只有证明能排除路径或收窄数值；SAT 模型的某个值不构成唯一值或链上观察。可选数值或可行性查询未完成，但条件已完整保留且保守传播能继续时，`RelationalPrecisionLost` 记录精度损失。`assume_checked` 区分条件保留失败与保留后的查询失败；普通分支和范围分支都使用这个区别。真正未覆盖的执行、没有保留下来的条件、构造失败或共享工作耗尽则保留 frontier，取消仍停止后续工作。

## 帧与账户分别绑定

CALL/STATICCALL 使用目标账户的 storage；CALLCODE/DELEGATECALL 使用调用方的 storage owner，代码仍来自实际目标。不同帧 memory 独立；子帧 calldata 是父 memory 指定 view，不能自动绑定为父 calldata 的整个输入。

Persistent 与 transient 有不同根和 namespace。Transient 在交易入口全零，随同账户嵌套调用共享当前状态，并遵守回滚和交易终结。失败子调用恢复其入口检查点及后代效果；父调用进入子帧之前的修改仍存在。保存点同时持有符号根、账户生命周期和其他可回滚效果。

CREATE/CREATE2 需要执行实际 initcode，再验证生成的 runtime。符号字节 view 本身不提供可执行 opcode：initcode/runtime、creator/nonce/salt、目标代码或预编译输入需要具体化时，当前语义继续使用 typed creation/target/precompile 边界。完整符号账户生命周期的支持范围须以相应实际指令测试为准。

KECCAK 对同一完整内容保持同值。不同来源、不同地址、不同内容均不自动证明哈希不同；不加入 injectivity 或无碰撞假设。可有界提取的真实内容使用真实 digest，并把该观察约束回同一个历史 Keccak 表达式。

当前 hash equality 编码将最多 64 个具体长度字节打包成 512 位内容，不足部分补零，并把原始长度作为同一函数的独立参数；独立地址得到的相同小内容可使用同一函数签名。原标准化数组在这些字节以外恒为零，所以此表示保留相同的完整内容相等关系，同时避免逐字节 store 链和数组外延相等查询。word 读取仍保留源边界与数学地址溢出的零补，不读取范围外的内容。原始 source view、可直接降低的 native array 和有非回绕 guard 的完整 Copy 另有对应支持。一般 lazy view 未能比较内容时使用 `HashSupport::Uncompared`：相同 immutable view 保持稳定，跨 view 的内容相等仍未证明，记录 `RelationalPrecisionLost(Unsupported(KECCAK256))` 并完整保守继续。不能把这一精度边界写成所有哈希目标已经验证。

## 汇合、固定点与摘要来源

整机 guarded join 为栈值、地址、byte/slot 根、continuation、rollback store 和完整路径公式使用同一个 selector。这样可以表示“p=0x80 时对应那份 memory，p=0xc0 时对应另一份”，避免逐字段交叉配对。路径公式保留 inherited formula 与当前 constraints，二者都属于完整状态。

两边完全相同的无条件约束只保留一份，剩余约束仍由同一个 selector 选择。例如 `ite(s, C AND L, C AND R)` 编码为 `C AND ite(s, L, R)`；只在某个分支成立的条件不能提到 selector 外面。这减少重复编码，并让读取编码继续使用两边共同成立的范围保证。

与自身或已表示的完整替代合并可以吸收。覆盖判断核对完整载荷，必要时用有界查询检查标量和路径保证；UNKNOWN 不能证明覆盖。重复相同覆盖通过规范化减少根的增长，不能仅因内部版本号增加就判定内容改变。

循环处理分两种情况。若除操作数栈与路径外，所有帧内容、byte/slot 根、source、view、rollback checkpoint 和 continuation 都保持相同，栈 widening 可以扩大改变的 word 的数值范围。持续改变的 word 保留扩大的无符号区间，放弃会继续增长的有限集、位和同余细节；未改变的 word 保留原数值事实。结果只保留共同路径保证，不能把已丢失的标量关联仍显示为精确析取。状态根也随迭代改变时，这个方法不适用。

整机循环 widening 另用一组有界的**完整 cases**扩大状态。引擎登记自己在 join 时产生的 selector；循环归一化只将这些 selector 分别代入 0/1，同一张替换表作用于栈、地址、byte/slot 根、返回 view、continuation 和 rollback store。输入 source、GAS 及手工 guard 永不作为内部选择枚举；它们继续是固定或独立的符号变量，不能随意选一个模型值代替。

这一步是显式 widening：先忘掉旧路径公式和条件保证，并清理已失效的显示缓存、临时复制身份及必须成立的初始独立性等元数据，再按完整载荷去重。不可变根的有序效果、历史读取、来源身份和仍可达的初始观察保留。旧路径保证的丢失可以扩大输入集合；它没有声称无限循环后仍保存原来的精确路径公式。重新组合 cases 时使用同一个新 selector 配对所有字段；只有完整 case 集稳定，才可据此吸收新的输入。

完整和待处理 cases 共用 `max_constraints` 的数量上限，另受表达式节点、深度、共享 work 和取消控制。真实 source 或状态效果持续变化可能产生新 case。达到边界保存 incoming payload 与对应 typed frontier：case 数量为 `StateJoin`，表达式限制为 `Relations`，共享工作耗尽为 `Work`。循环判断来自实际图中的回路，不能只因同一个 pc 再次出现就枚举变量或宣告固定点。

摘要重放共享一张 rename map，遍历隐藏在 state source、view、rollback 和返回数据中的 fresh token。固定输入身份保持绑定，调用中新产生的未知来源每次重放使用新身份。重命名和导入按完整变换提交，失败不能安装一半状态。

## 原生 JSON 7 与 Web 协议 12

Native analysis schema 为 7；域策略自身的版本独立。原生 `expression`（原始 ExprId 序列化）、byte root、slot root 与 path 采用 flat graph：节点按依赖顺序排列，root 指向本图索引。消费者必须迁移，旧递归 JSON 没有隐式兼容。`GraphLimits::default()` 的序列化额度为 65,536 节点工作、深度 256、16 MiB exact payload。payload 单独计量，24 KiB 精确代码来源无需按每个 byte 消耗一个表达式节点。这与分析时库默认的 1024 节点额度不同；扩大导出额度不能修复此前的分析或求解编码超限。

原生 graph 的 `identity_scope`、Fresh 编号及 Input scope/symbol 保留同一进程中的身份。source 的 `token` 是本图中的表达式引用，须先解析它指向的身份，再结合 namespace 判断来源；不能直接比较两个图的 token 或 root 索引，也不能用同名打印叶跨报告绑定输入。原生 CLI JSON 通过 `render::json` 的流式 writer 输出，整份 UTF-8 输出不设置字节上限，每次追加前检查取消。`pretty` 与 `compact` 沿用当前线程的取消作用域；应用可用 `with_control(value, control)` 或 `compact_with_control(value, control)` 明确传入控制。取消返回 `JsonError::Cancelled` 并记录已写字节，保留已计算的分析事实；值序列化失败继续返回 `JsonError::Serialization`。原生 pending Arc 快照仍可能重复其程序和状态，Web 使用下面的共享目录。

世界指纹保留原 v2 编码和全部初始观察；`World::fingerprint` 返回 `Result<B256, FingerprintError>`。取消、值编码失败和长度前缀溢出分别是 `Cancelled`、`Encoding`、`LengthOverflow`，不生成替代 digest。`render::world::evidence` 在报告输出前有界计算该指纹并返回可失败的借用视图，原生 JSON 通过 `JsonError::Fingerprint` 传播；文本和 DOT 明确显示 unavailable 和错误并停止渲染。Server 使用 `ErrorDetails::Fingerprint` 保存类型与编码坐标，取消映射为 Cancelled，其余失败映射为 Export。调用方须在适用的 `Control.scope` 内计算 evidence，随后 writer 仍逐次检查取消。

Web/server 的单份 `AnalysisReport.symbolic.nodes` 共享所有 scalar、byte、slot、view 和 path。引用按语义分为 `ExpressionRef`、`ByteStateRef`、`SlotStateRef`、`ByteViewRef`、`PathRef`；依赖指向较早节点。`ValueInfo.expression` 是 scalar ref；`ByteArraySnapshot.symbolic` 是 view ref；账户 `storage_root`、`transient_root` 分开。状态局部 `symbolic_facts` / `storage_facts` / `transient_facts` 还保存已被指令消耗的 symbolic operand 数值信息，不能跨快照错误复用。来源 token、namespace、非 singleton 常量集、位、signed/unsigned bounds、nonzero、congruence、provenance 和 equality identity 均保留。

`MachineSnapshot.path` 表达完整状态条件。`Frontier.range` 保留 offset/size/limit、condition 和 nonzero side；其 payload 与一般 join 的 `Frontier.payload` 都引用 `pending_machines`。Program、store、byte source 仍在原共享目录中，不在每个 frontier 递归倾倒完整分析。

导出独立计量实际 DAG 节点、exact bytes 与遍历/池化工作，默认上限为 65,536 节点、深度 256、16 MiB exact payload。策略和实耗分别在 `metadata.export_limits`、`metadata.export_usage`；其中 `export_usage.work` 仅作诊断统计，没有累计工作硬上限，也不会因 work 总量拒绝导出，执行分析的 `AnalysisLimits.max_work` 继续生效。Web 协议 12 的 `ExportLimits` 只声明节点、深度和 exact payload 策略，HTTP 整份报告 JSON 不设置字节上限。编码取消仍报告 typed `Cancelled` export failure，保留 `required` / `written`、图实耗和原策略，缓存中的分析事实不变。图或 exact payload 超限仍报告相应 typed export failure，不把成功分析改写成数值 Top。UI 校验版本、节点 sort、依赖顺序和 pool refs 后安装结果；DAG 与 byte/slot 行按可见区渲染，exact payload 只显示有界预览。

MemoryPolicy 前沿在 UI 明确标记“策略下尚未展开”，与 EVM Failure、REVERT 分开。`ByteRange` 描述表示层错误，`StateJoin` 描述完整替代无法合并；正常 alias 的未定精度不能显示成已证明成功。

未知执行过程的闭合关系、摘要返回边与过程覆盖目录见[第 18 课](18-unknown-world.md)。关系表会显式进行数值投影，保留保守上界；这种投影与原有完整图摘要的身份重命名有不同精度边界。

## 验证边界与证据

三个 native provider 的 Array/UF 基础接口、原生 resource/cancellation 及 malformed input 边界由 `embedded-smt`、`bitwuzla-native` 的测试覆盖。基础接口通过不自动证明所有 EVM 路径通过；实际 opcode、上下文、rollback、summary、收敛与 revm 对照需要独立验收。

教程中的同址 memory/storage、有限别名、完整 word 往返、重叠复制与矛盾分支例子，各自核对一项具体关系。完全无约束的 memory 地址及 calldata 长度仍保留策略前沿，退出码为 2。循环 cases 是有界的保守抽象规则，不保证任意循环都达到固定点；历史读取和摘要重放仍须按实际图、输入与分析额度分别检查。库默认 1024 节点与 CLI 默认 16384 节点也不是同一种验证条件，不能由简单例子的通过推断全部默认策略已验证。

协议验收还须从真实 COPY/storage alias 分析经过 typed report、JSON 解析、前端引用校验与 SSA，并覆盖大共享图、range prestate、预算和取消。专项源码 SHA、fork、输入假设、provider/rlimit、节点/状态/求解成本、内存与耗时见 [Issue #111 集成验收](issue111-validation.md)；最终提交的完整 Nix 门禁结果另记于实施 PR。
