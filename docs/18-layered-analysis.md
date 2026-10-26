# 18：分层分析——代码、初始事实与执行

假设账户 A 的代码读取 `storage[0]` 和 `storage[1]`，相加后返回。只有字节码时，我们已经知道它执行哪些指令，却还不知道两个槽的初始值。后来补入 `A[0] = 0`、`A[1] = 7`，就能分析更小的一组初始状态：代码相同，输入事实更具体，返回值也随之收窄。

这正是分层接口的职责：把可共享的代码、固定快照的观察和一次执行分别保存，再交给同一个 EVM 引擎处理。

## 三种对象分别保存什么

| 对象 | 当前实现保存的内容 | 例子 |
| --- | --- | --- |
| `CodeCatalog` | 与账户状态无关的已解码程序、代码身份和基本块元数据 | 这段代码需要两个槽读取，某个块只使用栈 |
| `StateFacts` | 一个固定初始快照上的精确观察及不可变版本 | A 的槽 0 已知为零，槽 1 尚未观察 |
| `AnalysisSession` | catalog、当前事实、入口、固定执行策略，以及跨轮累计预算和有限复用信息 | 从 A 进入，按这些事实建立机器并执行 |

`CodeCatalog::with_runtime` 返回新 catalog 和 `CodeId`，已有 catalog 保持有效。`CodeId` 包含 fork、`Runtime` / `Initcode` 模式和原始字节的 Keccak；同字节的 runtime 与 initcode 有不同身份。目录内程序必须使用同一个 fork，复用时还会比较实际程序内容。

这里的“编译”是生成基本块元数据：栈的最低输入高度、输出高度、可能读取的事实类别、可能写状态，以及是否只使用栈。`requirements()` 是静态需求的并集，也可能包括不可达块或出错指令之后的指令。它没有把最终返回值、调用效果或整个函数的执行结果预先算好；静态 `stack_only` 标记也只是后续复用检查的一项条件。

实现入口见 [`analysis/layers`](../crates/evm-abstract/src/analysis/layers.rs)，代码目录见 [`catalog.rs`](../crates/evm-abstract/src/analysis/layers/catalog.rs)。

## 没观察到，与观察到零，是两件事

`Observed<T>` 区分 `Unobserved` 和 `Known(T)`。新建 `StateFacts` 没有任何账户观察；把代码绑定到一个入口地址，也不会顺带建立它的余额、nonce、存在性或 storage 事实。

`Observation` 用不同变体表达代码、余额、nonce、存在性、单槽和完整性声明，对应的 `FactKey` 标识被限制的位置。代码观察又分为 `Runtime(CodeId)`、`Delegation(Address)` 和 `Empty`，因此代码内容、委托指针与空代码不会混在一起。

| 提供的 `Observation` | 能确定什么 |
| --- | --- |
| 没有提供槽观察 | 该槽保持未知 |
| `Storage { address: A, slot: 0, value: 0 }` | A 的槽 0 精确为零，其他槽仍未知 |
| `Storage { address: A, slot: 1, value: 7 }` | A 的槽 1 精确为 7 |
| `Code { address: A, code: CodeObservation::Empty }` | 已观察到空代码，存在性仍须单独判断 |
| `Balance` / `Nonce` | 对应字段的精确值，包括显式的零 |
| `Existence { address: A, exists: false }` | 明确不存在，因而代码为空、余额和 nonce 为零、所有槽为零 |
| `StorageComplete { address: A }` | storage 观察完整，未列出的槽为零；已列出的槽保留其值 |

`StorageComplete` 是调用者作出的完整性声明。只查了两个槽不能据此声明整个 storage 完整。类似地，RPC 返回空代码、零余额和零 nonce，当前采集器仍可能保留 `Existence::Unknown`，这些值本身不能证明账户不存在。

事实绑定同一个 `SnapshotId`。链上身份由完整 chain ID 和固定 block hash 构成；离线 `SnapshotId::offline` 每次创建都有独立身份，即使标签相同也不同。要继续细化同一个离线输入，应保留或 clone 原来的 ID。

`refine` 的含义是**限制同一快照允许的初始状态集合**：从“槽 0 可以是任何值”收窄为“槽 0 是零”。它不表示链上时间推进，也不表示给当前交易执行一次写入。已有精确观察不能改值；重复相同观察不增加版本。整批观察先检查再安装，任一矛盾都会拒绝整批，例如：

- 已知槽 0 为零，又声明槽 0 为 7。
- 已声明账户不存在，又补入非空代码、非零余额、nonce 或槽值。
- 已声明 storage 完整，又把此前未列出的已知零槽补成非零。

会话还检查入口代码与代码观察是否一致、引用的 runtime 是否在 catalog 中，以及新观察是否与原始 `EvmWorld<N>` 的抽象约束相容。通过 `AnalysisInput::new` 与 `AnalysisSession::new` 接入时，原有非单值范围、符号来源和 provenance 保留在不可变 base 中；精确观察与它们求交，不会先把它们抹成未知。具体逻辑见 [`facts.rs`](../crates/evm-abstract/src/analysis/layers/facts.rs) 和 [`session.rs`](../crates/evm-abstract/src/analysis/layers/session.rs)。

## 一个只依赖代码的 Rust 会话

下面使用库的默认组合域和 Osaka fork。程序 `5f54600154015f5260205ff3` 执行 `SLOAD(0)`、`SLOAD(1)`、`ADD`，再用 `MSTORE` / `RETURN` 返回一个 word。示例可放进已依赖本仓库 `evm-abstract` 库的 Rust 程序；所有类型均从公开接口导入。

```rust
use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, OutcomeKind,
        control::Control, cooperation::NoExtension,
        layers::{AnalysisSession, Observation, Observed},
    },
    bytecode::Program,
    domain::{EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    world::{Code, EvmAccount, EvmEntry, EvmWorld},
};

fn main() {
    let fork = Fork::Osaka;
    let address = Address::repeat_byte(0x11);
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let control = Control::default();
    let mut extension = NoExtension;
    let mut world = EvmWorld::<NumericValue>::new(fork, "two initial slots");
    let mut account = EvmAccount::unknown();
    account.code = Code::Runtime(
        Program::from_hex_with_fork("5f54600154015f5260205ff3", fork).unwrap(),
    );
    world.insert(address, account).unwrap();
    let input = AnalysisInput::new(world, EvmEntry::new(address)).unwrap();
    let snapshot = input.snapshot().clone();
    let engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let mut session = AnalysisSession::new(engine, input, ExecutionConfig::default()).unwrap();
    assert_eq!(session.facts().storage(address, U256::ZERO), Observed::Unobserved);

    let before = session.run(&control).unwrap();
    session.refine(&snapshot, [
        Observation::Storage { address, slot: U256::ZERO, value: U256::ZERO },
        Observation::Storage { address, slot: U256::from(1), value: U256::from(7) },
    ], &control).unwrap();
    let after = session.run(&control).unwrap();
    assert_ne!(before.revision(), after.revision());
    assert_eq!(session.facts().storage(address, U256::ZERO), Observed::Known(U256::ZERO));

    let returned = after.analysis().outcomes().iter()
        .find(|outcome| outcome.kind == OutcomeKind::Return).unwrap();
    let zero = EvmValue::constant(U256::ZERO);
    assert_eq!(returned.data.read_word(&zero, algebra.borrowed()).singleton(), Some(U256::from(7)));
}
```

第一次 `run(&control)` 按两个未知初始槽执行。补入的是两个不同槽的零与非零观察，第二次从入口重新执行，返回 word 收窄为 7。`before` 仍引用原来那份报告及其事实版本；补充事实不会修改它。`EvmEntry::new(address)` 固定入口目的地址，未提供的 caller、value、calldata 和区块字段继续使用符号输入。

符号身份用于标识输入和状态读取的来源，值未知时仍可保留；它不等于 `Observed::Known`。`StateFacts` 记录固定初始快照的精确观察，`revision()` 标识这些观察的版本；报告中的运行时写入、历史读取与回滚根则属于产生该报告的执行。补事实生成的新版本不会改绑旧历史。

## 新事实怎样影响执行与历史

`AnalysisSession::refine` 返回 `RefinementEffect`：没有新观察时是 `Unchanged`；尚未执行时是 `InitialFacts`；已经执行过则是 `RestartRequired`。后者记录旧执行使用的版本、新版本、加入的 `FactKey` 和受影响的图状态。连续补入多批事实时，失效范围累计到下一次执行，不会忘掉先前批次的影响。

新事实版本建立新的初始 `EvmWorld<N>`，再由同一引擎创建新的运行时 `EvmStore<N>`、帧和回滚检查点。举例说，程序先执行 `SSTORE(0, 9)`，然后等到槽 1 的初始观察：补入槽 1 后重新执行，槽 0 的运行时写入仍由这次 `SSTORE` 产生；若随后 `REVERT`，它恢复的是这次执行的入口状态。槽 1 的初始观察不会作为交易写入被撤销，旧报告里的历史读取也不会被改绑到新根。

观察中的 `address` 始终是对应事实的所有者。`DELEGATECALL` / `CALLCODE` 可以执行实现账户的代码，却读取调用方的 storage；补入实现账户的同号槽不能代替代理账户的槽。Persistent、transient、历史读取和 rollback 根继续遵守各自的 EVM 生命周期，分层接口没有合并这些身份。

`SessionAnalysis::dependencies()` 返回 `FactDependencies`。它记录可能读取的代码、余额、nonce、存在性、storage 和输入，也包含未观察到的事实、空代码或不存在等负观察，以及保守通配依赖。`FactDependency` 中的 `None` 表示通配，不表示地址或槽为零。目前块级 storage 索引通常保留 owner 并使用通配 slot；交互、摘要和终结状态还可以依赖 `AllInitialState`。普通循环，以及无环入口在精确保留状态、表达式、完整路径或 alternatives 达到精度策略边界后，经 `LoopBound` 整体抽象状态根时，也记录 `AllInitialState`，不再仅依赖原先有限的槽观察；该提升仍受完整投影、结构兼容和共享预算限制，不改变补事实后从入口重启的约定。`affected_states()` 从直接读取者沿实际图边传播到后继，包括调用与返回。

这份索引用于解释和计算失效范围。当前没有据此恢复任意“未受影响状态”的有状态继续执行，也不把旧 `EvmStore<N>` 载荷搬到新执行中。通用选择性续跑仍需要另外设计并验证其状态、历史和回滚契约。

### 当前确实可以复用的计算

会话保留有界的纯栈块 memo，fork、入口环境与执行策略固定。候选块必须与 catalog 中的实际 runtime `Program` 全等、只使用许可的栈与控制指令、输入栈均为常量，且输入关系为空；命中还要匹配完整帧 key 和完整 `EvmValue<N>` 栈，包括 provenance。录入须有后继，且不得含 outcomes、frontiers、诊断或完成调用；执行后还要检查除活动栈和控制位置外的载荷保持一致，输出栈仍为常量。读取初始状态、memory、调用和账户生命周期指令不属于这个复用范围。

查找与录入在构造完整签名前，都先支付廉价资格检查，通过后再独立预付当前 `EvmStore<N>` 的 touch 查询费用：只有 `touched_in_transaction(0x03) == Some(false)` 才可复用。`Some(true)` 或 `None` 都拒绝复用并走普通 transfer，避免漏掉 [`late_failure::at_block_entry`](../crates/evm-abstract/src/analysis/transfer/late_failure.rs) 的额外 `Failure` 分支。`Memo::apply` 不携带 outcomes，因此静态 `stack_only` 不足以允许复用，memo 仍依赖这个运行时 Store 条件。touch 标志来自这次执行的交易元数据，不是向 `StateFacts` 注入的初始链上观察。检查顺序与完整保护条件见 [`ExecutionReuse::{lookup, remember}` 和 `Memo::apply`](../crates/evm-abstract/src/analysis/engine/reuse.rs)。

memo 保存栈与控制增量，不保留原始 `EvmStore<N>`；应用时从**新执行的当前载荷**出发。其他 owner 的运行时写入、当前 `EvmStore<N>`、挂起帧和已保存的回滚检查点都取自新载荷。缓存数量、保留载荷与维护工作均有界，完整 key、比较、复制与录入检查继续计入工作账本。`SessionAnalysis::reuse()` 的 `hits`、`misses`、`entries` 描述这种复用；它们不证明完整请求已经更快。一次性 world 入口会关闭无法在后续版本使用的 memo。

这项局部保护及[对应回归](../crates/evm-abstract/src/analysis/engine/reuse/tests.rs) 不构成任意 gas / OOG 覆盖证明。[现有 Lean 模型](proofs/README.md#snapshot-truth-history-and-rollback) 未验证 Rust memo 或依赖失效算法，这项保护也不扩大已检查定理的范围。

## 普通执行与等待事实的模式

`run(&control)` 使用未知世界的保守执行方式。缺失槽值保持未知，未知交互可以交给现有交互模型继续；如果仍有无法覆盖的行为或预算边界，报告会留下 frontier。缺少观察本身不会自动变成零、空代码或账户不存在。

`run_with_demands(&control)` 则显式请求可具体定位的初始事实。它本身不做 I/O；当前 `SessionAnalysis::demands()` 返回去重后的 `BTreeSet<FactDemand>`，只有以下两种形状：

| demand | 表示什么 |
| --- | --- |
| `FactDemand::Code(address)` | 缺少这个具体账户的代码，也可能是尚未解析的 delegation 实现 |
| `FactDemand::Storage { address, slot }` | 缺少这个具体 owner、完整 256 位 key 的初始槽值 |

等待这些事实时，`MissingCode` / `MissingStorage` frontier 可以使状态为 `Incomplete`。调用者可取得可信观察，通过 `refine` 注入，再调用 `run_with_demands(&control)`。新增 runtime 字节先用 `supply_runtime(program)` 编入目录，再用 `Observation::Code` 绑定到相应地址。只补充代码字节不会自动声明某个账户拥有它。

无法完整枚举的调用目标或槽键继续走保守抽象，不会捏造一个具体 demand。`demands()` 只从 `MissingCode` / `MissingStorage` 提取事实请求；为空不能单独证明分析完成。例如，状态额度用尽仍会产生 `Budget(Limit::States)`，CREATE 无法具体化 initcode 仍可产生 `Creation(UnknownInitCode)`，两者都不会生成上述 demand。未知预编译输入通常使用保守摘要继续执行，不能仅凭输入未知推断存在 frontier。判断完成情况应同时查看 `analysis().status()` 与 `frontiers()`，具体生产路径见 [`Engine::successor`](../crates/evm-abstract/src/analysis/engine.rs)、[`create.rs`](../crates/evm-abstract/src/analysis/transfer/create.rs) 和 [`precompile.rs`](../crates/evm-abstract/src/analysis/transfer/precompile.rs)。

## CLI、单程序、world 与 RPC 走同一引擎

| 使用方式 | 实际路线 |
| --- | --- |
| `cfg` / `ssa` / `explain --hex` 或 `--file` | `AnalysisInput::from_program` → `AnalysisEngine::run` → 显式 `EvmAnalysis::from_execution`；投影保留原生执行和状态 ID 映射 |
| `analyze` / `explain --world` | `AnalysisInput::new` → `AnalysisEngine::run`；多轮补事实使用借用该引擎的 `AnalysisSession` |
| `analyze` / `explain --rpc` | `analyze_rpc` → 固定快照采集 → 分层会话执行、补事实、从入口重建执行 |
| RPC 加 `--no-rpc-discovery` | 只采集预选事实，再构造相同的原生输入并调用引擎 |

例如，在仓库根目录可以分别查看单程序与离线 world：

```bash
nix run . -- cfg --hex 5f54600154015f5260205ff3 --fork osaka --format json
nix run . -- analyze --world examples/worlds/proxy-storage.json \
  --evm.to 0x0000000000000000000000000000000000000101 --format json
```

已有可信节点时，可以使用 RPC 路线；`LAB_RPC_URL` 和 `LAB_TARGET` 需换成实际端点和入口：

```bash
nix run . -- analyze --rpc "$LAB_RPC_URL" --fork osaka \
  --evm.to "$LAB_TARGET" --format json
```

RPC 是精确初始观察的生产者。它在启动时解析区块选择并固定 chain ID、block hash；后续账户与 storage 批次继续使用该快照，并检查链和区块一致性。槽采集还核对固定高度对应的 hash。采集批次失败或身份不一致时，不安装半批事实，尝试过的请求仍计入累计额度。这里信任配置的 RPC 返回，固定快照与一致性检查不等于对返回值作密码学证明。具体采集接口见 [`world/rpc/session.rs`](../crates/evm-abstract/src/world/rpc/session.rs)。

## 预算、取消与结果的含义

会话按 `ExecutionConfig.root_terminal_coverage` 请求独立的[根终止覆盖](19-root-terminal-coverage.md)，默认 true。库自身仍使用 20000000 work，CLI/Web 默认使用 2000000000 work；需要仅做详细分析时显式关闭覆盖。记录绑定真实原始 snapshot/revision、storage source、环境、帧和域/执行策略；补事实的新 revision 需要新的权威与执行。显示摘要变宽不扩大记录的输入集合。准备与详细执行共用预算，附加记录不把当前 worklist、frontier 或部分 SSA 改写为闭合。

首次 `run` 或 `refine` 在传入 `Control` 下初始化会话保留的根账本，后续操作复用已消耗的前缀及其取消所有者，同时检查本次操作的 `Control`。`refine(snapshot, observations, &control)` 先支付对实际不可变数值载体的成员校验费用，全部观察通过后才原子发布事实与目录。冲突不能先变成可见事实、等下次执行才报错。状态分配和 transfer 计数也累计；补事实不会重置额度。RPC 发现循环同样向各轮传递共享账本，采集另外保留自己的累计请求限制。工作量是分析器的逻辑计费，不是 EVM gas 或毫秒数。

`AnalysisEngine::run` 与两种 session 运行方法都显式接收 `&Control`，由它携带观察者和协作取消信号。执行及返回缓存报告前会检查取消；账本还保留创建时捕获的取消信号。需要整段会话受同一控制时，应在同一 `Control::scope` 内创建并运行它。取消以类型化错误返回，资源额度或语义展开缺口则通过报告 frontier 表达。

当前接口的原生分析 JSON schema、Web wire schema 分别由[原生分析格式常量](../crates/evm-abstract/src/analysis.rs)与[Web 协议常量](../crates/evm-abstract-protocol/src/lib.rs)定义；域策略的结构版本与 `cost_version` 由 [`DomainSpec`](../crates/evm-abstract/src/domain/spec.rs)定义。数值与关系限制只由代数的 `spec()` 冻结，模型选择由应用记录。消费报告时分别核对报告、策略结构与费用口径。Cost5 曾在 Cost4 基础上记录类型化标量依赖及操作数克隆的共享计费；Cost6 更新共享 kernel 的逻辑工作模型，Cost7 改为借用式操作数与显式 opcode 工作报价。数值实现需要内部复制时，由其完整报价承担。跨费用版本的原始 work 不是同一单位，同样整数预算不代表相同分析工作量，不能直接比较或相除推导性能。冻结布局、规范化和物化仍由共享有界状态统一计费。报告新增 `WorkLimit`、`Cancelled`、`KernelFailure` reduction reason，分别保留共享工作耗尽、取消与 kernel 失败，不再用局部 `RoundLimit` 混称这些停止原因。

分层能力的直接价值是明确代码共享、初始知识与执行历史的边界，并提供同一快照的事实细化接口。是否减少真实合约的总耗时和内存，需要在相同输入、精度、预算与 frontier 覆盖下测量；接口本身不构成全部 EVM 行为的形式化证明。
