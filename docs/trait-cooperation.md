# 用公开 Trait 组合分析器

这份指南面向要接入数值域、状态策略或独立分析组件的 Rust 贡献者。组合由普通 Trait、关联类型和静态路由组成：组件声明自己的事实与读写能力，内核维护依赖、失效、调度和共同闭包；EVM 解释器也参与同一次求解。先看一个真实字节码例子，再区分这些接口各自承担的语义契约。

## 从字节码看到反馈

[公开示例](../crates/evm-abstract/examples/cooperative.rs)只分析内嵌字节码，不访问 RPC。在仓库根目录运行：

```bash
nix develop --no-update-lock-file --command cargo run --offline --locked \
  -p evm-abstract --example cooperative
```

字节码 `5f356001175f14600a565b600f57005bfe` 的控制流是：

```text
入口:  x = CALLDATALOAD(0); g = ((x | 1) == 0); JUMP 0x0a
0x0a: JUMPDEST; 以 g 为条件 JUMPI 0x0f; 否则 STOP
0x0f: JUMPDEST; INVALID
```

任意 256 位的 `x` 与 1 按位或后都不为零，因此 `g=0`。示例故意选择较粗的有限常量集合策略：[生产者](../crates/evm-abstract/examples/cooperative/producer.rs)检查输入表达式，发布客户端定义的 `ZeroGuards`；[消费者](../crates/evm-abstract/examples/cooperative/consumer.rs)只懂这份事实词汇，将它变成解释器消费的栈保证。消费者不需要重做生产者的表达式证明。

以下输出及后文的客户端测试结论均描述源码中的断言；本次文档核对未运行示例或测试。示例断言并打印：

```text
both: Converged, impossible branch reached = false
without producer: Converged, impossible branch reached = true
without consumer: Converged, impossible branch reached = true
```

这里的 `true` 表示抽象控制流保留了该分支。消费者先运行、读到 `Pending`，生产者发布后它被自动唤醒；栈反馈又唤醒真实解释器和读过反馈的生产者。两份组件没有另起求解循环。完整装配是：

```rust
let mut extension = Extension::with_tasks(
    Pair::new(
        Registered::new(consumer::Consumer::new(true)),
        Registered::new(producer::Producer::<FiniteConstantSet>::new(true)),
    ),
    PairedTasks::new(PerInput, PerInput),
)
.with_stores(contracts::Stores);

let input = AnalysisInput::new(world, entry)?;
let analysis = AnalysisEngine::new(&algebra, &policies, &mut extension)
    .run(input, config, &Control::default())?;
```

imports、错误类型和三个对照的完整代码在示例中。[contracts.rs](../crates/evm-abstract/examples/cooperative/contracts.rs)定义 `ZeroGuards::Value = BTreeSet<usize>`：每个元素是从活动栈底计数、已证明恒为零的槽。合取用集合并集，因为证明的槽越多，保证越强；空集是一个已就绪、没有额外保证的答案。本地工厂返回 `Cons<Channel<ZeroGuards, Descending<Conjoin>>, Nil>`，为该词汇选择明确的存储和合取策略。

## 三个独立的组合位置

| 选择 | 公开接口 | 所负责的语义 |
| --- | --- | --- |
| 一个机器字怎样表示和计算 | [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)、[`WordElement`](../crates/evm-abstract/src/domain/element.rs) | 实际载体 `N = D::Element`、数值运算、投影和局部规约 |
| 整个执行状态怎样分组、覆盖和加速 | [`EnginePolicies<C,P,S>`](../crates/evm-abstract/src/analysis/engine/policy.rs)、[`StatePolicy`](../crates/evm-abstract/src/analysis/engine/policy/semantic.rs) | 实际状态键、完整输入准入、相对输入及过程输出摘要 |
| 独立任务怎样交换事实 | [`Extension<A,P,F,G>`](../crates/evm-abstract/src/analysis/cooperation/composition.rs)、[`analysis-kernel`](../crates/analysis-kernel/src/lib.rs) | 任务计划、类型化 channel、跨程序点依赖与共同闭包 |

`ValueAlgebra<D>` 在数值策略外处理值身份、来源和表达式；`EvmValue<N>`、`EvmByteArray<N>`、`EvmStore<N>`、`EvmWorld<N>`、`EvmEntry<N>` 及 `EvmWorldAnalysis<N>` 保留实际载体。`ProductAlgebra` 使用 `NumericValue`，`FiniteSetAlgebra` 使用 `FiniteConstantSet`；泛型 helper 通过 `D::Element` 传递两者。原生 JSON 的 `.numeric` 保留实际载体，来源图的 `WordObservation` 和协议 `ValueInfo` 则是各自显式的观察边界。

[`AnalysisEngine`](../crates/evm-abstract/src/analysis.rs) 只有一个公开执行入口。先用 `AnalysisInput::new(world, entry)` 建立经过校验、带有真实 snapshot/revision 的不可变输入，再调用借用式引擎的 `run(input, config, &control)`。状态策略与组件任务计划是独立泛型参数；冻结策略按引用传入，无须 `Clone` 或 `Copy`。`NoExtension` 直接实现空组合，走同一引擎。数值与关系限制只来自 `algebra.spec()`，执行配置只保存执行预算。所有载体与组件都遵守相同的输入来源、取消和终止覆盖绑定协议。

需要单程序教学 CFG 时，显式调用 `EvmAnalysis::from_execution(native, &mut postprocess)`。投影持有完整原生执行，并用 `execution_state_id` 把局部状态映射回底层图；投影先按实际载体复制和结构工作预付费，不再次执行程序。应用将同一有限后处理账本传给后续 SSA 构建与验证，用 `MachineEdgeKind` 保留所有两端均被选中的原生边，以及 frontier 和指令进度；输出阶段的 Work/Cancelled 不会生成伪造的完整 SSA。

## 状态策略决定可执行的完整表示

`EnginePolicies<C,P,S=SemanticPolicies>` 的 `context` 选择有限跳转上下文，`partition` 提供到达分类，`state` 承担完整状态语义。`StatePolicy<D,P::Class>` 是以下四个协议的组合约束，具体定义见[语义接口](../crates/evm-abstract/src/analysis/engine/policy/semantic.rs)：

| 协议 | 必须成立的契约 |
| --- | --- |
| `MachinePartition` | 返回实际 `Key` 和覆盖原输入的 `Partitioned` / `PartitionedRelative`；等键的准备后输入必须可以合法合并 |
| `WholeStatePolicy` | 初始化并维护完整状态与覆盖元数据；准入结果覆盖已有及流入执行 |
| `AccelerationPolicy` | 决定何时请求 `Join` 或 `Widen`；执行该决定的语义运算仍须覆盖输入 |
| `ProcedureSummaryPolicy` | 对相对输入和终态输出实施覆盖性的准入，包括首个输出及运行时重试 |

普通节点与相对过程映射直接使用策略的 `S::Key`。冻结策略的键须确定、在有限上下文词汇下有有限基数，准备操作须幂等并覆盖输入。`MachineKey` 是结构形状及报告投影；自定义键可以改变分组，准备后的 payload 也必须成为实际保留和执行的输入。只删除键中的代码、owner、栈或 continuation 坐标，而没有支持这些执行形状的覆盖表示，会破坏契约；无法支持的合并应返回类型化拒绝并留下未完成义务。

有限种键不意味着键操作便宜。`MachinePartition::Key` 和 `PartitionPolicy::Class` 都要求实现 [`PolicyKey`](../crates/evm-abstract/src/analysis/engine/policy/key.rs)：`key_work` 要覆盖键的完整复制和释放，两个键的比较由双方报价之和覆盖。报价只读缓存或结构元数据，不能先运行待报价的 `Clone`、`Ord` 或语义查询。`QuotedKey<K>` 可为现有键附加可信缓存报价，报价本身不参与相等或排序。到达分类还有独立的 `PartitionPolicy::classification_work`，不能把分类构造费用藏在 payload 报价中。执行器内部的 `KeyIndex` 缓存键费用上界与保留总量，在查找、插入和复制前付款；这些缓存不改变实际 `S::Key` 的身份。

准入先通过 `prepare_owned` / `prepare_relative_owned` 准备拥有的输入，再用实际键查找节点、分阶段构造覆盖结果和元数据。预付安装、版本更新与唤醒工作后，将这次可执行输入和覆盖元数据随版本更新一起原子安装。首个输入同样需要初始化或摘要准入。`PartitionRefusal` 保留原输入；错误、费用拒绝或取消不能把尚未安装的执行义务变成已覆盖。`WholeStatePolicy::admit` 的 `None` 必须表示完整流入状态已被覆盖，且没有待发布的语义元数据变化；字段相同但覆盖元数据变化时，仍须发布以唤醒读者。`metadata_work` 负责元数据复制/比较的报价。这是单次准入的分阶段安装契约，不承诺整次分析回滚。

例如两条路径分别到达 `(栈值, memory[0])=(11,11)` 和 `(22,22)`。保留完整替代项能知道二者相等；逐字段合并可能还包含 `(11,22)`，使后续 `XOR` 无法保持恒零。[完整状态客户端](../crates/evm-abstract/tests/state_policy/join.rs)正是用真实 `MSTORE` / `MLOAD` / `XOR` 检查这种精度差异，同时用具体执行见证检查覆盖。状态策略因而影响实际值和效果。

以下装配使用客户端已有的公开构造器：

```rust
use evm_abstract::analysis::{
    ContextInsensitive, CorrelatedJoin, EnginePolicies, FeedbackAcceleration,
    MergeResumeStatus, ProjectedSummaries, SemanticPolicies, StructuralPartition,
};

let policies = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
    SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        ProjectedSummaries,
    ),
);
```

`Join` 是覆盖两个输入的运算，`Widen` 还允许为收敛丢弃精度。测试中的循环字节码 `5f5b600101600156` 不断执行 `x=x+1`：反馈加速得到覆盖后继的循环不变式；仅 join 的配置耗尽 transfer 预算时保留 `Incomplete`，不能暗中改用默认 widening。

同一策略也用于 `Procedure`、`Body`、`Subscriber`、`Output` 和 `RuntimeRetry` 边界。公开包装 `RelativeState<N>`、`ProcedureOutput<N>` 提供相对输入和摘要操作；错误与取消不能被当作输入已覆盖。覆盖元数据即使没有改变可执行字段，也必须使实际读者失效。借用调用者数据、使用自有键的例子见[分区客户端](../crates/evm-abstract/tests/state_policy/partition.rs)；递归、回滚和自定义输出策略见[过程客户端](../crates/evm-abstract/tests/state_policy/procedures.rs)，拒绝与取消见[中断客户端](../crates/evm-abstract/tests/state_policy/interruption.rs)。

## 任务只声明自己需要的能力

协议由内核的 [`capability`](../crates/analysis-kernel/src/capability.rs) 与 [`cooperation`](../crates/analysis-kernel/src/cooperation.rs) 定义，也通过 `evm_abstract::kernel` 公开：

| 接口 | 含义 |
| --- | --- |
| `Capability` | 客户端定义 `Key`、`Value` 及键、值、释放工作的保守报价 |
| `Read<Q,Route>` | 必需的受跟踪读取；缺失答案留下求解义务，取得值时登记其语义前提 |
| `Discover<Q,Route>` | 可选发现；保留唤醒与有效性检查，未回答本身不阻止闭包，不接纳固定代际语义前提 |
| `Accepts<Q,Route>` | 按选定 channel 的发布策略提交值，区分 `Changed` 与 `Unchanged` |
| `Charges` | 在客户端自己的语义工作前从同一账本预留工作量 |
| `Analyzer<Cx>` | 定义组件自己的 `Task`、`Error` 和一次求值 |
| `Reduction<Cx>`、`Registered<R>` | 显式接受固定输入下降规约的语义契约，作为普通任务调度 |

`Pair` 用 `Either` 保留两侧各自的任务和错误类型。`PairedTasks` 组合相应注册计划；`PerInput` 为每个新的可执行输入代际播种一次任务，后续重跑由真实读取依赖决定。只读过程入口不注册这些任务。组件无需持有另一个组件的内部表示。

`Discover` 适合找出目前可处理的点、边或提供者。发现集合尚未完成时，不能据其缺项推出必然保证；如果结论确实依赖某个值，应使用 `Read` 建立必需读取。同一任务对同一事实的必需读取会升级此前的可选发现。已知值为空也不自动证明开放目录已经穷尽。

channel 通常用 [`Ascending<JoinPolicy>` 或 `Descending<RefinePolicy>`](../crates/analysis-kernel/src/algebra.rs)声明发布顺序。直接实现 `PublicationPolicy` 时，`preserves_guarantees` 必须保证此前合法接纳的所有语义保证仍成立，其分类工作包含在 `work` 报价中。它只控制这次值更新的语义前提失效；版本、唤醒、写权限及来源/拓扑变化的独立失效规则仍然生效。

## 区分共享存储与代际存储

[`Ports<N,Tail,Global>`](../crates/evm-abstract/src/analysis/cooperation/ports.rs)是借用真实 channel 的静态路由视图：

| 路由 | 查询/存储 | 键的作用域与寿命 |
| --- | --- | --- |
| `InputRoute = Here` | `Inputs<N>` | `Subject.scope` 下的只读、单次初始化输入 |
| `RefinementRoute = There<Here>` | `Refinements` | 同一 scope 的 Native 活动栈保证；Process 拒绝非空栈反馈 |
| `ProcessRefinementRoute` | `ProcessRefinements` | 同一代际、可执行 Process Body 的类型化动作/效果保证 |
| `There<There<P>>` | 客户端 `ExtraStores` 的第 `P` 个提供者 | 每个输入代际重建的本地 tail |
| `SharedRoute<P>` | 客户端 `SharedStores` 的第 `P` 个提供者 | 整次分析只创建一次；跨点上升事实使用 `Input::universe()` |
| `PointsRoute`、`GenerationRoute`、`TopologyRoute` | 内建点目录、当前代际与控制流关系 | 分析级 universe 下的共享观察 |
| `CalleeRoute`、`ProcedureEntryRoute`、`ProcedureRoute` | 调用来源到过程、入口描述符、过程目录 | 分析级 universe 下的共享观察 |
| `SummaryRoute` | `Summaries<N>` 的增量 MAY 快照 | 分析级 universe；已发现输出，不证明完整性 |
| `SummaryClosureRoute` | `SummaryClosures` 的闭合证书 | 绑定 native 输出代际，受整表证明的来源有效性保护 |
| `SubscriptionRoute`、`ContinuationRoute` | 订阅目录、调用者模板 | 分析级 universe 下的共享观察 |

[工厂接口](../crates/evm-abstract/src/analysis/cooperation/composition.rs)使用传入的同一个 `Kernel` 和 `Meter`。`with_stores(factory)` 将 `ExtraStores` 接到本地 tail；`with_shared_stores(factory)` 复用 `ExtraStores` 工厂装配分析级 `SharedStores`，并要求共享提供者支持 `ReclaimScope`。可同时选择两种存储。不要在工厂内创建另一份预算、启动另一次 `solve` 或复制旧代已收缩的保证。

共享内建 `Inputs` 可以按 scope 查询外部点，可执行 World/Body 还可读取相应的 `Refinements`；查询外部 scope 不会切换当前组件所持有的本地 `ExtraStores`。跨点自定义事实应显式放入共享 channel。共享寿命也不赋予旧事实新的有效性：其作用域、发布顺序与来源前提仍须正确。

类型和路由共同确定提供者。同一事实族可以有多个提供者，客户端分别读取或实现有明确语义的聚合器；内核不会按类型暗中选择第一个答案。缺少所需路由会使 Trait 约束不成立。

## 跨程序点和过程摘要的读取

`Point` 标识运行内的命名空间、节点类别和索引；`Subject` 再加上内核颁发的 `Scope`，标识冻结假设下的一代输入。相同 PC 或相同数值上界不足以确定同一输入。全局查询使用稳定的 `input.universe()`，局部保证使用 `input.subject().scope`。

读取另一点的当前输入，应保存它的 `Point`，唤醒后重新调用 [`current_input`](../crates/evm-abstract/src/analysis/cooperation/query.rs)。这个函数依次跟踪 `Generations`、`ReadGeneration::current_generation` 和共享 `Inputs<N>`。旧 `Subject` 只指向历史代际。尚未注册的点、未播种或已失效的选择保留显式 `Pending`，不能拿旧 payload 补答。`Answer.version` 也不能单独充当有效性证书：提供者失效可以使答案变回 `Pending` 而不改变发布计数。

同一模块的 `discover_input` 仅把最初的点目录查询改成可选发现。找到生产者以后，代际有效性与实际输入仍是必需读取；它不能将一个失效的已知输入当作可用值，也不能靠“可选”标记证明路径不存在。

[跨点客户端](../crates/evm-abstract/tests/cooperation/global.rs)给出一个实际例子：来源点保留一个排除 `42` 的字，下一块计算 `g=(x==42)`，再把 `g` 带入执行 `JUMPI` 的块。提供者在共享 channel 发布来源字的覆盖性排除信息；消费者经 `Topology` 找到前驱，用 `current_input` 读取其当前输入并核对表达式身份，然后将 `g=0` 发布到本代反馈。测试用两种组件顺序及关闭反馈的对照检查真实 CFG。该证明还使用这段字节码的全部贡献来源，不能推广为“目前只发现一个前驱，所以其他前驱不存在”。

`Topology` 的发现集合向上增长；新转移即使没有扩大目标的数值输入，也需要发布并唤醒读者。贡献族撤回不等于删除已记录的历史控制流边。

过程导航从真实调用来源开始：`Callees` 给出 `ProcedureId`，`ProcedureEntries` 给出 `PointKind::ProcedureEntry` 和 frame 描述，再通过 `current_input` 读取实际保留的入口。这个输入跟踪 Procedure 自己的 `entry_source`、payload 和 revision；多个 caller 或多个 Body key 不会把首个 Body 变成入口。该点只读，不注册 `PerInput` 任务，不创建本地 `ExtraStores`，也不初始化可消费的 `Refinements`。入口可用只说明其保留状态有效，不证明整个过程已闭合。[导航接口](../crates/evm-abstract/src/analysis/cooperation/navigation.rs)和[多 caller 客户端](../crates/evm-abstract/tests/cooperation/global/entries.rs)展示这条读取链，后者还断言入口增长及同块的多个 Body key。

[`Summaries<N>`](../crates/evm-abstract/src/analysis/cooperation/global.rs)提供增量 MAY 快照：解释器当前已计算的抽象返回数据与 store 上界、native 输出 `generation` 和观察 `revision`。MAY 可以包含实际不会发生的行为，不是具体执行存在的见证。缺失的输出种类尚未被发现，不能据此排除执行。普通必需 `Read` 等待首个快照，后续增长唤醒读者，不接纳闭合证明的作用域。[过程观察客户端](../crates/evm-abstract/tests/cooperation/global/procedures.rs)分别以必需读取和可选发现观察 MAY，断言两种模式都能稳定。`Continuations<N>` 另提供调用者、callee、模板 revision 及实际 stack/store；`Procedures` 和 `Subscriptions` 用于发现键。

完整性由独立的 `SummaryClosures` 提供：以 `ProcedureId` 为键，值 `SummaryClosure { generation }` 绑定该过程的 native 输出代际。发布依据是同一 Kernel 内一个选中的整张相对执行表闭合证明，接口不承诺每个 callee 独立提前闭合。证明跟踪实际 Body 与贡献输入、native entry/output 及其他输入源槽，也跟踪来源成员变化；只有真正闭合后才发布证书。来源前提失效时，证书立即不可用并使受跟踪读取回到 `Pending`；相同代际的证书可以续证，不制造数值增长；只保存代际数值不保留证书有效性。它与只读入口输入的作用域分离，某个入口有效并不证明整表已闭合。

要据完整输出收缩，使用公开 [`closed_summary<N,Cx>`](../crates/evm-abstract/src/analysis/cooperation/query.rs)：它必需读取快照和证书，检查 native 代际一致，返回 `ClosedSummaryResult<N,E>` 中的 `Answer<Arc<Summary<N>>>`。就绪的完整空输出表示没有终态贡献，不自动证明入口不可达。底层查询失败保留为 `SummaryReadError::Capability`；两份已就绪值代际不同则返回 `GenerationMismatch`。`Pending` 保留给真正尚未解决的底层读取。[摘要有效期客户端](../crates/evm-abstract/tests/cooperation/global/summary_validity.rs)在 Body 重算前读取完整摘要，断言旧闭合证明不可继续使用，并设有未变提供者的对照。

若完整摘要只用于可选精度，`discover_closed_summary` 先发现证书；可用后才升级为快照与证书的必需读取，返回值遵守同一完整性契约。`Pending` 时要有独立成立的保守事实；不能从缺失证书推导保证。[反馈客户端](../crates/evm-abstract/tests/cooperation/global/deferred.rs)展示这种升级和独立依据。没有独立依据的必需闭合证明依赖环可以保持 `Pending`；必需读取未解决时，不能把本地结束当成共同 `Closed`。

## 全局覆盖、局部收缩与前提失效

全局节点和过程摘要合并来自不同路径的可能执行，join 与 widening 必须覆盖流入行为。这个上升输入是权威锚点。适配器从它建立不可变 `Input<N>`，将外部保证应用到**分离的执行视图**，再执行真实 transfer；局部反馈不改写全局锚点。

保证须对该 `Subject` 声明的全部到达执行成立。它可以包含数值载体尚未表达的信息，例如由按位语义证明的 `g=0`；需要覆盖的是数值上界和外部保证共同表示的执行。Rust 类型与 `Registered` 不会自动证明任意客户端规则满足这个条件。

设某代输入只有 `x=1`，后来全局 join 或摘要增长带来 `x=2`。新输入覆盖 `{1,2}`，旧代保证 `x=1` 不能继续与它相交。另一种情况更隐蔽：本地 payload 没变，但本地保证依赖的外部输入或提供者已扩大，也必须让依赖这份前提的局部代际失效。

内核将唤醒依赖与代际累计语义前提分开。必需读取取得外部值时接纳其来源 scope 及提供者有效性；之后某次任务不再读取它，也不能擦掉本代已使用的前提。来源增长先使旧保证不可消费，再由生产 owner 从权威输入重新播种，建立新 scope、初始反馈、额外存储和任务，切换稳定选择器并退役旧代。只因同一输入中的下降保证增强而重跑，不应凭空制造更大的输入 universe。实现见[内核前提](../crates/analysis-kernel/src/kernel/assumptions.rs)、[代际切换](../crates/analysis-kernel/src/kernel/generation.rs)与[EVM 输入适配](../crates/evm-abstract/src/analysis/engine/cooperation.rs)。

来源这次没有贡献，也需要证据。[`withdraw_generation`](../crates/analysis-kernel/src/kernel/generation.rs)只用于**完整来源求值**确认该贡献族当前没有贡献；部分执行、必需读取未解决、资源拒绝或取消都不能撤回。撤回一个贡献族不等于整个点不可达，也不能抹掉其他提供者的贡献。[EVM 生命周期适配](../crates/evm-abstract/src/analysis/engine/cooperation/lifecycle.rs)检查来源版本和本地闭包后才执行这项操作，保留选择器身份以便将来安全重新激活。

退役后还要释放本地任务、输入快照和 channel 中属于旧 scope 的值。共享 channel 通过 [`ReclaimScope`](../crates/analysis-kernel/src/reclaim.rs)只回收该 scope；分析级聚合值继续保留。回收逐 channel 原子；静态组合的后续提供者拒绝时，先前的回收仍保留，可安全重试同一退役 scope。`ExtraStores::retirement_work` 与 `Capability::drop_work` 必须覆盖释放的实际工作，不能因为克隆 `Arc` 很便宜就假定最后一个 owner 的销毁也便宜。客户端自行缓存语义事实时，同样需要完整前提身份与失效规则，不能只按 PC 复用。

## 未知、Pending、缺席与不可达

| 表示 | 含义 | 能据此排除执行吗 |
| --- | --- | --- |
| `Availability::Pending` | 查询尚未得到有效答案；必需 `Read` 留下义务 | 不能 |
| `Availability::Absent` | 此路由当前没有贡献；后续发布仍会唤醒 | 不能 |
| 已就绪的 Top，如 `StackFacts::none()` | 已提供覆盖所有值、没有额外约束的答案 | 不能 |
| 开放目录的空集或缺失成员 | 目前的发现结果，不保证已穷尽 | 不能 |
| `StackFacts::Unreachable` | 登记规则证明本代没有到达执行 | 证明满足本代全部前提时可以 |
| 配置、scope、组件或资源错误 | 所需求值未正常完成 | 不能 |

[`StackFacts`](../crates/evm-abstract/src/analysis/cooperation/facts.rs)是 Native 解释器消费的反馈词汇：活动栈槽的 [`WordObservation`](../crates/evm-abstract/src/domain/observation.rs) 或显式不可达。自定义事实若要影响 EVM，需要一个遵守契约的组件将结论送到这个端口；槽越界、无效事实与失效 scope 有类型化失败路径。

将同一代的合法数值保证合取后，明确数值矛盾可以排除该执行视图。但 [`ReductionStatus::Empty`](../crates/evm-abstract/src/domain/reduce.rs) 本身不会自动变成 EVM 路径不可达；来源冲突也不证明数值矛盾。具体处理见[反馈应用](../crates/evm-abstract/src/analysis/cooperation/refinement.rs)：区分已证明矛盾、保守精度损失和错误。

## Process 视图与 Hybrid 注册阶段

默认 `Extension::new(rule)` 声明 `InputViews::Native`。需要参与 Process 基线的组装显式调用 `.with_process_view()`，并通过 `Input::view()` 区分 `Native` 与 `Process`。Process 视图借用来源、owner、static 所限定的 `ProcessFrameKey` 控制身份、实际 logical ADDRESS/CALLER/CALLVALUE、Store 与可用的 Program。缺失的调用操作数保留 `None`，不能用适配器键中的地址替代。Program 是捕获代码目录，不证明所有块可达；Process 私有栈与内存不属于 Native 栈反馈的范围。

`ProcessRefinementRoute` 合取本代的 `ProcessFacts`：`excluded_actions` 排除已证明不可能的原始动作，`local_readonly` 保证这次控制输入没有本地状态写入。当前效果包络也承担动作相关的转账和生命周期效果，所以 `local_readonly` 只在所有剩余动作均被排除时接受；仍有动作时返回 `RefinementError::ProcessActionEffects`。把 Process 事实用于 Native 输入或非可执行 Body，或者把非空 `StackFacts` 用于 Process，都会明确失败。反馈只改变付费构造的局部转换能力，不收缩权威输入或 Store。

[独立生产者/消费者测试](../crates/evm-abstract/tests/process_cooperation.rs)包含一个有 `SSTORE` 的程序：`CALLVALUE=0` 时跳过写入。生产者核对确切来源与这次调用值，消费者读取其自定义 channel 后发布 `local_readonly`；实际 Process 输出因此保留原始存储值。关闭任一组件会失去这一保证。独立 revm 执行检查零值与非零值对照，属性测试改变原始存储值。这个结论覆盖该来源、控制输入和调用条件，不能从当前开放 CFG 的缺项推出。

只接受 Native 视图的组件可显式选择 `.native_only()`：Hybrid 基线使用自己的无扩展 kernel，Native detail 使用原来的可变分析器及重新创建的本地/共享 stores。两阶段仍使用同一个累计 work 账本；禁用 detail 或在注册前拒付所需准备工作时不会调用该扩展。默认 `PhaseScope::All` 会在每个阶段注册扩展，不支持 Process 的组装返回 `CooperationError::UnsupportedView`，不会自动跳过。直接 Process 运行仍要求 Process 视图声明。组件错误保留具体类型传出阶段边界。[阶段测试](../crates/evm-abstract/tests/process_cooperation/phases.rs)覆盖实际工厂次数、释放、错误与额度拒绝。

正向 `current` 证据同时核对实际 Native 输入/输出版本和外部收据：使用过的确切 `Subject`、owner 执行 epoch、当前代际选择、scope 有效性、客户端闭包与待处理 owner。输入增长或前提退役即使没有改变数值载荷，也使旧保证不能再次认证；新一次执行必须取得新的有效收据。该检查适用于区域 Body、Resume 投递和最终 World 来源认证。

World 来源认证还要检查该记录实际保留的 Body/Resume 收据。持久 Native scope 不自动继承可替换客户端 scope 的前提，因此只检查 World 自己的输入不足以发现子过程前提退役。区域 owner 每次重新求值会更新独立的内核版本槽；World 跟踪这些槽，即使数值输出不变，也会醒来保留新的区域证据。数值行仍可共享同一不可变分配，不能用这种共享证明当前性。旧版本模型保留发布时的输入和证据基础；新版本单独表示失效、等待或重新闭合。

World 重证明始终重新观察来源与区域证据。只有同一个 World `Subject` 的上一次数值投递已完整完成，而且根 procedure 的 namespace、entry/output 版本和 source epoch 都不变时，才保留已安装的后继收据，不重复向 caller 汇入同一输出。新的证据与诊断仍单独保留；输入 scope 或数值版本变化会走实际投递路径。这种去重不授予当前性，前提退役时仍必须由新收据重新证明。

## 数值局部协作与工作报价

[`ProductAlgebra<M,R,W>`](../crates/evm-abstract/src/domain/cooperation/policy.rs)在一次固定数值输入内组合位、区间与标量规约；它与程序层的 `Extension` 可以分别选择。`ProductAlgebra::from_spec(spec)` 装配内置规则和 `BuiltinWork`；`ProductAlgebra::new(spec, masks, bounds)` 选择替换规则并使用 `ComposedWork`。只有确切的内置组合可以使用经过核对的收敛、拼接和均匀字节定律。替换规则必须保持这次输入的共同具体含义，不能沿用另一组规则的收敛或成本证明。

例如 `x∈[7,14]` 时，`ADD 1; AND 8` 恒得 `8`：区间先得到 `[8,15]`，再将恒定的位传给按位域，供下一条 `AND` 使用。[数值组合客户端](../crates/evm-abstract/tests/cooperation/product.rs)用实际返回值检查这条信息传递，并关闭该规约方向检查精度贡献。

`ComposedWork` 要求所选规则实现 [`RuleWork`](../crates/evm-abstract/src/domain/cooperation/cost.rs)，覆盖一次执行的能力调用、发布策略及计算，再按允许的整个调度组合报价。`with_work` 可注入针对这对规则的 `ProductWork`，包括显式的 `BoundedWork`；有限工作额度并不承诺局部闭包，也不改变事实容量和允许的规则步数。

`EvmNumericAlgebra::apply_numeric(op, args)` 接收 `ExactSizeIterator<Item = &Element>`，`observe_byte_numeric` 使用同一借用协议。`operation_work(op, args)` 必须按实际 opcode 覆盖数值运算本身（含实现内部的复制和查询），且报价必须不小于返回元素的 `Element::work_size`；调用方可据此另行预留临时结果的释放费用。`EvmValue` 的公共元数据独立计费，共用的 `ValueAlgebra` 不强制复制数值操作数。报价不能通过先执行运算或构造结果取得；未知报价以 `usize::MAX` 拒绝。缺少可选能力时可以保守返回 Top。后续独立查询仍按下文的查询契约另行计费。

观察导入与闭包是不同工作。策略若提供 `refinement_import_work`，先支付导入，再按实际导入结果支付后续规约；新增 `MemberOf` 的集合可能比原输入大，原来的 atom 数不足以推导其闭包成本。未提供导入专用报价时，保留完整 `refinement_work` 的付款契约。具体接口见 [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)。

数值优化同样需要明确语义证书。`uniform_word_bytes(word)` 声明在**这一策略**下，32 个合法 `BYTE` 索引的结果等价，包含隐藏载体约束；可见形状相同或 `byte_quotes_uniform()` 报价相同都不能替代它。自定义策略默认拒绝这项证书，`ProductAlgebra` 也不会因选用了默认载体而自动继承更强的写入捷径。`pack_word_numeric` 则须等价于该策略约定的逐字节拼接，保留必需的自定义规约，受计费调用按 `pack_word_work` 先付费，缺少报价则走普通路径。

这些证书只声明数值语义。[`EvmValue`](../crates/evm-abstract/src/domain/value.rs) 将数值约束、provenance、身份和表达式分层保存：数值相同不能建立源身份，来源相同也不能建立值身份。内存字节的表达式与来源仍须按真实操作保留，不能用 uniform-byte 或拼接证书替代其来源证明。

例如，`MLOAD` 已付费的内容证明给出零时，可以保留字面值 `0`，来源仍是 `Memory`。[外部来源与回滚见证](../crates/evm-abstract/tests/issue13_review.rs)将实际 `MLOAD` 数值检查到完成的 `SSTORE` 和后续存储读取。算术运算已获得的单值证明也只针对这次输入下的当前结果，不能按 opcode 跨输入复用。

`WordElement` 的纯方法不自动计费。保留/遍历大小 `work_size` 与实际元素的复制报价 `clone_work` 是两个契约；自定义深复制可以更贵，不能普遍用保留大小代替它。未被完整运算报价覆盖的派生结果复制，须在构造后按实际结果自己的 `clone_work` 付款，或转移所有权避免复制。输入报价的倍数不能自动证明已覆盖新结果，标准观察也不是完整载体副本。

[`SharedCheckpoint`](../crates/snapshot-state/src/shared_checkpoint.rs) 的句柄克隆只共享已保存版本；恢复为可写 Store 时，独占版本可以移动，共享版本则需要实际 Store 复制。句柄复制费用不能代替这次完整载荷复制的报价。

完整运算报价包含运算内部的查询；前后独立查询 singleton，要先付实际 `singleton_work`，独立导出观察或做成员判断（`contains`）则要支付覆盖该查询的 `work_size`。复制和保留费用不预付之后的查询。

同一不可变载体的已获答案可以复用，构造或运算协议也可以传递针对当前结果的证书。值改变后需要新结果自己的证明或一次已付款的查询。查询前后都要检查取消，不能发布取消后取得的证明。

不能从内建载体查询便宜推断自定义载体也便宜，也不能重复收取完整报价已覆盖的内部查询。缺失的报价不代表免费；受计费路径中的 `literal_quote_work` 默认以 `usize::MAX` 在构造前拒绝。没有账本的纯库调用由调用者承担工作。

局部结果必须连同结束原因读取：精度轮数/事实容量限制保留安全的已完成结果，`WorkLimit`、`Cancelled`、`KernelFailure` 分别记录工作中断、取消与协议失败，不能都解释为“规则已稳定”。原生报告 schema、wire schema 与 `DomainSpec` 的成本版本分别见[原生分析格式常量](../crates/evm-abstract/src/analysis.rs)、[Web 协议常量](../crates/evm-abstract-protocol/src/lib.rs)和[域策略定义](../crates/evm-abstract/src/domain/spec.rs)；报告格式与逻辑工作单位各自有版本。

## 共同闭包与客户端责任

`LocalClosure::Stable` 只描述一次任务求值。内核 `Closure::Closed` 还要求没有必需的缺失读取、待执行或中断任务、未封口 scope 和显式语义义务；可选发现可以仍未回答。队列暂时为空、一个摘要已有输出或一次规约停止，都不足以宣称全局闭包。可选精度损失也不能清除未覆盖的必需行为。

各方共享累计工作账本：解释执行、输入快照、注册、读取、复制、比较、发布、唤醒和回收都计入其中。客户端要在遍历和构造集合前调用 `Charges::charge`，报价应保守、无分配并正确传播饱和/拒绝。单次发布原子安装；之后发生中断不承诺整个异构任务全部回滚。`max_work` 是实现定义的逻辑工作上限，不是 EVM gas、墙钟时间或进程总内存上限。

返回的详细 `Status::Incomplete` 应连同 diagnostics 和 frontiers 阅读；组件、作用域及协作协议错误见 [`CooperationError<E>`](../crates/evm-abstract/src/analysis/cooperation/error.rs)。取消沿控制入口返回取消错误。根终态覆盖附件有自己的前提，不能替代详细图的完成状态。客户端作者仍负责事实的作用域、全部贡献来源、join/widening 覆盖、等价判定和成本契约。

若要从内核开始实现一门完整解释器，可读[有限区间/奇偶客户端](../crates/analysis-kernel/tests/interpreter.rs)：它自行定义程序点、状态和终态，由一个 `solve` 调度分支与循环，穷尽具体状态图检查节点、边和返回值覆盖，并以移除反馈的对照检验精度贡献。此类具体见证帮助检查实现；任意新规则仍需自己的语义论证。集成检查流程见[本地门禁](local-ci.md)。
