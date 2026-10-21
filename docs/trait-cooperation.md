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

示例源码断言并打印以下对照：

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
        Registered::new(producer::Producer::new(true)),
    ),
    PairedTasks::new(PerInput, PerInput),
)
.with_stores(contracts::Stores);

let analysis = analyze_world_cooperating(
    world, entry, config, &algebra, &policies, &mut extension,
)?;
```

imports、错误类型和三个对照的完整代码在示例中。[contracts.rs](../crates/evm-abstract/examples/cooperative/contracts.rs)定义 `ZeroGuards::Value = BTreeSet<usize>`：每个元素是从活动栈底计数、已证明恒为零的槽。合取用集合并集，因为证明的槽越多，保证越强；空集是一个已就绪、没有额外保证的答案。本地工厂返回 `Cons<Channel<ZeroGuards, Descending<Conjoin>>, Nil>`，为该词汇选择明确的存储和合取策略。

## 三个独立的组合位置

| 选择 | 公开接口 | 所负责的语义 |
| --- | --- | --- |
| 一个机器字怎样表示和计算 | [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)、[`WordElement`](../crates/evm-abstract/src/domain/element.rs) | 实际载体 `N = D::Element`、数值运算、投影和局部规约 |
| 整个执行状态怎样分组、覆盖和加速 | [`EnginePolicies<C,P,S>`](../crates/evm-abstract/src/analysis/engine/policy.rs)、[`StatePolicy`](../crates/evm-abstract/src/analysis/engine/policy/semantic.rs) | 实际状态键、完整输入准入、相对输入及过程输出摘要 |
| 独立任务怎样交换事实 | [`Extension<A,P,F,G>`](../crates/evm-abstract/src/analysis/cooperation/composition.rs)、[`analysis-kernel`](../crates/analysis-kernel/src/lib.rs) | 任务计划、类型化 channel、跨程序点依赖与共同闭包 |

`ValueAlgebra<D>` 在数值策略外处理值身份、来源和表达式；`EvmValue<N>`、`EvmByteArray<N>`、`EvmStore<N>`、`EvmWorld<N>`、`EvmEntry<N>` 及 `EvmWorldAnalysis<N>` 保留实际载体。`AbstractValue`、`World` 等名字是默认 `NumericValue` 的具体别名。泛型原生报告入口见 [`render::world`](../crates/evm-abstract/src/render/world.rs)；跨表示边界时仍需检查具体事实是否被保留。

`analyze_world_with` 接收数值和状态策略；接入独立组件使用 `analyze_world_cooperating`。冻结策略按引用传入，一般入口不要求策略实现 `Clone` 或 `Copy`。配置中的 `DomainSpec` 必须与传入数值策略一致；默认入口使用同一生产内核的空扩展。

## 状态策略决定可执行的完整表示

`EnginePolicies<C,P,S=SemanticPolicies>` 的 `context` 选择有限跳转上下文，`partition` 提供到达分类，`state` 承担完整状态语义。`StatePolicy<D,P::Class>` 是以下四个协议的组合约束，具体定义见[语义接口](../crates/evm-abstract/src/analysis/engine/policy/semantic.rs)：

| 协议 | 必须成立的契约 |
| --- | --- |
| `MachinePartition` | 返回实际 `Key` 和覆盖原输入的 `Partitioned` / `PartitionedRelative`；等键的准备后输入必须可以合法合并 |
| `WholeStatePolicy` | 初始化并维护完整状态与覆盖元数据；准入结果覆盖已有及流入执行 |
| `AccelerationPolicy` | 决定何时请求 `Join` 或 `Widen`；执行该决定的语义运算仍须覆盖输入 |
| `ProcedureSummaryPolicy` | 对相对输入和终态输出实施覆盖性的准入，包括首个输出及运行时重试 |

普通节点与相对过程映射直接使用策略的 `S::Key`。冻结策略的键须确定、在有限上下文词汇下有有限基数，准备操作须幂等并覆盖输入。`MachineKey` 是结构形状及报告投影；自定义键可以改变分组，准备后的 payload 也必须成为实际保留和执行的输入。只删除键中的代码、owner、栈或 continuation 坐标，而没有支持这些执行形状的覆盖表示，会破坏契约；无法支持的合并应返回类型化拒绝并留下未完成义务。

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

`Pair` 用 `Either` 保留两侧各自的任务和错误类型。`PairedTasks` 组合相应注册计划；`PerInput` 为每个新输入代际播种一次任务，后续重跑由真实读取依赖决定。组件无需持有另一个组件的内部表示。

`Discover` 适合找出目前可处理的点、边或提供者。发现集合尚未完成时，不能据其缺项推出必然保证；如果结论确实依赖某个值，应使用 `Read` 建立必需读取。同一任务对同一事实的必需读取会升级此前的可选发现。已知值为空也不自动证明开放目录已经穷尽。

## 区分共享存储与代际存储

[`Ports<N,Tail,Global>`](../crates/evm-abstract/src/analysis/cooperation/ports.rs)是借用真实 channel 的静态路由视图：

| 路由 | 查询/存储 | 键的作用域与寿命 |
| --- | --- | --- |
| `InputRoute = Here` | `Inputs<N>` | `Subject.scope` 下的只读、单次初始化输入 |
| `RefinementRoute = There<Here>` | `Refinements` | 同一 scope 下合取的 `StackFacts` |
| `There<There<P>>` | 客户端 `ExtraStores` 的第 `P` 个提供者 | 每个输入代际重建的本地 tail |
| `SharedRoute<P>` | 客户端 `SharedStores` 的第 `P` 个提供者 | 整次分析只创建一次；跨点上升事实使用 `Input::universe()` |
| `PointsRoute`、`GenerationRoute`、`TopologyRoute` | 内建点目录、当前代际与控制流关系 | 分析级 universe 下的共享观察 |
| `CalleeRoute`、`ProcedureEntryRoute`、`ProcedureRoute` | 调用来源到过程、入口描述符、过程目录 | 分析级 universe 下的共享观察 |
| `SummaryRoute`、`SubscriptionRoute`、`ContinuationRoute` | 过程输出、订阅目录、调用者模板 | 分析级 universe 下的共享观察 |

[工厂接口](../crates/evm-abstract/src/analysis/cooperation/composition.rs)使用传入的同一个 `Kernel` 和 `Meter`。`with_stores(factory)` 将 `ExtraStores` 接到本地 tail；`with_shared_stores(factory)` 复用 `ExtraStores` 工厂装配分析级 `SharedStores`，并要求共享提供者支持 `ReclaimScope`。可同时选择两种存储。不要在工厂内创建另一份预算、启动另一次 `solve` 或复制旧代已收缩的保证。

共享内建 `Inputs` 与 `Refinements` 可以按 scope 查询外部点；查询外部 scope 不会切换当前组件所持有的本地 `ExtraStores`。跨点自定义事实应显式放入共享 channel。共享寿命也不赋予旧事实新的有效性：其作用域、发布顺序与来源前提仍须正确。

类型和路由共同确定提供者。同一事实族可以有多个提供者，客户端分别读取或实现有明确语义的聚合器；内核不会按类型暗中选择第一个答案。缺少所需路由会使 Trait 约束不成立。

## 跨程序点和过程摘要的读取

`Point` 标识运行内的命名空间、节点类别和索引；`Subject` 再加上内核颁发的 `Scope`，标识冻结假设下的一代输入。相同 PC 或相同数值上界不足以确定同一输入。全局查询使用稳定的 `input.universe()`，局部保证使用 `input.subject().scope`。

读取另一点的当前输入，应保存它的 `Point`，唤醒后重新调用 [`current_input`](../crates/evm-abstract/src/analysis/cooperation/query.rs)。这个函数依次跟踪 `Generations`、`ReadGeneration::current_generation` 和共享 `Inputs<N>`。旧 `Subject` 只指向历史代际。尚未注册的点、未播种或已失效的选择保留显式 `Pending`，不能拿旧 payload 补答。`Answer.version` 也不能单独充当有效性证书：提供者失效可以使答案变回 `Pending` 而不改变发布计数。

[跨点客户端](../crates/evm-abstract/tests/cooperation/global.rs)给出一个实际例子：来源点保留一个排除 `42` 的字，下一块计算 `g=(x==42)`，再把 `g` 带入执行 `JUMPI` 的块。提供者在共享 channel 发布来源字的覆盖性排除信息；消费者经 `Topology` 找到前驱，用 `current_input` 读取其当前输入并核对表达式身份，然后将 `g=0` 发布到本代反馈。测试用两种组件顺序及关闭反馈的对照检查真实 CFG。该证明还使用这段字节码的全部贡献来源，不能推广为“目前只发现一个前驱，所以其他前驱不存在”。

`Topology` 的发现集合向上增长；新转移即使没有扩大目标的数值输入，也需要发布并唤醒读者。贡献族撤回不等于删除已记录的历史控制流边。

过程导航从真实调用来源开始：`Callees` 给出 `ProcedureId`，`ProcedureEntries` 给出入口 `Point` 和 frame 描述，再通过 `current_input` 读取当前 payload。Procedure 的保留入口与 Body 准入后的输入有各自前提；后者不能替代前者的完整入口覆盖。接线见[导航接口](../crates/evm-abstract/src/analysis/cooperation/navigation.rs)和[过程发布适配](../crates/evm-abstract/src/analysis/transfer/procedure/cooperation.rs)。

[`Summaries<N>`](../crates/evm-abstract/src/analysis/cooperation/global.rs)提供真实返回数据、store、输出代际、观察 revision 与独立的 `closed` 标记；`Continuations<N>` 提供调用者、callee、模板 revision 及实际 stack/store。`Procedures` 和 `Subscriptions` 用于发现键，不需要猜测内部表索引。[过程观察客户端](../crates/evm-abstract/tests/cooperation/global/procedures.rs)展示完整的公开读取链。缺失的输出种类尚未被发现；开放目录或一次返回观察都不是完整性证明，必须区分 payload 增长与生产求解的完成状态。

## 全局覆盖、局部收缩与前提失效

全局节点和过程摘要合并来自不同路径的可能执行，join 与 widening 必须覆盖流入行为。这个上升输入是权威锚点。适配器从它建立不可变 `Input<N>`，将外部保证应用到**分离的执行视图**，再执行真实 transfer；局部反馈不改写全局锚点。

保证须对该 `Subject` 声明的全部到达执行成立。它可以包含数值载体尚未表达的信息，例如由按位语义证明的 `g=0`；需要覆盖的是数值上界和外部保证共同表示的执行。Rust 类型与 `Registered` 不会自动证明任意客户端规则满足这个条件。

设某代输入只有 `x=1`，后来全局 join 或摘要增长带来 `x=2`。新输入覆盖 `{1,2}`，旧代保证 `x=1` 不能继续与它相交。另一种情况更隐蔽：本地 payload 没变，但本地保证依赖的外部输入或提供者已扩大，也必须让依赖这份前提的局部代际失效。

内核将唤醒依赖与代际累计语义前提分开。必需读取取得外部值时接纳其来源 scope 及提供者有效性；之后某次任务不再读取它，也不能擦掉本代已使用的前提。来源增长先使旧保证不可消费，再由生产 owner 从权威输入重新播种，建立新 scope、初始反馈、额外存储和任务，切换稳定选择器并退役旧代。只因同一输入中的下降保证增强而重跑，不应凭空制造更大的输入 universe。实现见[内核前提](../crates/analysis-kernel/src/kernel/assumptions.rs)、[代际切换](../crates/analysis-kernel/src/kernel/generation.rs)与[EVM 输入适配](../crates/evm-abstract/src/analysis/engine/cooperation.rs)。

来源这次没有贡献，也需要证据。[`withdraw_generation`](../crates/analysis-kernel/src/kernel/generation.rs)只用于**完整来源求值**确认该贡献族当前没有贡献；部分执行、必需读取未解决、资源拒绝或取消都不能撤回。撤回一个贡献族不等于整个点不可达，也不能抹掉其他提供者的贡献。[EVM 生命周期适配](../crates/evm-abstract/src/analysis/engine/cooperation/lifecycle.rs)检查来源版本和本地闭包后才执行这项操作，保留选择器身份以便将来安全重新激活。

退役后还要释放本地任务、输入快照和 channel 中属于旧 scope 的值。共享 channel 通过 [`ReclaimScope`](../crates/analysis-kernel/src/reclamation.rs)只回收该 scope；分析级聚合值继续保留。`ExtraStores::retirement_work` 与 `Capability::drop_work` 必须覆盖释放的实际工作，不能因为克隆 `Arc` 很便宜就假定最后一个 owner 的销毁也便宜。客户端自行缓存语义事实时，同样需要完整前提身份与失效规则，不能只按 PC 复用。

## 未知、Pending、缺席与不可达

| 表示 | 含义 | 能据此排除执行吗 |
| --- | --- | --- |
| `Availability::Pending` | 查询尚未得到有效答案；必需 `Read` 留下义务 | 不能 |
| `Availability::Absent` | 此路由当前没有贡献；后续发布仍会唤醒 | 不能 |
| 已就绪的 Top，如 `StackFacts::none()` | 已提供覆盖所有值、没有额外约束的答案 | 不能 |
| 开放目录的空集或缺失成员 | 目前的发现结果，不保证已穷尽 | 不能 |
| `StackFacts::Unreachable` | 登记规则证明本代没有到达执行 | 证明满足本代全部前提时可以 |
| 配置、scope、组件或资源错误 | 所需求值未正常完成 | 不能 |

[`StackFacts`](../crates/evm-abstract/src/analysis/cooperation/facts.rs)是当前解释器消费的反馈词汇：活动栈槽的 [`WordObservation`](../crates/evm-abstract/src/domain/observation.rs) 或显式不可达。自定义事实若要影响 EVM，需要一个遵守契约的组件将结论送到这个端口；槽越界、无效事实与失效 scope 有类型化失败路径。

将同一代的合法数值保证合取后，明确数值矛盾可以排除该执行视图。但 [`ReductionStatus::Empty`](../crates/evm-abstract/src/domain/reduce.rs) 本身不会自动变成 EVM 路径不可达；来源冲突也不证明数值矛盾。具体处理见[反馈应用](../crates/evm-abstract/src/analysis/cooperation/refinement.rs)：区分已证明矛盾、保守精度损失和错误。

## 数值局部协作与工作报价

[`ProductAlgebra<M,R,W=ComposedWork>`](../crates/evm-abstract/src/domain/cooperation/policy.rs)在一次固定数值输入内组合位、区间与标量规约；它与程序层的 `Extension` 可以分别选择。替换规则必须保持这次输入的共同具体含义，不能沿用另一组规则的收敛或成本证明。

例如 `x∈[7,14]` 时，`ADD 1; AND 8` 恒得 `8`：区间先得到 `[8,15]`，再将恒定的位传给按位域，供下一条 `AND` 使用。[数值组合客户端](../crates/evm-abstract/tests/cooperation/product.rs)用实际返回值检查这条信息传递，并关闭该规约方向检查精度贡献。

默认 `ComposedWork` 要求所选规则实现 [`RuleWork`](../crates/evm-abstract/src/domain/cooperation/cost.rs)，覆盖一次执行的能力调用、发布策略及计算，再按允许的整个调度组合报价。`with_work` 可注入针对这对规则的 `ProductWork`，包括显式的 `BoundedWork`；有限工作额度并不承诺局部闭包，也不改变事实容量和允许的规则步数。

观察导入与闭包是不同工作。策略若提供 `refinement_import_work`，先支付导入，再按实际导入结果支付后续规约；新增 `MemberOf` 的集合可能比原输入大，原来的 atom 数不足以推导其闭包成本。未提供导入专用报价时，保留完整 `refinement_work` 的付款契约。具体接口见 [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)。

数值优化同样需要明确语义证书。`uniform_word_bytes(word)` 声明在**这一策略**下，32 个合法 `BYTE` 索引的结果等价，包含隐藏载体约束；可见形状相同或 `byte_quotes_uniform()` 报价相同都不能替代它。自定义策略默认拒绝这项证书，`ProductAlgebra` 也不会因选用了默认载体而自动继承更强的写入捷径。`pack_word_numeric` 则须等价于该策略约定的逐字节拼接，保留必需的自定义规约，并按 `pack_word_work` 先付费。

完整运算报价包含运算内部的查询；调用者之后独立查询返回载体的 singleton，仍需按实际 `singleton_work` 付费。不能从内建载体查询便宜推断自定义载体也便宜，也不能重复收取完整报价已经覆盖的内部查询。缺失的报价不代表免费；例如 `literal_quote_work` 默认以 `usize::MAX` 在构造前拒绝。

局部结果必须连同结束原因读取：精度轮数/事实容量限制保留安全的已完成结果，`WorkLimit`、`Cancelled`、`KernelFailure` 分别记录工作中断、取消与协议失败，不能都解释为“规则已稳定”。原生报告 schema 为 [11](../crates/evm-abstract/src/analysis.rs)，wire schema 为 [16](../crates/evm-abstract-protocol/src/lib.rs)，`DomainSpec` 的成本版本为 [6](../crates/evm-abstract/src/domain/spec.rs)；报告格式与逻辑工作单位各自有版本。

## 共同闭包与客户端责任

`LocalClosure::Stable` 只描述一次任务求值。内核 `Closure::Closed` 还要求没有必需的缺失读取、待执行或中断任务、未封口 scope 和显式语义义务；可选发现可以仍未回答。队列暂时为空、一个摘要已有输出或一次规约停止，都不足以宣称全局闭包。可选精度损失也不能清除未覆盖的必需行为。

各方共享累计工作账本：解释执行、输入快照、注册、读取、复制、比较、发布、唤醒和回收都计入其中。客户端要在遍历和构造集合前调用 `Charges::charge`，报价应保守、无分配并正确传播饱和/拒绝。单次发布原子安装；之后发生中断不承诺整个异构任务全部回滚。`max_work` 是实现定义的逻辑工作上限，不是 EVM gas、墙钟时间或进程总内存上限。

返回的详细 `Status::Incomplete` 应连同 diagnostics 和 frontiers 阅读；组件、作用域及协作协议错误见 [`CooperationError<E>`](../crates/evm-abstract/src/analysis/cooperation/error.rs)。取消沿控制入口返回取消错误。根终态覆盖附件有自己的前提，不能替代详细图的完成状态。客户端作者仍负责事实的作用域、全部贡献来源、join/widening 覆盖、等价判定和成本契约。

若要从内核开始实现一门完整解释器，可读[有限区间/奇偶客户端](../crates/analysis-kernel/tests/interpreter.rs)：它自行定义程序点、状态和终态，由一个 `solve` 调度分支与循环，穷尽具体状态图检查节点、边和返回值覆盖，并以移除反馈的对照检验精度贡献。此类具体见证帮助检查实现；任意新规则仍需自己的语义论证。集成检查流程见[本地门禁](local-ci.md)。
