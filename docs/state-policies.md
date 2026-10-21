# 选择整机状态策略

`AnalysisEngine::new(&algebra, &policies, &mut extension)` 借用数值代数、`EnginePolicies` 和组件。其中
`SemanticPolicies` 分别决定状态的实际分组、完整输入的合并、何时加速，以及过程输入和输出的摘要表示。
这些策略在一次分析中保持固定，可以借用调用方的数据，无须实现 `Clone` 或 `Copy`。

```rust
use evm_abstract::{
    Address, Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, CoarseSummaries, EagerAcceleration,
        EnginePolicies, ExecutionConfig, FirstResumeStatus, ForgetJumpHistory,
        OutcomeKind, RecentDistinctJumps, SemanticPolicies, Status,
        control::Control, cooperation::NoExtension,
    },
    domain::{EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    world::{EvmAccount, EvmEntry, EvmWorld},
};

let address = Address::repeat_byte(0x11);
let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "state policy example");
world.insert(address, EvmAccount::from_hex(
    "602a5f5260205ff3", Fork::Osaka,
).unwrap()).unwrap();
let algebra = ValueAlgebra::new(ProductAlgebra::default());
let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus)
    .with_state(SemanticPolicies::new(
        ForgetJumpHistory, BoundedJoin, EagerAcceleration, CoarseSummaries,
    ));
let input = AnalysisInput::new(world, EvmEntry::new(address)).unwrap();
let mut extension = NoExtension;
let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
    .run(input, ExecutionConfig::default(), &Control::default()).unwrap();
assert_eq!(result.status(), Status::Converged);
assert!(result.outcomes().iter().any(|outcome| {
    outcome.kind == OutcomeKind::Return
        && outcome.data.read_word(
            &EvmValue::constant(0u64.into()), algebra.borrowed(),
        ).contains(42u64.into())
}));
```

默认组合为 `StructuralPartition`、`CorrelatedJoin`、`FeedbackAcceleration` 和
`ProjectedSummaries`。上例的另一种完整组合先清除跳转历史，再选择分组键；合并时采用有界整机表示；过程摘要进一步放宽字节、存储和别名保证。
两种组合都必须覆盖具体执行，精度可以不同。`NoAcceleration` 只请求覆盖性合并；有限预算不足以完成闭包时，结果保留 `Incomplete` 和对应 frontier。

自定义 `MachinePartition` 的关联类型 `Key` 就是执行器使用的实际键。
`prepare` 和 `prepare_relative` 同时返回这个键与覆盖原输入的准备结果。
例如，[公开客户端测试](../crates/evm-abstract/tests/state_policy/partition.rs)借用调用方的
`Program`，在指定 PC 读取真实栈操作数，按零与非零分组，并在分组前清除历史。
测试同时检查来自不同历史的值、内存和存储见证。

自定义摘要可以通过 `RelativeState::covering_join`、`coarsen`、`instantiate`
及 `ProcedureOutput::covering_join`、`coarsen` 组合已有操作。
[借用配置的摘要示例](../crates/evm-abstract/tests/state_policy/procedures.rs)改变实际发布的返回字节，并验证递归调用者仍覆盖 revm 见证。
`PolicyContext` 提供原环境、数值代数和共享工作账本；这些帮助方法负责计费和取消检查。
拒绝准备或合并会留下未完成义务，不能作为不可达证明。只有所有相关输入、输出和依赖稳定后，过程记录才可标记 `closed`。

发生类型化表示容量拒绝后，`RuntimeRetry` 站点把原输入与覆盖它的粗候选交给摘要策略。
默认策略在此选择有界准入；自定义策略可以返回自己的闭合覆盖结果或拒绝。
执行器检查结果符合持久重试表示后才物化并执行，后续增长继续经过同一站点。
策略错误、工作不足和取消均保留原义务。

执行器将已拥有的输入交给 `prepare_owned` / `prepare_relative_owned`。
默认结构分区直接转移载荷，只复制键的结构坐标；拒绝结果 `PartitionRefusal`
保留完整原输入。自定义策略只实现借用式方法也可运行，默认适配器负责分阶段准备。
自定义键默认不启用图证书重放；只有声明 `supports_graph_replay`，保证键在调用者重绑定和源重命名后仍合法，才会启用。
重放仍重新准备实际键，并在安装后的完整输入相等时才解除对应执行义务。

策略作者需要说明分区数量及收敛条件：固定程序与有界上下文下，键必须确定，且具有明确的有限范围；
不能每次调用都分配一个新身份。`context_depth` 只约束跳转历史，不替自定义键提供其他数量界。
同键输入的准备表示必须允许完整覆盖性准入；不兼容的代码、所有者、栈或续体应返回类型化拒绝。
覆盖性 join 不等于 widening；后者的调度与表示共同承担收敛责任。
有限预算可以在闭包前耗尽，此时必须保留 `Incomplete`，预算本身不构成闭包证明。

键的数量界与操作费用是两个约定。`PolicyKey::key_work` 为键的完整复制、释放和比较提供保守上界；比较两个键的费用由两个上界共同覆盖。
报价只读取已缓存或已准备的结构元数据，不能在报价时执行昂贵的 Clone/Ord。`QuotedKey` 可给调用方已有的键附加缓存报价，报价不参与键的语义相等。
执行索引缓存最大键费用与保留总量，用已固定工具链的 B-tree 高度界预付查找和插入，复制整个索引则使用缓存总量。
未知费用 `usize::MAX`、不足的预算或取消会在使用键前保留未完成义务。
旧的到达分类器也要通过 `classification_work` 声明其构造费用；默认未知费用会拒绝执行，内置状态分类有明确报价。
