# 用公开 Trait 组合分析器

这份指南面向要接入自己分析组件的 Rust 使用者。这里的嵌入式领域语言（EDSL）由普通 Rust Trait、关联类型和组合类型构成：声明要读写的事实，选择事实的提供者，组合独立任务，再由同一个内核维护依赖和闭包。生产 EVM 解释器也是这个组合中的参与者。

先运行[公开 API 示例](../crates/evm-abstract/examples/cooperative.rs)，再沿本文查看各个类型的职责。已有的 `analyze_world` 等默认入口仍然运行同一个生产内核，使用空的外部扩展。

## 先运行一次真实反馈

在仓库根目录执行：

```bash
nix develop --no-update-lock-file --command cargo run --offline --locked \
  -p evm-abstract --example cooperative
```

Nix 提供项目固定的 Rust 和原生依赖；首次进入环境可能需要下载 Nix 输入。`--offline` 限制 Cargo 的网络访问。示例本身只分析内嵌字节码，不访问 RPC。

程序读取任意 calldata 的第一个字 `x`，计算 `g = ((x | 1) == 0)`，携带 `g` 跳入下一个块，再执行 `JUMPI`。按 256 位按位或的语义，`x | 1` 的最低位恒为 1，所以 `g` 恒为 0；这条证明不要求 calldata 取某个值。示例特意选用只保存有限常量集合的数值策略，让独立组件能提供额外精度。

期望输出为：

```text
both: Converged, impossible branch reached = false
without producer: Converged, impossible branch reached = true
without consumer: Converged, impossible branch reached = true
```

后两行停用了各自组件的有效贡献。`true` 表示粗分析保留了这条分支，具体执行仍然不会走它。三个运行的代码、世界与入口假设相同。这个对照验证了两个组件都参与实际 EVM 分析；运行过程中还检查消费者先读到 `Pending`、生产者被反馈变化重新唤醒。

示例分成四个小文件，全部只使用公开 API：

| 文件 | 负责什么 |
| --- | --- |
| [cooperative.rs](../crates/evm-abstract/examples/cooperative.rs) | 构造离线世界，组合组件，调用生产入口，检查移除对照 |
| [contracts.rs](../crates/evm-abstract/examples/cooperative/contracts.rs) | 客户端自己的 `ZeroGuards` 查询、合取策略、额外存储与具体错误类型 |
| [producer.rs](../crates/evm-abstract/examples/cooperative/producer.rs) | 读取生产输入的表达式，证明哪些活动栈槽恒为零，发布 `ZeroGuards` |
| [consumer.rs](../crates/evm-abstract/examples/cooperative/consumer.rs) | 读取 `ZeroGuards`，转成解释器真正消费的栈约束；不检查表达式 |

运行示例中的测试：

```bash
nix develop --no-update-lock-file --command cargo test --offline --locked \
  -p evm-abstract --example cooperative
```

测试执行相同的 `main` 和全部三个对照；仓库的所有 target 测试也会执行它。

## 内核、EVM 适配器和客户端各管什么

[`analysis-kernel`](../crates/analysis-kernel/README.md)不依赖 EVM 指令、机器状态或数值域。它管理任务、带版本的事实、读取依赖、发布、代际替换与共同闭包。事实值保存在显式类型的 `Channel<Q, P>` 中；语义合并由策略 `P` 实现。

[EVM 适配器](../crates/evm-abstract/src/analysis/cooperation.rs)把真实解释器的输入发布到内核，并将外部证明应用到执行视图。普通控制流、相对过程体、摘要更新后的继续执行与外部组件进入同一个生产调度器。客户端负责自己的事实含义、规则和成本，不需要复制 EVM 控制流。

只想理解一次读取如何引起再次执行，可以先看内核已有的[可编译 Rustdoc 例子](../crates/analysis-kernel/src/lib.rs)：任务在首次发布前读取，发布后依赖使它再次执行，直到整个内核关闭。EVM 示例在同一协议上增加了实际解释器与自己的事实族。

## 数值载体、精度策略和程序分析组件

下面三类选择互相独立：

| 选择 | 公开类型 | 在执行中起什么作用 |
| --- | --- | --- |
| 一个机器字怎样表示、计算和约束 | [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)，关联类型 `Element` 满足 [`WordElement`](../crates/evm-abstract/src/domain/element.rs) | 选择实际数值载体 `N = D::Element` 和数值语义 `D` |
| 哪些机器输入保留不同精度身份 | [`EnginePolicies<C, P>`](../crates/evm-abstract/src/analysis/engine/policy.rs)，`ContextPolicy`、`PartitionPolicy` | 普通路径和过程路径共同使用的上下文与可选分区策略 |
| 哪些独立组件交换事实 | [`Extension<A, S, F>`](../crates/evm-abstract/src/analysis/cooperation/composition.rs) | 选择分析组件、每代任务计划和额外事实存储 |

`D` 是冻结的策略，`N` 是每个值携带的数据。`ValueAlgebra<D>` 在数值语义外统一处理值身份、来源和表达式。`EvmValue<N>`、`EvmByteArray<N>`、`EvmStore<N>`、`EvmWorld<N>`、`EvmEntry<N>` 与返回的 `EvmWorldAnalysis<N>` 都保留实际 `N`。`AbstractValue`、`World`、`Entry` 等原有名字是默认 `NumericValue` 的具体别名。

自定义数值策略通过 `analyze_world_with` 使用；同时接入外部组件时使用 `analyze_world_cooperating`。后者仍独立接收 `&ValueAlgebra<D>` 和 `&EnginePolicies<C, P>`。策略可以借用调用者的数据，一般入口不要求它们是 `Copy`；执行沿同一个策略实例借用，不在每层构造新的默认策略。返回值是自定义载体的实际分析结果。原生报告的泛型入口见 [`render::world`](../crates/evm-abstract/src/render/world.rs)；已有教学/SSA 入口各自保留其具体类型边界。载体类型保持不变，还需要检查每项额外语义事实在表示边界的保留情况，相关证据与未决项见[验证记录](trait-cooperation-validation.md)。

数值内部也能静态组合组件：[`ProductAlgebra<M, R>`](../crates/evm-abstract/src/domain/cooperation/policy.rs)选择位与区间之间的独立 reduction，使用相同内核协议。它负责一次固定数值输入中的局部交换。程序层的 `Extension` 则与正在执行的 EVM 协作，两者可以分别选择。自定义 `D` 也可以使用自己的 `Element`，无需把所有运算转回默认数值类型。

## 独立组件只声明自己需要的能力

核心协议在 [`kernel::cooperation`](../crates/analysis-kernel/src/cooperation.rs) 和 [`kernel::capability`](../crates/analysis-kernel/src/capability.rs) 中，也通过 `evm_abstract::kernel` 公开：

| 协议 | 职责 |
| --- | --- |
| `Capability` | 定义自己的 `Key`、`Value`，以及键和值的保守工作报价 |
| `Read<Q, Route>` | 读取指定事实族与提供者，自动记录依赖，包括尚未有答案的读取 |
| `Accepts<Q, Route>` | 按该 channel 的发布策略提交值，返回 `Changed` 或 `Unchanged` |
| `Charges` | 在组件执行自己的语义工作前，向同一个账本预留工作量 |
| `Analyzer<Cx>` | 用自己的 `Task` 和 `Error` 实现一次组件求值 |
| `Reduction<Cx>` + `Registered<R>` | 明确登记一个满足固定输入收缩契约的规则，然后作为普通 Analyzer 参与调度 |

例如示例生产者的泛型约束声明它能读 `Inputs<NumericValue>`、读反馈、写 `ZeroGuards`、计费。消费者只需要读 `ZeroGuards` 和反馈、写反馈、计费。它们都没有获得另一个组件的内部表示或解释器调度权。

`Pair::new(consumer, producer)`保留两个组件自己的任务和错误类型，用 `Either` 路由。`PairedTasks::new(PerInput, PerInput)`为每个新的 EVM 输入代际各注册一个任务。`PerInput` 只决定注册时机；后续反复执行由真实读取依赖触发。增加组件时，继续组合相应的分析器与任务路由即可。

## 用 ExtraStores 接入自己的事实族

适配器的 `Ports<N, Tail>` 是一份静态提供者列表：

| 路由 | 事实族 | 发布语义 |
| --- | --- | --- |
| `InputRoute = Here` | `Inputs<N>` | 解释器初始化的只读输入；同一代不能覆盖 |
| `RefinementRoute = There<Here>` | `Refinements` | 已登记组件给出的栈保证，按合取收缩 |
| `There<There<Here>>` 起 | 客户端自己的事实族 | 由 `ExtraStores::Stores` 中每个 channel 的策略决定 |

示例定义 `ZeroGuards::Value = BTreeSet<usize>`，含义是“这些从栈底计数的活动栈槽恒等于零”。合取两个答案时取槽号集合的并集：已证明的槽更多，约束更强。空集表示一个已算出的、没有额外保证的答案。

`Stores::create` 使用适配器传入的同一个 `Kernel` 和 `Meter` 创建 `Cons<Channel<ZeroGuards, Descending<Conjoin>>, Nil>`。`Extension::with_stores` 把它接到两个内建端口之后。该工厂在每个新的 `Subject` 上重新创建存储。不要在工厂中启动另一次 `solve` 或预算，也不要把旧代已收缩的 channel 复制进去。

路由同时指定事实类型和提供者位置。同一事实族可以有多个显式提供者；客户端选择分别读哪些路由，或实现有明确语义的聚合器。系统不会按类型暗中选择第一个答案。缺少所需路由会使相应 Trait 约束不成立。

示例的核心装配是：

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

完整的 imports、具体错误类型和可执行检查都在[示例源码](../crates/evm-abstract/examples/cooperative.rs)。

## 全局 join 与固定输入的局部收缩

全局节点与过程摘要合并来自不同路径的可能执行。它们的 join 必须覆盖两个输入；必要的 widening 还要覆盖被加速的行为。这个不断扩大的输入是权威锚点。

外部 reduction 处理某一份固定前提下的输入。适配器从锚点建立不可变 `Input<N>`，外部组件给出对同一批具体执行成立的保证，解释器将保证应用到一份**分离的执行视图**，再运行真实 transfer。全局锚点始终保留对流入行为的上界。

示例中 `g` 的数值上界可能是 `{0,1}`；生产者通过表达式语义独立证明 `g=0`，消费者把这个保证交给 EVM。证明可以包含 EVM 数值载体尚未表达的信息，只要它覆盖同一作用域前提下的所有具体执行。最终保持的是 EVM 上界与外部保证共同组成的乘积覆盖。

同一事实的变化会唤醒读取它的任务；语义等价的重复发布返回 `Unchanged`。示例故意先运行消费者：它读到 `Pending` 后结束这次本地求值，生产者稍后发布自有事实，内核自动重新执行消费者。消费者发布栈约束后，解释器和读过反馈的生产者都再次得到调度。无需客户端维护第二份工作表。

## Scope、代际与摘要更新后的重新播种

`Subject` 由生产位置 `Point` 和内核颁发的 `Scope` 组成。`Point` 包含命名空间、World/Body 类别与索引；`Scope` 区分运行 epoch、上下文、输入代际和冻结策略。相同 PC、相同槽号、相同数值上界都不足以证明两个输入共享前提。查询与发布使用 `Scoped::new(subject.scope, key)`。

假设某节点最初只有输入 `x=1`，组件据此收缩了一个局部视图。后来另一条路径或过程摘要更新带来 `x=2`，新输入必须覆盖 `{1,2}`。继续把旧代的 `x=1` 保证与新输入相交会漏掉执行。

适配器的处理顺序是：建立新输入作用域，重新初始化输入与空反馈，重新创建额外存储和组件任务，再切换稳定的代际选择器并退役旧代。选择器变化唤醒依赖者；旧代任务和句柄不能重新激活。输入变化引起的收缩失效由此与全局 join 分开。

过程摘要的输出更新也可能扩大调用者的继续执行输入，并触发上述流程。当前公开端口读取的是 World 或 Body 的生产输入；摘要通过这些输入变化参与协作。带摘要增长、Body 反馈与自有事实重新播种的实际客户端见[递归反馈测试](../crates/evm-abstract/tests/cooperation/bridge/extra.rs)。

组件自己的长期字段可以保存计数和诊断。若保存语义事实缓存，必须正确区分和失效其完整前提身份；直接按 PC 复用旧 `ZeroGuards` 会破坏本例契约。初始事实/观察会话还有自己的快照和 revision 约束，见[代码与初始事实分层](18-layered-analysis.md)；这次 ad-hoc 协作调用不会凭空获得会话身份或其覆盖证明。

## Pending、未知和不可达分别是什么意思

| 表示 | 含义 | 可以据此删除执行吗 |
| --- | --- | --- |
| `Availability::Pending` | 所需查询尚未算完，读取留下待解决义务 | 不可以 |
| `Availability::Absent` | 此路由当前没有贡献，后来发布仍会唤醒读者 | 不可以 |
| 已就绪的 Top，例如 `StackFacts::none()` | 已提供一个覆盖所有值、没有额外约束的答案 | 不可以 |
| `StackFacts::Unreachable` | 已登记规则证明该输入代际下没有具体执行 | 只有证明满足本代契约时可以 |
| 配置、组件、作用域或资源错误 | 求值没有正常完成，保留具体错误/前沿 | 不可以 |

`WordObservation` 是受控的字保证词汇；具体字段和构造器见[源码](../crates/evm-abstract/src/domain/observation.rs)。将多个合法观察合取时，明确的数值矛盾可以证明不可达。数值 reduction 的可选精度限制、`ReductionStatus::Empty` 或非数值的来源冲突各有处理；`Empty` 不会自动变成路径不可达。适配器保留粗视图或返回相应的类型化错误。

## 共同闭包、可选精度和共享预算

`LocalClosure::Stable` 只描述一次组件求值。内核 `Closure::Closed` 还要求没有待运行、执行中、被中断、缺失读取、未封口作用域或显式语义义务。队列暂时为空并不足以宣称闭包。`LocalClosure::Limited` 可记录接受了保守粗答案的可选精度损失；它不能清除仍未表示的必需行为。

EVM 返回的详细 `Status::Converged` 与 `Status::Incomplete` 应连同诊断和 frontiers 阅读。普通共享 Work/States/Transfers 限制沿解释器支持的路径留下 `Incomplete` 和未完成证据；实际组件失败、错误作用域或未解决的协作义务可以返回 [`CooperationError<E>`](../crates/evm-abstract/src/analysis/cooperation/error.rs)。取消使用控制入口并返回 `Cancelled`，不返回一个冒充完成的报告。根终止覆盖附件也有独立前提，不能替代详细图的完成状态。

各方共享累计工作账本，包括解释执行、数值操作、输入快照、注册、依赖读取、事实复制/比较、发布和唤醒。`max_work` 是实现定义的逻辑工作上限；它不等于 EVM gas、墙钟时间或进程总内存上限。

客户端应在语义遍历、集合构造等工作之前调用 `Charges::charge`，并让 capability/policy 的报价覆盖自己的键、值和合并成本。报价本身应保守、无分配，不通过深拷贝数值来估价；饱和或拒绝必须传播。框架支付的调度成本不会自动覆盖任意客户端计算。示例展示了这种预付方式。

数值操作与观察导入还有自己的实际操作数/上下文报价协议，直接以当前 [`EvmNumericAlgebra`](../crates/evm-abstract/src/domain/algebra.rs)、[默认域实现](../crates/evm-abstract/src/domain/product.rs)和[工作预算](../crates/evm-abstract/src/resource.rs)的契约为准。自定义策略若使用受计费的 literal 构造路径，需要为 `literal_quote_work` 显式提供可信的构造与释放成本上界；其默认值为 `usize::MAX`，会在构造前保守拒绝。发布按单次更新原子安装；成功的先前事实和已支付工作可以在之后中断时保留，整个异构任务不承诺一起回滚。具体语义回滚仍由 EVM 适配器负责。

## 当前反馈端口和可信语义边界

当前 EVM 反馈词汇就是 [`StackFacts`](../crates/evm-abstract/src/analysis/cooperation/facts.rs)：对活动帧从栈底计数的槽发布 `WordObservation` 保证，或显式证明该代不可达。解释器通过实际 `D` 把它们用于分离输入视图，保留载体和元数据。槽越界、错误 scope、失效代际或无效事实返回类型化失败。

客户端可以在 `ExtraStores` 中定义自己的强类型事实，借助普通 Analyzer/Reduction 继续组合。要影响 EVM，必须由一个满足语义契约的组件把相关结论送到受支持的反馈端口。当前接口没有让每一种 EVM 效果都可替换，也没有通用事实撤销协议。保守控制流边仍按解释器规则保留，不能用外部证明任意擦除历史边。

Rust 检查类型、路由和借用，`Registered` 明确接受下降发布权限及其语义契约。组件作者仍需保证：事实对同一固定前提下的所有具体执行成立；join/widening 覆盖输入；局部合取保持共同具体含义；等价判定不会隐藏语义变化；报价覆盖实际工作。任意黑盒代码不会因为实现 Trait 就自动可靠。

本例只实现一条有明确按位语义证明的规则，验证普通 EVM 反馈与两个移除对照。更广的闭包、代际、中断、递归摘要和载体边界证据及仍待完成的集成验收，见[验证记录](trait-cooperation-validation.md)。示例通过不能替代根集成的完整检查。
