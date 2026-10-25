//! 整个事务共用的工作表与固定点调度器。
//!
//! 本模块决定“哪个分析状态还要处理、后继信息怎样汇合、什么时候停止”。
//! 指令怎样改变栈、内存和账户状态由 `transfer` 负责；CALL/RETURN 改变的是
//! 同一机器载荷里的调用栈，并不会另起一套独立的分析器。
//!
//! 初读时按一轮普通传播的顺序阅读：
//!
//! ```text
//! run_world 初始化 S0
//!   -> Engine::run 取出需要处理的状态
//!   -> transfer::execute 用最新入口摘要执行一次块转换
//!   -> Engine::execution 收集执行证据和后继
//!   -> Engine::successor 按结构键查找节点、join 输入并决定是否重新排队
//!   -> 回到 run，直到没有有效待处理节点或出现停止边界
//! ```
//!
//! 再读摘要路径：`try_summary` 查找已认证的 callee（被调用方）子图，`replay`
//! 把子图接到当前 caller（调用方）下；它复用同一个状态表、图和工作预算，
//! 而不是只缓存一个返回值。
//!
//! 阅读时保持三条不变量：同一结构键的入口摘要只通过 join 扩大；已观察到的边
//! 只增加；任何没有完成的展开都留下 frontier，不能冒充已完成的空结果。
//! `worklist` 统一保存 FIFO 调度记录与当前仍需处理的节点。
//! 最终状态由是否存在 frontier 决定，仅队列排空不足以证明 `Converged`。

use super::{
    ConfigError, Diagnostic, Limit, Status, SummaryStats,
    machine::{
        EvmExecutionEvidence, EvmMachineFrontier, EvmMachineOutcome, EvmMachinePayload,
        EvmMachineState, EvmWorldAnalysis, ExecutionConfig, FrontierReason, MachineEdge,
        MachineEdgeKind, MachineError, OutcomeKind,
    },
    summary::{EvmCache, EvmCertificate, EvmPrepared, EvmReplay},
    transfer::{self, EvmCompletedCall, EvmSuccessor, WorkBudget},
    worklist::{
        MachineMeter, kernel_failure,
        runtime::{Runtime, TaskKind},
        transfer_stop,
    },
};

use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, symbolic::ExprId},
    world::{EvmEntry, EvmWorld},
};
use analysis_kernel::{Availability, Change, RunningJob, SlotMeta};
use std::{collections::BTreeSet, sync::Arc};

pub(in crate::analysis) mod cooperation;
pub(in crate::analysis) mod policy;
use super::cooperation::{Composition, CooperationError, InputTasks, LocalInput, Point, PointKind};
use cooperation::Cooperation;
#[cfg(test)]
use cooperation::Empty;
mod graph;
mod input;
mod installation;
mod reuse;
mod terminal;
pub use policy::ContextPolicy as EvmContextPolicy;
use policy::KeyIndex;
pub use policy::{
    Acceleration, AccelerationInput, AccelerationPolicy, BoundedJoin, CoarseSummaries,
    CorrelatedJoin, EagerAcceleration, FeedbackAcceleration, ForgetJumpHistory,
    KnownProcessSummaries, MachinePartition, NoAcceleration, PartitionRefusal, PartitionResult,
    Partitioned, PartitionedRelative, PolicyContext, PolicyKey, PolicySite, ProcedureOutput,
    ProcedureSummaryPolicy, ProjectedSummaries, QuotedKey, RegionalSummaries,
    RelativePartitionResult, RelativeState, SemanticPolicies, StatePolicy, StructuralPartition,
    WholeStateMetadata, WholeStatePolicy,
};
pub use policy::{
    Arrival, ContextInsensitive, ContextPolicy, EnginePolicies, FirstResumeStatus,
    MergeResumeStatus, PartitionPolicy, RecentDistinctJumps,
};
pub(super) use reuse::EvmExecutionReuse;
#[cfg(test)]
use reuse::ExecutionReuse;
pub use reuse::ReuseStats;
use terminal::boundary::Boundary as TerminalBoundary;

/// Allocations and transfers remain charged when RPC refinement discards a graph.
#[derive(Default)]
pub(super) struct Counters {
    pub states: usize,
    pub transfers: usize,
    observer: super::progress::Observer,
}

impl Counters {
    pub(super) fn with_observer(observer: &super::progress::Observer) -> Self {
        Self {
            observer: observer.clone(),
            ..Self::default()
        }
    }

    pub(super) fn set_observer(&mut self, observer: &super::progress::Observer) {
        self.observer = observer.clone();
    }

    pub(super) fn report(&self, work: usize) {
        self.observer.emit(super::progress::Event::Execution {
            states: self.states,
            transfers: self.transfers,
            work,
        });
    }
}

/// Immutable-fact sessions retain pure computation while every run creates a
/// fresh `EvmStore<D::Element>` and rollback namespace. Existing callers need no reuse state.
#[expect(
    clippy::too_many_arguments,
    reason = "original execution ledger and immutable session identity are separate inputs"
)]
pub(super) fn run_reusing<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
>(
    world: EvmWorld<D::Element>,
    mut entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    policies: &EnginePolicies<C, P, S>,
    budget: &mut WorkBudget,
    counters: &mut Counters,
    storage_reads: transfer::StorageReadPolicy,
    mut reuse: Option<&mut EvmExecutionReuse<D::Element>>,
    identity: (&super::layers::SnapshotId, super::layers::FactRevision),
    cooperation: Cooperation<D::Element, X, E>,
) -> Result<EvmWorldAnalysis<D::Element>, CooperationError<E>> {
    cooperation.validate_view(policies.state.runtime_execution())?;
    if let Some(reuse) = &mut reuse {
        reuse.begin();
        reuse
            .dependencies
            .add(0, super::layers::FactDependency::Code(Some(entry.address)));
    }
    counters.observer.phase(super::progress::Phase::Analyzing);
    world.apply_snapshot_environment(&mut entry.environment);
    entry
        .environment
        .validate_structure()
        .map_err(ConfigError::from)?;
    if let Some(observed) = entry.environment.to.as_concrete()
        && observed != entry.address
    {
        return Err(
            ConfigError::from(crate::world::environment::EnvironmentError::Destination {
                expected: entry.address,
                observed,
            })
            .into(),
        );
    }
    config.validate()?;
    domain.spec().validate().map_err(ConfigError::from)?;
    if config.max_work == 0 || config.max_call_depth == 0 || config.max_memory_bytes == 0 {
        return Err(ConfigError::Budget.into());
    }
    let mut result = EvmWorldAnalysis {
        world,
        entry,
        config,
        schema_version: super::SCHEMA_VERSION,
        domain_spec: domain.spec(),
        snapshot: identity.0.clone(),
        revision: identity.1,
        states: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        root_terminal_coverage: None,
        status: Status::Converged,
        transfers: counters.transfers,
        work: 0,
        rpc_acquisition: None,
        summary_stats: SummaryStats::default(),
        summaries: Vec::new(),
        procedure_summaries: Default::default(),
    };
    // Structural input admission performs no opaque query. Pay the actual
    // carrier's independent blob-count proof before numeric consistency checks.
    let validation_work = result
        .entry
        .environment
        .blob_hashes
        .length
        .singleton_work()
        .saturating_add(
            result
                .entry
                .environment
                .blob_hashes
                .hashes
                .len()
                .saturating_mul(4),
        );
    if validation_work == usize::MAX || !budget.charge(validation_work) {
        if budget.cancellation().is_cancelled()
            || analysis_control::current_cancellation().is_cancelled()
        {
            return Err(CooperationError::Cancelled);
        }
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: None,
            pc: None,
            reason: FrontierReason::Work,
            range: None,
            payload: None,
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    result
        .entry
        .environment
        .validate()
        .map_err(ConfigError::from)?;
    if budget.cancellation().is_cancelled()
        || analysis_control::current_cancellation().is_cancelled()
    {
        return Err(CooperationError::Cancelled);
    }
    // 输入 lift、EvmStore<D::Element> 与回滚点复制也使用根账本，不让初始化获得免费工作。
    if counters.states >= result.config.analysis.max_states {
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: None,
            pc: None,
            reason: FrontierReason::Budget(Limit::States),
            range: None,
            payload: None,
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    let initial_work = result
        .world
        .work_size()
        .saturating_add(result.entry.environment.work_size())
        .saturating_add(result.world.projection_work(domain))
        .saturating_add(result.entry.environment.projection_work(domain))
        .saturating_mul(4);
    // 饱和后已不能证明预留的是可信上界，即使用户给了 usize::MAX 预算。
    if initial_work == usize::MAX || !budget.charge(initial_work) {
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: None,
            pc: None,
            reason: MachineError::work_failure().frontier_reason(),
            range: None,
            payload: None,
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    let initial = match transfer::initial(&result.world, &result.entry, domain) {
        Ok(payload) => payload,
        Err(reason) => {
            result.frontiers.push(EvmMachineFrontier {
                from: None,
                target: None,
                pc: None,
                reason,
                range: None,
                payload: None,
            });
            result.status = Status::Incomplete;
            result.work = budget.used();
            return Ok(result);
        }
    };
    if !budget.charge(initial.work_size()) {
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: Some(initial.key()),
            pc: None,
            reason: MachineError::work_failure().frontier_reason(),
            range: None,
            payload: Some(Arc::new(initial)),
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    let classes = policy::classify(&policies.partition, Arrival::default(), budget)
        .and_then(|class| policy::clone_key(&class, budget).map(|prepared| (class, prepared)));
    let (initial_class, prepared_class) = match classes {
        Ok(classes) => classes,
        Err(error) => {
            result.frontiers.push(EvmMachineFrontier {
                from: None,
                target: Some(initial.key()),
                pc: None,
                reason: error.frontier_reason(),
                range: None,
                payload: Some(Arc::new(initial)),
            });
            result.status = Status::Incomplete;
            result.work = budget.used();
            return Ok(result);
        }
    };
    let prepared = policies.state.prepare_owned(
        initial,
        PolicySite::World,
        prepared_class,
        &mut PolicyContext {
            domain,
            environment: &result.entry,
            budget,
        },
    );
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(PartitionRefusal {
            input: initial,
            error,
        }) => {
            result.frontiers.push(EvmMachineFrontier {
                from: None,
                target: Some(initial.key()),
                pc: None,
                reason: error.frontier_reason(),
                range: None,
                payload: Some(Arc::new(initial)),
            });
            result.status = Status::Incomplete;
            result.work = budget.used();
            return Ok(result);
        }
    };
    let initial_key = prepared.key;
    let initial = prepared.payload;
    let alternatives = match policies.state.initialize(
        &initial,
        &mut PolicyContext {
            domain,
            environment: &result.entry,
            budget,
        },
    ) {
        Ok(metadata) => metadata,
        Err(error) => {
            result.frontiers.push(EvmMachineFrontier {
                from: None,
                target: Some(initial.key()),
                pc: None,
                reason: error.frontier_reason(),
                range: None,
                payload: Some(Arc::new(initial)),
            });
            result.status = Status::Incomplete;
            result.work = budget.used();
            return Ok(result);
        }
    };
    if let Some(reuse) = &mut reuse {
        reuse.record_dependencies(0, &initial);
    }
    if storage_reads == transfer::StorageReadPolicy::Discover
        && initial.active().code == super::FrameCode::Unknown
    {
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: Some(initial.key()),
            pc: Some(0),
            reason: FrontierReason::MissingCode(initial.active().key.code_address),
            range: None,
            payload: Some(Arc::new(initial)),
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    // This is the only minting point: initial() has actually entered the root,
    // all entry work is paid, and no mutable machine state has executed yet.
    // Failed optional admission consumes its paid discovery without poisoning
    // ordinary execution. A new fact revision always admits from a new initial.
    let ready_terminal = if result.config.root_terminal_coverage {
        if let Some(reuse) = &mut reuse {
            reuse
                .dependencies
                .add(0, super::layers::FactDependency::AllInitialState);
        }
        terminal::prepare(
            &initial,
            &result.world,
            &result.entry,
            &result.config,
            domain,
            Some(identity),
            budget,
        )
        .ok()
    } else {
        None
    };
    let terminal_boundary = TerminalBoundary::new(ready_terminal.is_some());
    // 本模型不精确跟踪剩余 gas，先保留入口即失败、EvmStore<D::Element> 不变的保守可能。
    // 这条 outcome 不是 transfer 已执行指令后报告的异常，也不是未完成前沿。
    let failure_store = if policies.state.runtime_execution() == super::RuntimeExecution::Process
        && result.entry.environment.to.as_concrete().is_none()
    {
        match initial
            .call_stack
            .root()
            .state
            .saved_store
            .state()
            .process_erase_namespace_with_payment(result.entry.address, domain, budget)
        {
            Ok(store) => store,
            Err(error) => {
                result.frontiers.push(EvmMachineFrontier {
                    from: None,
                    target: Some(initial.key()),
                    pc: Some(0),
                    reason: super::MachineError::from(error).frontier_reason(),
                    range: None,
                    payload: Some(Arc::new(initial)),
                });
                result.status = Status::Incomplete;
                result.work = budget.used();
                return Ok(result);
            }
        }
    } else {
        initial.call_stack.root().state.saved_store.state().clone()
    };
    result.outcomes.push(EvmMachineOutcome {
        state: 0,
        kind: OutcomeKind::Failure,
        data: crate::world::EvmByteArray::empty(),
        store: failure_store,
    });
    // S0 是状态表中的第一个节点。后续编号按发现顺序分配，不按基本块或 PC 排序。
    let key = initial.key();
    result.states.push(EvmMachineState {
        id: 0,
        key: key.clone(),
        entry: initial,
        exit: None,
        exit_stack: Vec::new(),
        executed_pcs: Vec::new(),
        execution_evidence: None,
        resume_status: None,
    });
    counters.states += 1;
    let cache =
        (result.config.use_summaries && !X::ENABLED && policies.state.supports_graph_replay())
            .then(EvmCache::new);
    let runtime = Runtime::new(budget).and_then(|mut runtime| {
        let scope = runtime.world;
        let input = runtime
            .kernel()
            .slot(scope, &mut MachineMeter(budget))
            .map_err(kernel_failure)?;
        runtime.register_world(0, result.states[0].entry.call_stack.depth(), budget)?;
        Ok((runtime, input))
    });
    let (runtime, input) = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            result.frontiers.push(EvmMachineFrontier {
                from: None,
                target: Some(result.states[0].key.clone()),
                pc: None,
                reason: error.frontier_reason(),
                range: None,
                payload: None,
            });
            result.status = Status::Incomplete;
            result.work = budget.used();
            return Ok(result);
        }
    };
    let mut ids = KeyIndex::new();
    if let Err(error) = ids.insert_with(initial_key, budget, |_| Ok(0)) {
        result.frontiers.push(EvmMachineFrontier {
            from: None,
            target: Some(result.states[0].key.clone()),
            pc: None,
            reason: error.frontier_reason(),
            range: None,
            payload: None,
        });
        result.status = Status::Incomplete;
        result.work = budget.used();
        return Ok(result);
    }
    let mut engine = EvmEngine {
        result,
        domain,
        policies,
        budget,
        counters,
        storage_reads,
        ids,
        partitions: vec![initial_class],
        updates: vec![0],
        feedback: vec![false],
        runtime,
        cooperation: Some(cooperation),
        inputs: vec![input],
        active: None,
        active_node: None,
        #[cfg(test)]
        test_driver: None,
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new()],
        source_records: vec![None],
        cache,
        alternatives: vec![alternatives],
        selectors: BTreeSet::new(),
        procedures: transfer::procedure::EvmTable::default(),
        reuse,
        terminal_boundary,
    };
    engine.run();
    if engine.result.status == Status::Incomplete
        && let Some(ready) = ready_terminal
        && let Some(trigger) = std::mem::take(&mut engine.terminal_boundary).into_trigger()
    {
        // A constant-size, already-funded move. In particular Work stays
        // exhausted and consumed work is neither charged nor reset here.
        engine.result.root_terminal_coverage = ready.publish(trigger);
    }
    engine.counters.report(engine.budget.used());
    if let Some(reuse) = &mut engine.reuse {
        reuse.dependencies.finish(&engine.result.edges);
    }
    if let Some(error) = engine
        .cooperation
        .as_mut()
        .and_then(Cooperation::take_failure)
    {
        return Err(error);
    }
    if X::ENABLED
        && !engine.runtime.closed()
        && engine.result.frontiers.is_empty()
        && !engine
            .cooperation
            .as_ref()
            .is_some_and(|cooperation| cooperation.interrupted)
    {
        return Err(CooperationError::Unfinished(
            engine.runtime.kernel().closure(),
        ));
    }
    Ok(engine.result)
}

/// 拥有一次世界分析的调度状态、执行证据和共享预算。
///
/// `result.states` 保存节点内容，`ids` 提供按结构键查找节点的索引；待处理队列
/// 只保存节点编号。分支、循环、嵌套调用和摘要导入都汇入这同一张固定点图。
struct EvmEngine<
    'a,
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> {
    /// 对外返回的分析结果；状态入口可继续 join，出口保存最近一次转换的证据。
    result: EvmWorldAnalysis<D::Element>,
    /// 所有普通转换、join 和摘要回放共用的有限值域及其容量规则。
    domain: AlgebraRef<'a, D>,
    policies: &'a EnginePolicies<C, P, S>,
    /// 入口复制、执行、join、摘要比较/认证/导入共用的累计工作账本。
    budget: &'a mut WorkBudget,
    /// Prior RPC refinement rounds also consume state and transfer capacity.
    counters: &'a mut Counters,
    /// 是否在 RPC refinement 中把缺失的初始槽观测记录为请求边界。
    storage_reads: transfer::StorageReadPolicy,
    /// 完整 payload 备选的覆盖证据；独立字段的数值 widening 不证明状态覆盖。
    alternatives: Vec<S::Metadata>,
    /// Only selectors allocated by a machine join, never source/observation IDs.
    selectors: BTreeSet<ExprId>,
    /// The semantic partition's actual identity, after its covering preparation.
    /// Public MachineKeys are report projections and need not be unique.
    ids: KeyIndex<S::Key>,
    partitions: Vec<P::Class>,
    updates: Vec<usize>,
    feedback: Vec<bool>,
    /// 待处理义务及 FIFO 记录；摘要回放满足节点后可保留失效记录。
    runtime: Runtime<<X as InputTasks>::Task>,
    cooperation: Option<Cooperation<D::Element, X, E>>,
    inputs: Vec<SlotMeta<EvmMachinePayload<D::Element>>>,
    active: Option<RunningJob>,
    active_node: Option<usize>,
    #[cfg(test)]
    test_driver: Option<analysis_kernel::JobId>,
    /// 已观察到的转移边，按结构排序去重；重访节点时不会删除旧边。
    edges: BTreeSet<MachineEdge>,
    /// 按状态、PC 和种类去重的程序诊断，与未完成分析的 frontier 分开保存。
    diagnostics: BTreeSet<Diagnostic>,
    /// 与 `result.states` 按编号对齐的子调用终结证据，供摘要认证使用。
    /// 保存 caller join 之前的原始终结和状态效果，不能从已汇合的返回节点逆推。
    completed_calls: Vec<Vec<EvmCompletedCall<D::Element>>>,
    /// Latest region observation produced by a successful World evaluation.
    /// All public flags stay false until one prepaid final certification pass.
    source_records: Vec<Option<usize>>,
    /// 可选的完整 callee 图证书缓存；关闭摘要时普通工作表路径仍然成立。
    cache: Option<EvmCache<D::Element, P::Class>>,
    /// Known/unknown procedure inputs, outputs and retained return continuations.
    procedures: transfer::procedure::EvmTable<D::Element, S::Key>,
    /// Session-owned pure deltas and conservative initial-read index.
    reuse: Option<&'a mut EvmExecutionReuse<D::Element>>,
    /// Original typed detailed boundary; never inferred from final JSON reasons.
    terminal_boundary: TerminalBoundary,
}

struct WorldWork<N: WordElement> {
    id: usize,
    payload: EvmMachinePayload<N>,
    /// Exact immutable foreign universe consumed before any nested execution.
    consumed: Option<super::cooperation::Subject>,
}

/// 当前节点尝试使用摘要后的调度决定，而非子调用的 Return/Revert/Failure。
enum SummaryAttempt {
    /// 不适用或没有完整证书；本轮仍需执行普通 transfer。
    Miss,
    /// 已导入认证子图并连接当前 caller；本轮无须再执行这个 callee 入口。
    Hit,
    /// 摘要工作没有完成，已经记录未完成前沿；必须停止，不能当作 Miss 回退。
    Interrupted(FrontierReason),
}

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmEngine<'_, D, C, P, S, X, E>
{
    #[cfg(test)]
    fn test_admission(&mut self) -> Result<(), MachineError> {
        if self.active.is_none()
            && let Some(driver) = self.test_driver
        {
            self.active = Some(
                self.runtime
                    .kernel()
                    .begin(driver, &mut MachineMeter(self.budget))
                    .map_err(kernel_failure)?
                    .suspend(),
            );
        }
        Ok(())
    }

    fn observe_terminal_boundary(&mut self, reason: &FrontierReason) {
        self.terminal_boundary.observe(
            reason,
            self.budget.exhausted(),
            self.counters.states >= self.result.config.analysis.max_states,
            self.counters.transfers >= self.result.config.analysis.max_transfers,
        );
    }
    /// 记录调度层无法继续完成的目标；此处没有具体指令 PC。
    ///
    /// 指令层边界由 `execution` 或 `replay` 携带 PC 记录。frontier 表示分析
    /// 不完整，与运行本身产生的失败 outcome 或程序诊断不是同一类事实。
    fn frontier(
        &mut self,
        from: Option<usize>,
        target: Option<super::MachineKey>,
        reason: FrontierReason,
    ) {
        self.observe_terminal_boundary(&reason);
        self.result.frontiers.push(EvmMachineFrontier {
            from,
            target,
            pc: None,
            reason,
            range: None,
            payload: None,
        });
    }

    fn frontier_payload(
        &mut self,
        from: Option<usize>,
        reason: FrontierReason,
        payload: EvmMachinePayload<D::Element>,
    ) {
        self.observe_terminal_boundary(&reason);
        self.result.frontiers.push(EvmMachineFrontier {
            from,
            target: Some(payload.key()),
            pc: None,
            reason,
            range: None,
            payload: Some(Arc::new(payload)),
        });
    }

    fn stop_queued(&mut self, reason: FrontierReason) {
        self.observe_terminal_boundary(&reason);
        let pending: Vec<_> = self.runtime.pending_world().collect();
        if pending.is_empty() && self.result.frontiers.is_empty() {
            self.frontier(None, None, reason.clone());
        }
        for id in pending {
            self.frontier(
                None,
                Some(self.result.states[id].key.clone()),
                reason.clone(),
            );
        }
    }

    /// 全局预算耗尽时，留下本轮节点和其余有效待处理节点的未完成证据。
    ///
    /// `id` 已经从 worklist 取出，必须单独纳入；否则结果会漏掉刚刚因预算
    /// 停止的节点。in_order 排除摘要回放满足后留下的失效队列项。
    fn stop_pending(&mut self, id: usize, reason: FrontierReason) {
        self.observe_terminal_boundary(&reason);
        let pending: Vec<_> = std::iter::once(id)
            .chain(self.runtime.pending_world())
            .collect();
        for pending in pending {
            self.frontier(
                None,
                Some(self.result.states[pending].key.clone()),
                reason.clone(),
            );
        }
    }

    /// A precise ordinary-transfer gap already represents this fixed-input
    /// evaluation. Finishing the kernel job still failed: retain its obligation
    /// and every queued rerun, including this node after a dependency/input wake.
    /// The receipt is local to this evaluation, never inferred from old reports
    /// or a relative Body/Resume frontier carrying the same outer source ID.
    fn stop_after_execution(&mut self, id: usize, reported_work: bool, reason: FrontierReason) {
        if reported_work && reason == FrontierReason::Work {
            self.stop_queued(reason);
        } else {
            self.stop_pending(id, reason);
        }
    }

    /// Finish deeper callees before propagating their callers' pending unions.
    /// A caller's immediate rejection and later return then meet before that
    /// caller executes. Same-depth nodes retain their existing FIFO order.
    fn next_input(&mut self) -> Result<Option<WorldWork<D::Element>>, MachineError> {
        loop {
            let Some((task, token)) = self.runtime.next(false, self.budget)? else {
                return Ok(None);
            };
            if matches!(&task.0, TaskKind::Extension(_) | TaskKind::Seed(_)) {
                let cooperation = self.cooperation.as_mut().ok_or(MachineError::Shape)?;
                if let Err(error) =
                    cooperation.evaluate(&mut self.runtime, task, token, self.budget)
                {
                    return Err(cooperation.stop(error, self.budget));
                }
                continue;
            }
            if matches!(
                &task.0,
                TaskKind::Body(_) | TaskKind::Resume(_) | TaskKind::SummaryObservation
            ) {
                let frontiers = self.procedures.resume_task(
                    task,
                    token,
                    &self.result.world,
                    &self.result.entry,
                    &self.result.config,
                    self.domain,
                    self.policies,
                    self.budget,
                    self.counters,
                    self.storage_reads,
                    &mut self.runtime,
                    self.cooperation.as_mut().ok_or(MachineError::Shape)?,
                );
                self.result.transfers = self.counters.transfers;
                let global_stop = frontiers.iter().find_map(|frontier| {
                    matches!(
                        &frontier.reason,
                        FrontierReason::Work
                            | FrontierReason::SummaryWork
                            | FrontierReason::Budget(_)
                            | FrontierReason::Relations(
                                crate::domain::relational::QueryReason::Cancelled
                            )
                    )
                    .then(|| frontier.reason.clone())
                });
                for frontier in frontiers {
                    self.observe_terminal_boundary(&frontier.reason);
                    self.result.frontiers.push(EvmMachineFrontier {
                        from: None,
                        target: frontier.target,
                        pc: Some(frontier.pc),
                        reason: frontier.reason,
                        range: frontier.range,
                        payload: frontier.payload,
                    });
                }
                if let Some(reason) = global_stop {
                    self.stop_queued(reason);
                    return Ok(None);
                }
                continue;
            }
            let TaskKind::World { node: id, .. } = &task.0 else {
                return Err(MachineError::Shape);
            };
            let id = *id;
            self.active = Some(token);
            self.active_node = Some(id);
            // A dependency wake may leave the input slot's version unchanged.
            // Even a failed read/preparation is a new evaluation, so neither an
            // older region record nor an older execution receipt can certify it.
            self.source_records[id] = None;
            self.result.states[id].invalidate_execution();
            if X::ENABLED {
                let cooperation = self.cooperation.as_mut().ok_or(MachineError::Shape)?;
                cooperation
                    .begin_owner(
                        &mut self.runtime,
                        self.active.as_mut().ok_or(MachineError::Shape)?,
                        self.budget,
                    )
                    .map_err(|error| cooperation.stop(error, self.budget))?;
            }
            #[cfg(test)]
            {
                tests::source_current::world_read_probe(id)?;
                if tests::source_current::attach_dependency(id) {
                    let scope = self.runtime.world;
                    let slot = self
                        .runtime
                        .kernel()
                        .slot(scope, &mut MachineMeter(self.budget))
                        .map_err(kernel_failure)?;
                    self.runtime.with_run(
                        self.active.as_mut().ok_or(MachineError::Shape)?,
                        self.budget,
                        |run| {
                            run.read_slot(&slot, &0usize, 1).map_err(kernel_failure)?;
                            Ok(())
                        },
                    )?;
                    tests::source_current::remember_dependency(slot);
                }
            }
            let state = &self.result.states[id];
            let answer = input::read(
                &mut self.runtime,
                self.active.as_mut().ok_or(MachineError::Shape)?,
                &self.inputs[id],
                &state.entry,
                self.budget,
            )?;
            let Availability::Value(input) = answer.state else {
                return Err(MachineError::Shape);
            };
            let cooperation = self.cooperation.as_mut().ok_or(MachineError::Shape)?;
            if !X::ENABLED {
                // No cooperative view is required: retain the original owned
                // transfer input without a box allocation or an extra charge.
                // Preserve the previous empty-limit reservation's cancel guard.
                if !self.budget.charge(0) {
                    return Err(MachineError::work_failure());
                }
                return Ok(Some(WorldWork {
                    id,
                    payload: input,
                    consumed: None,
                }));
            }
            let view = cooperation
                .input(
                    Point {
                        namespace: 0,
                        kind: PointKind::World,
                        index: id,
                    },
                    answer.version,
                    0,
                    None,
                    None,
                    input,
                    self.domain,
                    &mut self.runtime,
                    self.active.as_mut().ok_or(MachineError::Shape)?,
                    self.budget,
                    |run| {
                        let observed = run.observe_slot(&self.inputs[id])?;
                        if observed != answer.version {
                            return Err(analysis_kernel::KernelError::InvalidHandle);
                        }
                        Ok(())
                    },
                )
                .map_err(|error| cooperation.stop(error, self.budget))?;
            match view {
                LocalInput::Deferred => {
                    self.runtime
                        .finish(self.active.take().ok_or(MachineError::Shape)?, self.budget)?;
                    self.active_node = None;
                }
                LocalInput::Reachable { payload, limited } => {
                    let work = limited.len().saturating_mul(16);
                    if !self.budget.charge(work) {
                        return Err(MachineError::work_failure());
                    }
                    for status in limited {
                        self.diagnostics.insert(Diagnostic {
                            state: id,
                            pc: 0,
                            kind: super::DiagnosticKind::FactExchangeLimited(status),
                        });
                    }
                    return Ok(Some(WorldWork {
                        id,
                        payload: *payload,
                        consumed: cooperation.subject(Point {
                            namespace: 0,
                            kind: PointKind::World,
                            index: id,
                        }),
                    }));
                }
                LocalInput::Unreachable => {
                    if !self.budget.charge(self.result.states[id].entry.work_size()) {
                        return Err(MachineError::work_failure());
                    }
                    let state = &mut self.result.states[id];
                    state.exit = Some(state.entry.clone());
                    state.exit_stack = Vec::new();
                    state.executed_pcs = Vec::new();
                    state.execution_evidence =
                        Some(EvmExecutionEvidence::new(Vec::new(), Vec::new()));
                    cooperation
                        .complete_native(
                            &mut self.runtime,
                            self.active.as_mut().ok_or(MachineError::Shape)?,
                            self.budget,
                        )
                        .map_err(|error| cooperation.stop(error, self.budget))?;
                    self.runtime
                        .finish(self.active.take().ok_or(MachineError::Shape)?, self.budget)?;
                }
            }
        }
    }

    /// 反复处理入口摘要有新信息的节点，并在结束时整理对外结果。
    ///
    /// 普通执行与摘要复用最终都更新同一状态表。一次完成的 transfer 不代表
    /// 节点永远完成：后续 join 若扩大入口，`successor` 会让它重新参与传播。
    fn run(&mut self) {
        #[cfg(test)]
        if self.test_driver.is_some()
            && let Some(token) = self.active.take()
        {
            let _ = self.runtime.finish(token, self.budget);
        }
        loop {
            let WorldWork {
                id,
                payload: input,
                consumed,
            } = match self.next_input() {
                Ok(Some(work)) => work,
                Ok(None) => break,
                Err(error) => {
                    if let Some(id) = self.active_node.take() {
                        self.stop_pending(id, error.frontier_reason());
                    } else {
                        self.stop_queued(error.frontier_reason());
                    }
                    break;
                }
            };
            let frontiers_before = self.result.frontiers.len();
            let mut source_record = None;
            self.counters.report(self.budget.used());
            if let Some(reason) = transfer_stop(
                self.result.transfers,
                self.result.config.analysis.max_transfers,
                self.budget,
            ) {
                self.stop_pending(id, reason);
                break;
            }
            // 暂时移出 cache，使它与 &mut self 能同时用于传播和认证。
            // 每条继续/中断路径都放回同一缓存；take 不是清空或重置分析结果。
            let mut cache = self.cache.take();
            let entry = &self.result.states[id].entry;
            if let Some(reuse) = &mut self.reuse {
                reuse.record_dependencies(id, entry);
            }
            let relative_procedure = (self.policies.state.runtime_execution()
                == super::RuntimeExecution::Process)
                || entry.active().code == super::FrameCode::Unknown
                || transfer::procedure::is_recursive_entry(entry)
                || (transfer::procedure::is_known_call_entry(entry)
                    && self
                        .policies
                        .state
                        .summarize_known_call(&entry.active().key))
                || (transfer::procedure::is_known_root_entry(entry)
                    && self
                        .policies
                        .state
                        .summarize_known_root(&entry.active().key));
            if (relative_procedure
                || cache
                    .as_ref()
                    .is_some_and(|cache| cache.call_entries.contains(&id)))
                && let Some(reuse) = &mut self.reuse
            {
                reuse
                    .dependencies
                    .add(id, super::layers::FactDependency::AllInitialState);
            }
            let attempt = if !relative_procedure && let Some(cache) = &mut cache {
                self.try_summary(id, cache)
            } else {
                SummaryAttempt::Miss
            };
            if let SummaryAttempt::Interrupted(reason) = attempt {
                self.cache = cache;
                self.stop_queued(reason);
                break;
            }
            let reused = matches!(attempt, SummaryAttempt::Hit);
            let mut reported_work = false;
            if !reused {
                let execution = if relative_procedure {
                    let (execution, evidence) = self.procedures.execute_with(
                        id,
                        &self.result.world,
                        &self.result.entry,
                        input,
                        &self.result.config,
                        self.domain,
                        self.policies,
                        self.budget,
                        self.counters,
                        self.storage_reads,
                        &mut self.runtime,
                        self.active.as_mut().expect("World evaluation is suspended"),
                        self.cooperation
                            .as_mut()
                            .expect("composition is owned by this run"),
                    );
                    self.result.transfers = self.counters.transfers;
                    source_record = Some(self.result.procedure_summaries.len());
                    self.result.procedure_summaries.push(evidence);
                    execution
                } else {
                    self.result.transfers += 1;
                    self.counters.transfers += 1;
                    match self.ordinary_execution(id, input) {
                        Ok(execution) => {
                            // Only this ordinary World evaluation owns these
                            // instruction PCs. Relative frontiers and previous
                            // evaluations cannot satisfy its reporting duty.
                            reported_work = execution
                                .frontiers
                                .iter()
                                .any(|frontier| frontier.reason == FrontierReason::Work);
                            execution
                        }
                        Err(error) => {
                            self.cache = cache;
                            self.stop_pending(id, error.frontier_reason());
                            break;
                        }
                    }
                };
                self.execution(id, execution, cache.as_mut());
            }
            // 本轮传播后再检查候选子图是否闭合。候选 callee 入口若与登记时不同，
            // 或子图还有待处理节点/前沿，就不能发表完整可复用证书。
            // 返回 true 只表示本轮检查完成，仍可保留待闭合候选，并不保证新发表了证书。
            let published = cache.as_mut().is_none_or(|cache| {
                cache.publish_closed(
                    &mut self.result,
                    &self.edges,
                    &self.runtime,
                    &self.diagnostics,
                    &self.completed_calls,
                    &self.partitions,
                    self.budget,
                )
            });
            self.cache = cache;
            if X::ENABLED && self.result.frontiers.len() == frontiers_before {
                let cooperation = self.cooperation.as_mut().expect("composition owner");
                let completion = cooperation
                    .complete_native(
                        &mut self.runtime,
                        self.active.as_mut().expect("complete world evaluation"),
                        self.budget,
                    )
                    .and_then(|ready| {
                        if ready {
                            cooperation.record_source(
                                &mut self.runtime,
                                self.active.as_mut().expect("complete world evaluation"),
                                self.budget,
                                consumed,
                            )
                        } else {
                            Ok(())
                        }
                    });
                if let Err(error) = completion {
                    let error = cooperation.stop(error, self.budget);
                    self.stop_pending(id, error.frontier_reason());
                    break;
                }
            }
            if let Some(token) = self.active.take()
                && let Err(error) = self.runtime.finish(token, self.budget)
            {
                let finish_reason = error.frontier_reason();
                let reason = if matches!(
                    finish_reason,
                    FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
                ) {
                    finish_reason
                } else {
                    self.cache
                        .as_ref()
                        .and_then(|cache| cache.stop_reason())
                        .unwrap_or(finish_reason)
                };
                self.stop_after_execution(id, reported_work, reason);
                break;
            }
            self.active_node = None;
            if !published {
                let target = self
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.pending_target(&self.result));
                let reason = self
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.stop_reason())
                    .unwrap_or_else(|| super::summary::summary_boundary(self.budget));
                self.frontier(Some(id), target, reason.clone());
                self.stop_queued(reason);
                break;
            }
            if self.result.frontiers.len() == frontiers_before
                && self.result.states[id]
                    .execution_evidence()
                    .is_some_and(EvmExecutionEvidence::is_current)
            {
                self.source_records[id] = source_record;
            }
        }
        // 未闭合的候选只计为拒绝，不能在分析停止时补成“成功摘要”。
        if let Some(cache) = self.cache.take() {
            cache.finish(&mut self.result);
        }
        // 摘要中断也可能直接离开循环，因此用 frontier 判定完整性，不用队列长度。
        self.result.status = if self.result.frontiers.is_empty() && self.runtime.closed() {
            Status::Converged
        } else {
            Status::Incomplete
        };
        self.result.edges = std::mem::take(&mut self.edges).into_iter().collect();
        self.result.diagnostics = std::mem::take(&mut self.diagnostics).into_iter().collect();
        self.certify_procedure_sources();
        self.result.work = self.budget.used();
    }

    /// Positive report currency, separate from kernel/regional closure. Production
    /// runs start with fresh false flags; replay never changes them. If funding,
    /// unique ownership or foreign-source validity is unavailable, they stay false.
    fn certify_procedure_sources(&mut self) {
        #[cfg(test)]
        if tests::source_current::hold_certification() {
            return;
        }
        let records = self.result.procedure_summaries.len();
        if records == 0
            || !self.result.procedure_summaries.is_unique()
            || MachineError::check_cancelled().is_err()
        {
            return;
        }
        // Length quotes are O(1). pending_world scans native queue records and
        // deduplicates with a bounded tree; prepay its walk before constructing
        // either vector. This envelope includes publication and cancellation undo.
        let work = records
            .saturating_mul(4)
            .saturating_add(self.result.states.len().saturating_mul(12))
            .saturating_add(
                self.runtime
                    .work_size()
                    .saturating_mul((usize::BITS as usize).saturating_add(1))
                    .saturating_mul(4),
            )
            .saturating_add(8);
        let Ok(permit) = self.budget.try_completion_escrow(work) else {
            return;
        };
        let captured = self.budget.cancellation();
        let mut pending = vec![false; self.result.states.len()];
        for id in self.runtime.pending_world() {
            if let Some(pending) = pending.get_mut(id) {
                *pending = true;
            }
        }
        if let Some(id) = self.active_node {
            if let Some(pending) = pending.get_mut(id) {
                *pending = true;
            }
        } else if self.active.is_some() {
            // An unidentified in-flight owner cannot supply a World certificate.
            return;
        }
        let mut eligible = Vec::new();
        for id in 0..self.source_records.len() {
            if let Some(record) = self.current_source_record(id, &pending) {
                let valid = self.cooperation.as_ref().is_some_and(|cooperation| {
                    cooperation
                        .source_current(
                            &TaskKind::World { node: id, depth: 0 },
                            &self.runtime,
                            self.budget,
                        )
                        .unwrap_or(false)
                });
                if valid {
                    eligible.push(record);
                }
            }
        }
        if !permit.publish_allowed() || MachineError::check_cancelled().is_err() {
            return;
        }
        let Some(records) = self.result.procedure_summaries.unique_records() else {
            return;
        };
        for record in records.iter_mut() {
            record.source_current = false;
        }
        for &index in &eligible {
            records[index].source_current = true;
        }
        if captured.is_cancelled() || MachineError::check_cancelled().is_err() {
            // Already included in the escrow; never charge or deep-copy after a
            // late cancellation, and never publish a half-certified collection.
            for &index in &eligible {
                records[index].source_current = false;
            }
        }
    }

    /// Called only within the funded final metadata walk. Keeping the native
    /// pending predicate explicit also covers a popped but unbegun admission.
    fn current_source_record(&self, id: usize, pending: &[bool]) -> Option<usize> {
        if *pending.get(id)? {
            return None;
        }
        let record = (*self.source_records.get(id)?)?;
        (self
            .result
            .states
            .get(id)?
            .execution_evidence()?
            .is_current()
            && self.result.procedure_summaries().get(record)?.source_state == id)
            .then_some(record)
    }

    fn ordinary_execution(
        &mut self,
        id: usize,
        payload: EvmMachinePayload<D::Element>,
    ) -> Result<transfer::EvmExecution<D::Element>, MachineError> {
        let input = &self.result.states[id].entry;
        if let Some(reuse) = &mut self.reuse
            && let Some(execution) = reuse.lookup(input, self.budget)?
        {
            return Ok(execution);
        }
        let execution = transfer::execute(
            &self.result.world,
            &self.result.entry,
            payload,
            &self.result.config,
            self.domain,
            &self.policies.context,
            self.budget,
            self.storage_reads,
        );
        if let Some(reuse) = &mut self.reuse {
            reuse.remember(input, &execution, self.budget);
        }
        Ok(execution)
    }

    /// 只在普通子调用入口尝试完整图证书；CREATE initcode 不走此复用途径。
    ///
    /// 匹配需要当前 callee、世界、`EvmStore<D::Element>` 和分析策略的完整输入相等。未命中时
    /// 登记认证候选并继续普通执行；输入构造、查找或导入耗尽预算则留下前沿。
    fn try_summary(
        &mut self,
        id: usize,
        cache: &mut EvmCache<D::Element, P::Class>,
    ) -> SummaryAttempt {
        if !cache.call_entries.contains(&id)
            || !self.result.states[id]
                .entry
                .call_stack
                .active_child()
                .is_some_and(|frame| frame.continuation.creation.is_none())
        {
            return SummaryAttempt::Miss;
        }
        let input = match cache.input(&self.result, id, self.budget) {
            Ok(Some(input)) => input,
            Ok(None) => return SummaryAttempt::Miss,
            Err(reason) => {
                self.frontier(
                    Some(id),
                    Some(self.result.states[id].key.clone()),
                    reason.clone(),
                );
                return SummaryAttempt::Interrupted(reason);
            }
        };
        match cache.lookup(&input, &self.result, self.budget) {
            Ok(Some(index)) => {
                let certificate = cache.certificate(index);
                self.result.summary_stats.hits += 1;
                match self.replay(id, certificate) {
                    Ok(()) => {
                        self.result.summaries[certificate.record].reused_at.push(id);
                        SummaryAttempt::Hit
                    }
                    Err(reason) => SummaryAttempt::Interrupted(reason),
                }
            }
            Ok(None) => {
                self.result.summary_stats.misses += 1;
                if cache.begin(id, input, self.budget) {
                    SummaryAttempt::Miss
                } else {
                    let reason = super::summary::summary_boundary(self.budget);
                    self.frontier(
                        Some(id),
                        Some(self.result.states[id].key.clone()),
                        reason.clone(),
                    );
                    SummaryAttempt::Interrupted(reason)
                }
            }
            Err(()) => {
                let reason = super::summary::summary_boundary(self.budget);
                self.frontier(
                    Some(id),
                    Some(self.result.states[id].key.clone()),
                    reason.clone(),
                );
                SummaryAttempt::Interrupted(reason)
            }
        }
    }

    /// 把一次块转换的结果接入图：更新本节点出口，收集诊断、前沿和终结结果，
    /// 再把每个可继续传播的后继交给 `successor` 汇合与调度。
    ///
    /// `exit`、`executed_pcs` 和 `completed_calls[id]` 是本次转换的证据，重访会
    /// 替换它们；edges 和 diagnostics 则累计保留。CALL 后继还要登记为摘要入口。
    fn execution(
        &mut self,
        id: usize,
        mut execution: transfer::EvmExecution<D::Element>,
        mut cache: Option<&mut EvmCache<D::Element, P::Class>>,
    ) {
        self.terminal_boundary.retention(id, &execution);
        execution.payload.normalize();
        for completed in &mut execution.completed_calls {
            completed.payload.normalize();
        }
        self.result.states[id].exit_stack = execution.payload.active().stack.clone();
        self.result.states[id].executed_pcs = execution.executed_pcs;
        self.result.states[id].execution_evidence = Some(EvmExecutionEvidence::new(
            execution.instruction_progress,
            execution.instruction_values,
        ));
        self.result.states[id].exit = Some(execution.payload);
        self.completed_calls[id] = execution.completed_calls;
        for (pc, kind) in execution.diagnostics {
            self.diagnostics.insert(Diagnostic {
                state: id,
                pc,
                kind,
            });
        }
        for frontier in execution.frontiers {
            let super::transfer::EvmExecutionFrontier {
                pc,
                reason,
                target,
                range,
                payload,
            } = frontier;
            self.observe_terminal_boundary(&reason);
            self.result.frontiers.push(EvmMachineFrontier {
                from: Some(id),
                target,
                pc: Some(pc),
                reason,
                // RangeWitness already owns the conditional pre-effect input.
                payload,
                range,
            });
        }
        for outcome in execution.outcomes {
            self.result.outcomes.push(EvmMachineOutcome {
                state: id,
                kind: outcome.kind,
                data: outcome.data,
                store: outcome.store,
            });
        }
        for successor in execution.successors {
            let kind = successor.kind;
            if let Some(to) = self.successor(id, successor, FrontierReason::Work)
                && kind == MachineEdgeKind::Call
                && let Some(cache) = &mut cache
            {
                cache.call_entries.insert(to);
            }
        }
    }

    /// Preserve the owned arrival when admission fails. Only plain work refusal
    /// takes the caller's ordinary/replay label; cancellation keeps its cause.
    fn refuse_successor(
        &mut self,
        from: usize,
        payload: EvmMachinePayload<D::Element>,
        error: MachineError,
        work_reason: FrontierReason,
    ) -> Option<usize> {
        let reason = if error == MachineError::Work {
            work_reason
        } else {
            error.frontier_reason()
        };
        self.frontier_payload(Some(from), reason, payload);
        None
    }

    /// 将一个后继输入汇入按结构键区分的节点，并记录从 `from` 到该节点的边。
    ///
    /// 已有节点只通过 join 扩大入口；输入确实变化才重新排队。新键创建节点并
    /// 初次排队。返回 `None` 表示复制/join 或节点数量预算不足，已记录 frontier。
    /// `work_reason` 让普通传播与摘要导入分别报告 Work 和 SummaryWork。
    fn successor(
        &mut self,
        from: usize,
        mut successor: EvmSuccessor<D::Element>,
        work_reason: FrontierReason,
    ) -> Option<usize> {
        successor.payload.normalize();
        #[cfg(test)]
        if let Err(error) = self.test_admission() {
            self.frontier_payload(Some(from), error.frontier_reason(), successor.payload);
            return None;
        }
        let resume_status = self.call_resume_status(from, &successor);
        let classes = policy::classify(
            &self.policies.partition,
            Arrival { resume_status },
            self.budget,
        )
        .and_then(|class| policy::clone_key(&class, self.budget).map(|prepared| (class, prepared)));
        let (class, prepared_class) = match classes {
            Ok(classes) => classes,
            Err(error) => {
                return self.refuse_successor(from, successor.payload, error, work_reason);
            }
        };
        let prepared = self.policies.state.prepare_owned(
            successor.payload,
            PolicySite::World,
            prepared_class,
            &mut PolicyContext {
                domain: self.domain,
                environment: &self.result.entry,
                budget: self.budget,
            },
        );
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(PartitionRefusal { input, error }) => {
                return self.refuse_successor(from, input, error, work_reason);
            }
        };
        let entry_key = prepared.key;
        successor.payload = prepared.payload;
        let existing = match self.ids.get(&entry_key, self.budget) {
            Ok(existing) => existing,
            Err(error) => {
                return self.refuse_successor(from, successor.payload, error, work_reason);
            }
        };
        let to = if let Some(existing) = existing {
            // Feedback is monotone for the retained graph. Inputs without it
            // still need to discover cycles formed by newly inserted edges.
            let cyclic = if self.feedback[existing] {
                false
            } else {
                match self.joins_cycle(existing, from) {
                    Ok(cyclic) => cyclic,
                    Err(error) => {
                        return self.refuse_successor(from, successor.payload, error, work_reason);
                    }
                }
            };
            if cyclic && let Err(error) = self.mark_feedback(existing) {
                // Feedback publishes its own scheduling premise, independently
                // of whether this arrival changes the numerical input.
                self.frontier_payload(Some(from), error.frontier_reason(), successor.payload);
                return None;
            }
            let old_alternatives_work = self
                .policies
                .state
                .metadata_work(&self.alternatives[existing]);
            // Admission clones its metadata; a changed installation alone
            // copies the incoming payload after reserving JoinWork below.
            if !self.budget.charge(old_alternatives_work) {
                self.frontier_payload(Some(from), work_reason, successor.payload);
                return None;
            }
            if let Err(error) =
                self.admit_existing(existing, &successor.payload, old_alternatives_work)
            {
                return self.refuse_successor(from, successor.payload, error, work_reason);
            }
            existing
        } else {
            if self.counters.states >= self.result.config.analysis.max_states {
                self.frontier_payload(
                    Some(from),
                    FrontierReason::Budget(Limit::States),
                    successor.payload,
                );
                return None;
            }
            let cost = successor.payload.work_size();
            if cost == usize::MAX || !self.budget.charge(cost) {
                let reason = MachineError::check_cancelled()
                    .err()
                    .map_or(work_reason, MachineError::frontier_reason);
                self.frontier_payload(Some(from), reason, successor.payload);
                return None;
            }
            let alternatives = match self.policies.state.initialize(
                &successor.payload,
                &mut PolicyContext {
                    domain: self.domain,
                    environment: &self.result.entry,
                    budget: self.budget,
                },
            ) {
                Ok(metadata) => metadata,
                Err(error) => {
                    return self.refuse_successor(from, successor.payload, error, work_reason);
                }
            };
            let key = successor.payload.key();
            // Reserve/search the private index before registering a job. Taking
            // it out allows the infallible state builder to borrow other fields;
            // no interpreter/policy callback runs while the index is detached.
            let mut payload = Some(successor.payload);
            let mut ids = std::mem::take(&mut self.ids);
            let admitted = ids.insert_with(entry_key, self.budget, |budget| {
                let id = self.result.states.len();
                let scope = self.runtime.world;
                let slot = self
                    .runtime
                    .kernel()
                    .slot(scope, &mut MachineMeter(budget))
                    .map_err(kernel_failure)?;
                self.runtime.register_world(
                    id,
                    payload.as_ref().expect("pending input").call_stack.depth(),
                    budget,
                )?;
                self.partitions.push(class);
                self.updates.push(0);
                self.feedback.push(false);
                self.result.states.push(EvmMachineState {
                    id,
                    key,
                    entry: payload.take().expect("registered input"),
                    exit: None,
                    exit_stack: Vec::new(),
                    executed_pcs: Vec::new(),
                    execution_evidence: None,
                    resume_status,
                });
                self.counters.states += 1;
                self.completed_calls.push(Vec::new());
                self.source_records.push(None);
                self.alternatives.push(alternatives);
                self.inputs.push(slot);
                Ok(id)
            });
            self.ids = ids;
            match admitted {
                Ok(id) => id,
                Err(error) => {
                    return self.refuse_successor(
                        from,
                        payload.take().expect("refused input remains owned"),
                        error,
                        work_reason,
                    );
                }
            }
        };
        // 即使入口没有变，也观察到了一条有效转移；不能因“不重访”而漏掉这条边。
        self.edges.insert(MachineEdge {
            from,
            to,
            kind: successor.kind,
        });
        if X::ENABLED {
            let edge = super::cooperation::Edge {
                from: super::cooperation::Point {
                    namespace: 0,
                    kind: super::cooperation::PointKind::World,
                    index: from,
                },
                to: super::cooperation::Point {
                    namespace: 0,
                    kind: super::cooperation::PointKind::World,
                    index: to,
                },
                kind: successor.kind,
            };
            let cooperation = self.cooperation.as_mut().expect("cooperation owner");
            if let Err(error) = cooperation.edge(
                edge,
                &mut self.runtime,
                self.active.as_mut().expect("active world publisher"),
                self.budget,
            ) {
                let error = cooperation.stop(error, self.budget);
                self.frontier(Some(from), None, error.frontier_reason());
                return None;
            }
        }
        self.record_successor(from, to, successor.kind);
        Some(to)
    }

    /// Admission and acceleration are shared by ordinary propagation and staged
    /// replay. The caller prepays the metadata copy (or includes it in replay's
    /// copy envelope) before entering this lifecycle.
    fn admit_existing(
        &mut self,
        id: usize,
        incoming: &EvmMachinePayload<D::Element>,
        old_alternatives_work: usize,
    ) -> Result<(), MachineError> {
        let mut alternatives = self.alternatives[id].clone();
        let acceleration = self.policies.state.decide(AccelerationInput {
            site: PolicySite::World,
            feedback: self.feedback[id],
            updates: self.updates[id],
            default_delay: self.domain.spec().widening_after_updates(),
        });
        let joined = self.policies.state.admit(
            &self.result.states[id].entry,
            incoming,
            &mut alternatives,
            acceleration,
            &self.selectors,
            &mut PolicyContext {
                domain: self.domain,
                environment: &self.result.entry,
                budget: self.budget,
            },
        )?;
        self.install_join(id, incoming, joined, alternatives, old_alternatives_work)
    }

    /// Ordinary propagation and replay install the same changed input and
    /// invalidate its old execution. `EvmReplay<D::Element>` separately decides whether its
    /// certificate satisfies the new obligation or execution must be scheduled.
    fn install_join(
        &mut self,
        id: usize,
        incoming: &EvmMachinePayload<D::Element>,
        joined: Option<EvmMachinePayload<D::Element>>,
        mut alternatives: S::Metadata,
        old_alternatives_work: usize,
    ) -> Result<(), MachineError> {
        // None is the state adapter's whole-input coverage proof. It has not
        // changed admission metadata; retaining that covered contribution again
        // would grow the historical alternatives without semantic growth.
        let Some(joined) = joined else {
            return Ok(());
        };
        let work = installation::JoinWork::quote_for::<D::Element, S::Metadata>(
            self.result.states[id].entry.work_size(),
            incoming.work_size(),
            joined.work_size(),
            self.policies.state.metadata_work(&alternatives),
            old_alternatives_work,
            self.selectors.len(),
        );
        if work.preparation == usize::MAX || !self.budget.charge(work.preparation) {
            return Err(MachineError::work_failure());
        }
        let mut selectors = self.selectors.clone();
        if !self.policies.state.has_bound(&alternatives) {
            let mut previous = self.result.states[id].entry.fresh_leaves();
            previous.extend(incoming.fresh_leaves());
            selectors.extend(joined.fresh_leaves().difference(&previous).cloned());
        }
        self.policies
            .state
            .retain(&mut alternatives, incoming.clone());
        let bounded = self.policies.state.has_bound(&alternatives);
        let updates = self.updates[id]
            .checked_add(1)
            .ok_or(MachineError::IdentityExhausted)?;
        let public_key = joined.key();
        let state = &mut self.result.states[id];
        let current_updates = &mut self.updates[id];
        let current_alternatives = &mut self.alternatives[id];
        let current_selectors = &mut self.selectors;
        let source_record = &mut self.source_records[id];
        // The infallible installation includes coverage/frozen mode and old
        // receipt invalidation, after the kernel reserves every wake/version.
        self.runtime.with_run(
            self.active.as_mut().ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_with(
                    &self.inputs[id],
                    Change::Changed,
                    work.installation,
                    || {
                        *current_alternatives = alternatives;
                        *current_selectors = selectors;
                        *current_updates = updates;
                        state.entry = joined;
                        state.key = public_key;
                        state.invalidate_execution();
                        *source_record = None;
                    },
                )
                .map_err(kernel_failure)
            },
        )?;
        if bounded && let Some(reuse) = &mut self.reuse {
            reuse
                .dependencies
                .add(id, super::layers::FactDependency::AllInitialState);
        }
        Ok(())
    }

    fn call_resume_status(
        &self,
        from: usize,
        successor: &EvmSuccessor<D::Element>,
    ) -> Option<bool> {
        if !matches!(
            successor.kind,
            MachineEdgeKind::Return
                | MachineEdgeKind::Revert
                | MachineEdgeKind::Failure
                | MachineEdgeKind::Summary(_)
        ) || !matches!(
            successor.payload.active().code,
            super::FrameCode::Runtime | super::FrameCode::InitCode
        ) {
            return None;
        }
        let state = &self.result.states[from];
        let source = &state.entry;
        let popped_call = source.call_stack.depth()
            == successor.payload.call_stack.depth().saturating_add(1)
            && source
                .call_stack
                .active_child()
                .is_some_and(|child| child.continuation.creation.is_none());
        let rejected_call = successor.kind == MachineEdgeKind::Failure
            && source.call_stack.depth() == successor.payload.call_stack.depth()
            && state.executed_pcs.last().is_some_and(|pc| {
                source.active().program.as_ref().is_some_and(|program| {
                    program.blocks()[source.active().key.basic_block_index]
                        .instructions
                        .iter()
                        .any(|instruction| {
                            instruction.pc == *pc
                                && matches!(
                                    instruction.opcode,
                                    revm_bytecode::opcode::CALL
                                        | revm_bytecode::opcode::CALLCODE
                                        | revm_bytecode::opcode::DELEGATECALL
                                        | revm_bytecode::opcode::STATICCALL
                                )
                        })
                })
            });
        if !popped_call && !rejected_call {
            return None;
        }
        // The typed call-resume edge already proves the CALL result, including
        // local rejection. Classifying it needs no opaque numeric discovery.
        // This precision class lasts only for the first resumed block.
        policy::call_result(successor.kind)
    }

    fn joins_cycle(&mut self, target: usize, from: usize) -> Result<bool, MachineError> {
        let mut pending = vec![target];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            MachineError::check_cancelled()?;
            if id == from {
                return Ok(true);
            }
            if !seen.insert(id) {
                continue;
            }
            if !self.budget.charge(self.edges.len().saturating_add(1)) {
                return Err(MachineError::work_failure());
            }
            pending.extend(graph::outgoing(&self.edges, id).map(|edge| edge.to));
        }
        Ok(false)
    }

    /// Feedback is semantic admission metadata. Its first discovery invalidates
    /// both execution receipts and every kernel reader even for a covered input.
    fn mark_feedback(&mut self, id: usize) -> Result<(), MachineError> {
        let feedback = &mut self.feedback[id];
        let state = &mut self.result.states[id];
        let source_record = &mut self.source_records[id];
        self.runtime.with_run(
            self.active.as_mut().ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_with(&self.inputs[id], Change::Changed, 1, || {
                    *feedback = true;
                    state.invalidate_execution();
                    *source_record = None;
                })
                .map(|_| ())
                .map_err(kernel_failure)
            },
        )
    }

    fn record_successor(&mut self, from: usize, to: usize, kind: MachineEdgeKind) {
        if let Some(evidence) = &mut self.result.states[from].execution_evidence {
            let successor = (to, kind);
            if !evidence.successors.contains(&successor) {
                evidence.successors.push(successor);
            }
        }
    }

    /// Clone cost for a temporary import transaction, including retained graph
    /// metadata, certificate records and every state that might be compared.
    /// Frozen procedure evidence only copies its single shared owner handle.
    fn replay_copy_cost(&self) -> Result<usize, MachineError> {
        MachineError::check_cancelled()?;
        let mut cost = self
            .result
            .world
            .work_size()
            .saturating_add(self.result.entry.environment.work_size())
            .saturating_add(self.ids.work_size())
            .saturating_add(self.runtime.work_size())
            .saturating_add(self.edges.len())
            .saturating_add(self.diagnostics.len())
            .saturating_add(self.result.edges.len())
            .saturating_add(self.result.diagnostics.len())
            .saturating_add(self.result.frontiers.len())
            .saturating_add(1); // One ProcedureEvidence owner, independent of report size.
        cost = cost.saturating_add(self.selectors.len());
        cost = cost.saturating_add(self.source_records.len().saturating_mul(2));
        for state in &self.result.states {
            MachineError::check_cancelled()?;
            cost = cost
                .saturating_add(state.entry.work_size())
                .saturating_add(state.exit.as_ref().map_or(0, EvmMachinePayload::work_size))
                .saturating_add(state.executed_pcs.len())
                .saturating_add(state.key.frames.iter().fold(0usize, |cost, frame| {
                    cost.saturating_add(frame.jump_history.len().saturating_add(1))
                }))
                .saturating_add(
                    state
                        .exit_stack
                        .iter()
                        .fold(0usize, |cost, value| cost.saturating_add(value.work_size())),
                )
                .saturating_add(state.execution_evidence.as_ref().map_or(0, |evidence| {
                    evidence
                        .instructions
                        .len()
                        .saturating_add(evidence.successors.len())
                }));
        }
        for class in &self.partitions {
            MachineError::check_cancelled()?;
            cost = cost.saturating_add(class.key_work());
        }
        for alternatives in &self.alternatives {
            MachineError::check_cancelled()?;
            cost = cost.saturating_add(self.policies.state.metadata_work(alternatives));
        }
        for calls in &self.completed_calls {
            for call in calls {
                MachineError::check_cancelled()?;
                cost = cost.saturating_add(super::summary::call_work(call));
            }
        }
        for outcome in &self.result.outcomes {
            MachineError::check_cancelled()?;
            cost = cost
                .saturating_add(outcome.data.work_size())
                .saturating_add(outcome.store.work_size());
        }
        for record in &self.result.summaries {
            MachineError::check_cancelled()?;
            cost = cost
                .saturating_add(record.input.work_size())
                .saturating_add(record.reused_at.len());
            for output in &record.outputs {
                cost = cost
                    .saturating_add(output.data.work_size())
                    .saturating_add(output.store.work_size());
            }
        }
        for frontier in &self.result.frontiers {
            MachineError::check_cancelled()?;
            if let Some(key) = &frontier.target {
                cost = cost.saturating_add(key.frames.iter().fold(0usize, |cost, frame| {
                    cost.saturating_add(frame.jump_history.len().saturating_add(1))
                }));
            }
            if let Some(range) = &frontier.range {
                cost = cost
                    .saturating_add(range.request.offset.work_size())
                    .saturating_add(range.request.size.work_size())
                    .saturating_add(range.request.limit.work_size())
                    .saturating_add(range.condition.work_size());
            }
        }
        MachineError::check_cancelled()?;
        Ok(cost.saturating_mul(4))
    }

    fn replay_failure(
        &mut self,
        root: usize,
        reason: FrontierReason,
        pending: Arc<EvmMachinePayload<D::Element>>,
    ) -> FrontierReason {
        let reason = if reason == FrontierReason::Work {
            FrontierReason::SummaryWork
        } else {
            reason
        };
        self.observe_terminal_boundary(&reason);
        self.result.frontiers.push(EvmMachineFrontier {
            from: Some(root),
            target: Some(self.result.states[root].key.clone()),
            pc: None,
            reason: reason.clone(),
            range: None,
            payload: Some(pending),
        });
        reason
    }

    /// Rename the complete certificate, then import into a temporary graph. The
    /// shared ledger remains consumed on failure, but no partial graph, stale
    /// receipt replacement or half-renamed state is installed in the analysis.
    fn replay(
        &mut self,
        root: usize,
        certificate: &EvmCertificate<D::Element, P::Class>,
    ) -> Result<(), FrontierReason> {
        #[cfg(test)]
        self.test_admission()
            .map_err(MachineError::frontier_reason)?;
        let cost = match self.replay_copy_cost() {
            Ok(cost) => cost,
            Err(error) => {
                let reason = error.frontier_reason();
                self.frontier(
                    Some(root),
                    Some(self.result.states[root].key.clone()),
                    reason.clone(),
                );
                return Err(reason);
            }
        };
        if cost == usize::MAX || !self.budget.charge(cost) {
            let reason = super::summary::summary_boundary(self.budget);
            self.frontier(
                Some(root),
                Some(self.result.states[root].key.clone()),
                reason.clone(),
            );
            return Err(reason);
        }
        let pending = Arc::new(self.result.states[root].entry.clone());
        let prepared = match EvmReplay::new(
            certificate,
            &self.result.states[root].entry,
            self.budget,
            self.domain.spec().relations().expressions(),
        )
        .and_then(|replay| replay.prepare(self.budget, self.domain))
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(self.replay_failure(root, error.frontier_reason(), pending));
            }
        };
        let mut counters = Counters {
            states: self.counters.states,
            transfers: self.counters.transfers,
            observer: self.counters.observer.clone(),
        };
        let runtime = match self.runtime.fork(self.budget) {
            Ok(runtime) => runtime,
            Err(error) => return Err(self.replay_failure(root, error.frontier_reason(), pending)),
        };
        // The replay copy envelope includes index and class work. Check
        // cancellation around opaque clones without charging their owner twice.
        let copied_ids = self.ids.clone();
        if let Err(error) =
            PolicyContext::new(self.domain, &self.result.entry, self.budget).reserve(0)
        {
            return Err(self.replay_failure(root, error.frontier_reason(), pending));
        }
        let mut copied_partitions = Vec::new();
        for class in &self.partitions {
            if let Err(error) =
                PolicyContext::new(self.domain, &self.result.entry, self.budget).reserve(0)
            {
                return Err(self.replay_failure(root, error.frontier_reason(), pending));
            }
            let class = class.clone();
            if let Err(error) =
                PolicyContext::new(self.domain, &self.result.entry, self.budget).reserve(0)
            {
                return Err(self.replay_failure(root, error.frontier_reason(), pending));
            }
            copied_partitions.push(class);
        }
        let mut staged: EvmEngine<'_, D, C, P, S, X, E> = EvmEngine {
            result: self.result.clone(),
            domain: self.domain,
            policies: self.policies,
            budget: self.budget,
            counters: &mut counters,
            storage_reads: self.storage_reads,
            alternatives: self.alternatives.clone(),
            selectors: self.selectors.clone(),
            ids: copied_ids,
            partitions: copied_partitions,
            updates: self.updates.clone(),
            feedback: self.feedback.clone(),
            runtime,
            cooperation: None,
            inputs: self.inputs.clone(),
            active: self.active.take(),
            active_node: self.active_node,
            #[cfg(test)]
            test_driver: self.test_driver,
            edges: self.edges.clone(),
            diagnostics: self.diagnostics.clone(),
            completed_calls: self.completed_calls.clone(),
            source_records: self.source_records.clone(),
            cache: None,
            procedures: transfer::procedure::EvmTable::default(),
            reuse: None,
            terminal_boundary: self.terminal_boundary.clone(),
        };
        for (old, new) in &prepared.renaming {
            if staged.selectors.contains(old) {
                staged.selectors.insert(new.clone());
            }
        }
        if let Err(reason) = staged.install_replay(certificate, prepared) {
            // The temporary import is dropped; the original callee input still
            // describes all paths whose certificate could not be imported.
            // RPC re-execution cannot regain even failed temporary allocation
            // capacity. Only graph contents/receipts are rolled back.
            self.counters.states = staged.counters.states;
            self.active = staged.active.take();
            drop(staged);
            return Err(self.replay_failure(root, reason, pending));
        }
        self.counters.states = staged.counters.states;
        self.active = staged.active.take();
        let EvmEngine {
            result,
            terminal_boundary,
            alternatives,
            selectors,
            ids,
            partitions,
            updates,
            feedback,
            runtime,
            inputs,
            edges,
            diagnostics,
            completed_calls,
            source_records,
            ..
        } = staged;
        let commit = self.runtime.commit(runtime, self.budget, || {
            self.result = result;
            self.terminal_boundary = terminal_boundary;
            self.alternatives = alternatives;
            self.selectors = selectors;
            self.ids = ids;
            self.partitions = partitions;
            self.updates = updates;
            self.feedback = feedback;
            self.inputs = inputs;
            self.edges = edges;
            self.diagnostics = diagnostics;
            self.completed_calls = completed_calls;
            self.source_records = source_records;
        });
        if let Err(error) = commit {
            return Err(self.replay_failure(root, error.frontier_reason(), pending));
        }
        if let Some(reuse) = &mut self.reuse {
            // Staging owns no reuse index: publish conservative dependencies
            // only after the entire import succeeds. The replay copy fee
            // already covers walking the retained alternatives metadata.
            for (id, alternatives) in self.alternatives.iter().enumerate() {
                if self.policies.state.has_bound(alternatives) {
                    reuse
                        .dependencies
                        .add(id, super::layers::FactDependency::AllInitialState);
                }
            }
        }
        Ok(())
    }

    /// This method only mutates a temporary Engine owned by replay().
    fn install_replay(
        &mut self,
        certificate: &EvmCertificate<D::Element, P::Class>,
        prepared: EvmPrepared<D::Element, P::Class>,
    ) -> Result<(), FrontierReason> {
        #[cfg(test)]
        self.test_admission()
            .map_err(MachineError::frontier_reason)?;
        let mut ids = Vec::new();
        for mut node in prepared.nodes {
            // A certificate carries its arrival class, never an assumption that
            // every policy uses the legacy structural identity. Preparation may
            // cover more inputs; such a node remains an execution obligation.
            PolicyContext::new(self.domain, &self.result.entry, self.budget)
                .reserve(0)
                .map_err(MachineError::frontier_reason)?;
            let partition = node.partition.clone();
            PolicyContext::new(self.domain, &self.result.entry, self.budget)
                .reserve(0)
                .map_err(MachineError::frontier_reason)?;
            let partitioned = self
                .policies
                .state
                .prepare(
                    &node.entry,
                    PolicySite::World,
                    partition,
                    &mut PolicyContext {
                        domain: self.domain,
                        environment: &self.result.entry,
                        budget: self.budget,
                    },
                )
                .map_err(MachineError::frontier_reason)?;
            let certificate_covers_prepared = node
                .entry
                .content_form()
                .and_then(|entry| Ok(entry == partitioned.payload.content_form()?))
                .map_err(MachineError::frontier_reason)?;
            node.entry = partitioned.payload;
            let entry_key = partitioned.key;
            let key = node.entry.key();
            let id = if let Some(existing) = self
                .ids
                .get(&entry_key, self.budget)
                .map_err(MachineError::frontier_reason)?
            {
                // The replay copy reservation covers inspecting this retained
                // metadata. Reuse the quote at installation rather than walking
                // the old payload histories again before its preparation fee.
                self.source_records[existing] = None;
                let old_alternatives_work = self
                    .policies
                    .state
                    .metadata_work(&self.alternatives[existing]);
                self.admit_existing(existing, &node.entry, old_alternatives_work)
                    .map_err(MachineError::frontier_reason)?;
                // Check the installed state: acceleration may have changed its
                // representation even if the old input equalled the certificate.
                let compatible = certificate_covers_prepared
                    && self.result.states[existing]
                        .entry
                        .content_form()
                        .and_then(|entry| Ok(entry == node.entry.content_form()?))
                        .map_err(MachineError::frontier_reason)?;
                if compatible {
                    let state = &self.result.states[existing];
                    self.runtime
                        .satisfy_world(
                            existing,
                            &self.inputs[existing],
                            &state.entry,
                            state.entry.work_size(),
                            self.budget,
                        )
                        .map_err(MachineError::frontier_reason)?;
                    self.result.states[existing].exit = Some(node.exit);
                    self.result.states[existing].exit_stack = node.exit_stack;
                    self.result.states[existing].executed_pcs = node.executed_pcs;
                    self.result.states[existing].execution_evidence =
                        Some(EvmExecutionEvidence::new(
                            node.instruction_progress,
                            node.instruction_values,
                        ));
                    self.completed_calls[existing] = node.completed_calls;
                }
                existing
            } else {
                if self.counters.states >= self.result.config.analysis.max_states {
                    return Err(FrontierReason::Budget(Limit::States));
                }
                let cost = node.entry.work_size().saturating_mul(2);
                if cost == usize::MAX || !self.budget.charge(cost) {
                    return Err(super::summary::summary_boundary(self.budget));
                }
                let metadata = self
                    .policies
                    .state
                    .initialize(
                        &node.entry,
                        &mut PolicyContext {
                            domain: self.domain,
                            environment: &self.result.entry,
                            budget: self.budget,
                        },
                    )
                    .map_err(MachineError::frontier_reason)?;
                let mut index = std::mem::take(&mut self.ids);
                let admitted = index.insert_with(entry_key, self.budget, |budget| {
                    let id = self.result.states.len();
                    let scope = self.runtime.world;
                    let input = self
                        .runtime
                        .kernel()
                        .slot(scope, &mut MachineMeter(budget))
                        .map_err(kernel_failure)?;
                    self.runtime
                        .register_world(id, node.entry.call_stack.depth(), budget)?;
                    self.alternatives.push(metadata);
                    self.partitions.push(node.partition);
                    self.updates.push(0);
                    self.feedback.push(false);
                    self.result.states.push(EvmMachineState {
                        id,
                        key,
                        entry: node.entry,
                        exit: Some(node.exit),
                        exit_stack: node.exit_stack,
                        executed_pcs: node.executed_pcs,
                        execution_evidence: Some(EvmExecutionEvidence::new(
                            node.instruction_progress,
                            node.instruction_values,
                        )),
                        resume_status: node.resume_status,
                    });
                    self.counters.states += 1;
                    self.completed_calls.push(node.completed_calls);
                    self.source_records.push(None);
                    self.inputs.push(input);
                    Ok(id)
                });
                self.ids = index;
                let id = admitted.map_err(MachineError::frontier_reason)?;
                let state = &self.result.states[id];
                if certificate_covers_prepared {
                    self.runtime
                        .satisfy_world(
                            id,
                            &self.inputs[id],
                            &state.entry,
                            state.entry.work_size(),
                            self.budget,
                        )
                        .map_err(MachineError::frontier_reason)?;
                } else {
                    self.result.states[id].invalidate_execution();
                }
                id
            };
            for mut diagnostic in node.diagnostics {
                diagnostic.state = id;
                self.diagnostics.insert(diagnostic);
            }
            self.result.summary_stats.imported_states += 1;
            ids.push(id);
        }
        for edge in &certificate.edges {
            if !self.budget.charge(1) {
                return Err(super::summary::summary_boundary(self.budget));
            }
            let from = *ids
                .get(edge.from)
                .ok_or(FrontierReason::StateJoin(super::JoinBoundary::Shape))?;
            let to = *ids
                .get(edge.to)
                .ok_or(FrontierReason::StateJoin(super::JoinBoundary::Shape))?;
            // A certified graph may close feedback after its inputs were
            // imported. The new schedule is part of the retained obligation,
            // even if each individual certificate input was already covered.
            if !self.feedback[to]
                && self
                    .joins_cycle(to, from)
                    .map_err(MachineError::frontier_reason)?
            {
                self.mark_feedback(to)
                    .map_err(MachineError::frontier_reason)?;
            }
            self.edges.insert(MachineEdge {
                from,
                to,
                kind: edge.kind,
            });
            self.record_successor(from, to, edge.kind);
        }
        for terminal in prepared.terminals {
            let from = *ids
                .get(terminal.from)
                .ok_or(FrontierReason::StateJoin(super::JoinBoundary::Shape))?;
            let execution = transfer::resume_summary(
                terminal.payload,
                terminal.raw_kind,
                terminal.raw_data,
                &self.result.config,
                self.domain,
                self.budget,
            );
            if let Some(frontier) = execution.frontiers.into_iter().next() {
                return Err(frontier.reason);
            }
            for successor in execution.successors {
                if self
                    .successor(from, successor, FrontierReason::SummaryWork)
                    .is_none()
                {
                    return Err(self
                        .result
                        .frontiers
                        .last()
                        .expect("failed successor retains frontier")
                        .reason
                        .clone());
                }
            }
            if self.budget.exhausted() {
                return Err(super::summary::summary_boundary(self.budget));
            }
        }
        if !self.budget.charge(0) {
            return Err(super::summary::summary_boundary(self.budget));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
type Engine<'a> = EvmEngine<
    'a,
    crate::domain::ProductAlgebra,
    RecentDistinctJumps,
    FirstResumeStatus,
    SemanticPolicies,
    Empty,
    std::convert::Infallible,
>;

#[cfg(test)]
use super::{summary::Cache, transfer::Successor};
