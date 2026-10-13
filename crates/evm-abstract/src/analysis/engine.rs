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
//! `queue` 保存 FIFO 调度记录，`queued` 才表示哪些节点当前仍需处理。
//! 最终状态由是否存在 frontier 决定，仅队列排空不足以证明 `Converged`。

use super::{
    ConfigError, Diagnostic, Limit, Status, SummaryStats,
    machine::{
        EntryAlternatives, ExecutionConfig, ExecutionEvidence, FrontierReason, MachineEdge,
        MachineEdgeKind, MachineError, MachineFrontier, MachineOutcome, MachinePayload,
        MachineState, OutcomeKind, WorldAnalysis,
    },
    summary::{Cache, Certificate, Prepared, Replay},
    transfer::{self, CompletedCall, Successor, WorkBudget},
};
use crate::{
    domain::{Domain, symbolic::ExprId},
    world::{Entry, World},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

/// 从固定世界、入口上下文和执行策略建立初始机器，并运行同一事务的工作表。
///
/// 配置不合法返回 `ConfigError`；缺少入口代码等模型边界则返回带 frontier 的
/// `Incomplete` 分析，让调用者能够区分“不能完成分析”和 EVM 自身执行失败。
pub(super) fn run_world(
    world: World,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<WorldAnalysis, ConfigError> {
    run_world_with_observer(world, entry, config, &super::progress::Observer::default())
}

pub(super) fn run_world_with_observer(
    world: World,
    entry: Entry,
    config: ExecutionConfig,
    observer: &super::progress::Observer,
) -> Result<WorldAnalysis, ConfigError> {
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::with_observer(observer);
    run_metered(
        world,
        entry,
        config,
        &mut budget,
        &mut counters,
        transfer::StorageReadPolicy::Abstract,
    )
}

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

    pub(super) fn report(&self, work: usize) {
        self.observer.emit(super::progress::Event::Execution {
            states: self.states,
            transfers: self.transfers,
            work,
        });
    }
}

/// Execute one fixed snapshot under the caller's cumulative resource ledger.
pub(super) fn run_metered(
    world: World,
    mut entry: Entry,
    config: ExecutionConfig,
    budget: &mut WorkBudget,
    counters: &mut Counters,
    storage_reads: transfer::StorageReadPolicy,
) -> Result<WorldAnalysis, ConfigError> {
    counters.observer.phase(super::progress::Phase::Analyzing);
    world.apply_snapshot_environment(&mut entry.environment);
    entry.environment.validate()?;
    if let Some(observed) = entry.environment.to.as_concrete()
        && observed != entry.address
    {
        return Err(crate::world::environment::EnvironmentError::Destination {
            expected: entry.address,
            observed,
        }
        .into());
    }
    let domain = config.domain()?;
    let mut result = WorldAnalysis {
        world,
        entry,
        config,
        schema_version: super::SCHEMA_VERSION,
        domain_spec: domain.spec(),
        states: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        status: Status::Converged,
        transfers: counters.transfers,
        work: 0,
        rpc_acquisition: None,
        summary_stats: SummaryStats::default(),
        summaries: Vec::new(),
        unknown_summaries: Vec::new(),
    };
    // 输入 lift、Store 与回滚点复制也使用根账本，不让初始化获得免费工作。
    if counters.states >= result.config.analysis.max_states {
        result.frontiers.push(MachineFrontier {
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
        result.frontiers.push(MachineFrontier {
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
            result.frontiers.push(MachineFrontier {
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
        result.frontiers.push(MachineFrontier {
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
    let alternatives = EntryAlternatives::new(&initial);
    if storage_reads == transfer::StorageReadPolicy::Discover
        && initial.active().code == super::FrameCode::Unknown
    {
        result.frontiers.push(MachineFrontier {
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
    // 本模型不精确跟踪剩余 gas，先保留入口即失败、Store 不变的保守可能。
    // 这条 outcome 不是 transfer 已执行指令后报告的异常，也不是未完成前沿。
    result.outcomes.push(MachineOutcome {
        state: 0,
        kind: OutcomeKind::Failure,
        data: crate::world::ByteArray::empty(),
        store: initial.store.clone(),
    });
    // S0 是状态表中的第一个节点。后续编号按发现顺序分配，不按基本块或 PC 排序。
    let key = initial.key();
    result.states.push(MachineState {
        id: 0,
        key: key.clone(),
        entry: initial,
        exit: None,
        exit_stack: Vec::new(),
        executed_pcs: Vec::new(),
        execution_evidence: None,
    });
    counters.states += 1;
    let cache = result.config.use_summaries.then(Cache::new);
    let mut engine = Engine {
        result,
        domain,
        budget,
        counters,
        storage_reads,
        ids: BTreeMap::from([(key, 0)]),
        queue: VecDeque::from([0]),
        queued: BTreeSet::from([0]),
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new()],
        cache,
        alternatives: vec![alternatives],
        selectors: BTreeSet::new(),
        unknown: transfer::unknown::Table::default(),
    };
    engine.run();
    engine.counters.report(engine.budget.used());
    Ok(engine.result)
}

/// 拥有一次世界分析的调度状态、执行证据和共享预算。
///
/// `result.states` 保存节点内容，`ids` 提供按结构键查找节点的索引；待处理队列
/// 只保存节点编号。分支、循环、嵌套调用和摘要导入都汇入这同一张固定点图。
struct Engine<'a> {
    /// 对外返回的分析结果；状态入口可继续 join，出口保存最近一次转换的证据。
    result: WorldAnalysis,
    /// 所有普通转换、join 和摘要回放共用的有限值域及其容量规则。
    domain: Domain,
    /// 入口复制、执行、join、摘要比较/认证/导入共用的累计工作账本。
    budget: &'a mut WorkBudget,
    /// Prior RPC refinement rounds also consume state and transfer capacity.
    counters: &'a mut Counters,
    /// 是否在 RPC refinement 中把缺失的初始槽观测记录为请求边界。
    storage_reads: transfer::StorageReadPolicy,
    /// 完整 payload 备选的覆盖证据；独立字段的数值 widening 不证明状态覆盖。
    alternatives: Vec<EntryAlternatives>,
    /// Only selectors allocated by a machine join, never source/observation IDs.
    selectors: BTreeSet<ExprId>,
    /// 结构键 -> `result.states` 的索引；键相同才允许汇合抽象输入。
    ids: BTreeMap<super::MachineKey, usize>,
    /// FIFO 调度记录；摘要回放可能满足已排队节点，因此这里允许残留失效项。
    queue: VecDeque<usize>,
    /// 当前仍需处理的节点集合，用于去重和判断 queue 项是否仍有效。
    queued: BTreeSet<usize>,
    /// 已观察到的转移边，按结构排序去重；重访节点时不会删除旧边。
    edges: BTreeSet<MachineEdge>,
    /// 按状态、PC 和种类去重的程序诊断，与未完成分析的 frontier 分开保存。
    diagnostics: BTreeSet<Diagnostic>,
    /// 与 `result.states` 按编号对齐的子调用终结证据，供摘要认证使用。
    /// 保存 caller join 之前的原始终结和状态效果，不能从已汇合的返回节点逆推。
    completed_calls: Vec<Vec<CompletedCall>>,
    /// 可选的完整 callee 图证书缓存；关闭摘要时普通工作表路径仍然成立。
    cache: Option<Cache>,
    /// Known/unknown procedure inputs, outputs and retained return continuations.
    unknown: transfer::unknown::Table,
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

impl Engine<'_> {
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
        self.result.frontiers.push(MachineFrontier {
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
        payload: MachinePayload,
    ) {
        self.result.frontiers.push(MachineFrontier {
            from,
            target: Some(payload.key()),
            pc: None,
            reason,
            range: None,
            payload: Some(Arc::new(payload)),
        });
    }

    fn stop_queued(&mut self, reason: FrontierReason) {
        let pending: Vec<_> = self.queued.iter().copied().collect();
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
    /// `id` 已经从 queue 弹出并移除 queued 标记，必须单独纳入；否则结果会
    /// 漏掉刚刚因预算停止的节点。过滤 queued 可排除摘要回放留下的失效队列项。
    fn stop_pending(&mut self, id: usize, reason: FrontierReason) {
        let pending: Vec<_> = std::iter::once(id)
            .chain(
                self.queue
                    .iter()
                    .copied()
                    .filter(|id| self.queued.contains(id)),
            )
            .collect();
        for pending in pending {
            self.frontier(
                None,
                Some(self.result.states[pending].key.clone()),
                reason.clone(),
            );
        }
    }

    /// Finish deeper callees before propagating their callers' pending unions.
    /// A caller's immediate rejection and later return then meet before that
    /// caller executes. Same-depth nodes retain their existing FIFO order.
    fn next_pending(&mut self) -> Result<Option<usize>, MachineError> {
        if self.queued.is_empty() {
            return Ok(None);
        }
        let cost = self.queue.len().saturating_mul(2).saturating_add(1);
        if cost == usize::MAX || !self.budget.charge(cost) {
            return Err(MachineError::work_failure());
        }
        let mut choice = None;
        let mut depth = 0;
        for (position, id) in self.queue.iter().enumerate() {
            MachineError::check_cancelled()?;
            let candidate_depth = self.result.states[*id].entry.call_stack.depth();
            if self.queued.contains(id) && candidate_depth > depth {
                choice = Some(position);
                depth = candidate_depth;
            }
        }
        Ok(choice.and_then(|position| self.queue.remove(position)))
    }

    /// 反复处理入口摘要有新信息的节点，并在结束时整理对外结果。
    ///
    /// 普通执行与摘要复用最终都更新同一状态表。一次完成的 transfer 不代表
    /// 节点永远完成：后续 join 若扩大入口，`successor` 会让它重新参与传播。
    fn run(&mut self) {
        loop {
            let id = match self.next_pending() {
                Ok(Some(id)) => id,
                Ok(None) => break,
                Err(error) => {
                    self.stop_queued(error.frontier_reason());
                    break;
                }
            };
            self.counters.report(self.budget.used());
            // 回放可能已经用证书满足这个节点，只移除了其 queued 标记。
            // 物理队列里的旧编号仍可存在；没有标记时跳过，不重复执行已满足节点。
            if !self.queued.remove(&id) {
                continue;
            }
            if self.result.transfers >= self.result.config.analysis.max_transfers
                || self.budget.exhausted()
            {
                let reason = if self.budget.exhausted() {
                    MachineError::work_failure().frontier_reason()
                } else {
                    FrontierReason::Budget(Limit::Transfers)
                };
                self.stop_pending(id, reason);
                break;
            }
            // 先为本轮入口载荷的处理计费；深调用或缓存命中都不能获得新预算。
            if !self.budget.charge(self.result.states[id].entry.work_size()) {
                self.stop_pending(id, MachineError::work_failure().frontier_reason());
                break;
            }
            // 暂时移出 cache，使它与 &mut self 能同时用于传播和认证。
            // 每条继续/中断路径都放回同一缓存；take 不是清空或重置分析结果。
            let mut cache = self.cache.take();
            let is_unknown =
                self.result.states[id].entry.active().code == super::FrameCode::Unknown;
            let attempt = if !is_unknown && let Some(cache) = &mut cache {
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
            if !reused {
                let execution = if is_unknown {
                    let (execution, evidence) = self.unknown.execute(
                        id,
                        &self.result.world,
                        &self.result.entry,
                        self.result.states[id].entry.clone(),
                        &self.result.config,
                        self.domain,
                        self.budget,
                        self.counters,
                        self.storage_reads,
                    );
                    self.result.transfers = self.counters.transfers;
                    self.result.unknown_summaries.push(evidence);
                    execution
                } else {
                    self.result.transfers += 1;
                    self.counters.transfers += 1;
                    transfer::execute(
                        &self.result.world,
                        &self.result.entry,
                        self.result.states[id].entry.clone(),
                        &self.result.config,
                        self.domain,
                        self.budget,
                        self.storage_reads,
                    )
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
                    &self.queued,
                    &self.diagnostics,
                    &self.completed_calls,
                    self.budget,
                )
            });
            self.cache = cache;
            if !published {
                let target = self
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.pending_target(&self.result));
                let reason = super::summary::summary_boundary();
                self.frontier(Some(id), target, reason.clone());
                self.stop_queued(reason);
                break;
            }
        }
        // 未闭合的候选只计为拒绝，不能在分析停止时补成“成功摘要”。
        if let Some(cache) = self.cache.take() {
            cache.finish(&mut self.result);
        }
        // 摘要中断也可能直接离开循环，因此用 frontier 判定完整性，不用队列长度。
        self.result.status = if self.result.frontiers.is_empty() {
            Status::Converged
        } else {
            Status::Incomplete
        };
        self.result.edges = std::mem::take(&mut self.edges).into_iter().collect();
        self.result.diagnostics = std::mem::take(&mut self.diagnostics).into_iter().collect();
        self.result.work = self.budget.used();
    }

    /// 只在普通子调用入口尝试完整图证书；CREATE initcode 不走此复用途径。
    ///
    /// 匹配需要当前 callee、世界、Store 和分析策略的完整输入相等。未命中时
    /// 登记认证候选并继续普通执行；输入构造、查找或导入耗尽预算则留下前沿。
    fn try_summary(&mut self, id: usize, cache: &mut Cache) -> SummaryAttempt {
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
                    let reason = super::summary::summary_boundary();
                    self.frontier(
                        Some(id),
                        Some(self.result.states[id].key.clone()),
                        reason.clone(),
                    );
                    SummaryAttempt::Interrupted(reason)
                }
            }
            Err(()) => {
                let reason = super::summary::summary_boundary();
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
        mut execution: transfer::Execution,
        mut cache: Option<&mut Cache>,
    ) {
        execution.payload.normalize();
        for completed in &mut execution.completed_calls {
            completed.payload.normalize();
        }
        self.result.states[id].exit_stack = execution.payload.active().stack.clone();
        self.result.states[id].executed_pcs = execution.executed_pcs;
        self.result.states[id].execution_evidence = Some(ExecutionEvidence::new(
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
            let super::transfer::ExecutionFrontier {
                pc,
                reason,
                target,
                range,
                payload,
            } = frontier;
            self.result.frontiers.push(MachineFrontier {
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
            self.result.outcomes.push(MachineOutcome {
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

    /// 将一个后继输入汇入按结构键区分的节点，并记录从 `from` 到该节点的边。
    ///
    /// 已有节点只通过 join 扩大入口；输入确实变化才重新排队。新键创建节点并
    /// 初次排队。返回 `None` 表示复制/join 或节点数量预算不足，已记录 frontier。
    /// `work_reason` 让普通传播与摘要导入分别报告 Work 和 SummaryWork。
    fn successor(
        &mut self,
        from: usize,
        mut successor: Successor,
        work_reason: FrontierReason,
    ) -> Option<usize> {
        successor.payload.normalize();
        let key = successor.payload.key();
        let to = if let Some(existing) = self.ids.get(&key).copied() {
            let cyclic = match self.joins_cycle(existing, from) {
                Ok(cyclic) => cyclic,
                Err(error) => {
                    self.frontier_payload(Some(from), error.frontier_reason(), successor.payload);
                    return None;
                }
            };
            let joined = match self.alternatives[existing].join_cyclic(
                &self.result.states[existing].entry,
                &successor.payload,
                self.domain,
                self.budget,
                &self.selectors,
                cyclic,
            ) {
                Ok(joined) => joined,
                Err(error) => {
                    let reason = if error == MachineError::Work {
                        work_reason
                    } else {
                        error.frontier_reason()
                    };
                    self.frontier_payload(Some(from), reason, successor.payload);
                    return None;
                }
            };
            if let Some(joined) = joined {
                let mut previous = self.result.states[existing].entry.fresh_leaves();
                previous.extend(successor.payload.fresh_leaves());
                self.selectors
                    .extend(joined.fresh_leaves().difference(&previous).cloned());
                self.alternatives[existing].retain(successor.payload);
                self.result.states[existing].entry = joined;
                self.result.states[existing].invalidate_execution();
                if self.queued.insert(existing) {
                    self.queue.push_back(existing);
                }
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
            let alternatives = EntryAlternatives::new(&successor.payload);
            // states 与 completed_calls 同时追加，保证后者始终可用同一节点编号索引。
            let id = self.result.states.len();
            self.ids.insert(key.clone(), id);
            self.result.states.push(MachineState {
                id,
                key,
                entry: successor.payload,
                exit: None,
                exit_stack: Vec::new(),
                executed_pcs: Vec::new(),
                execution_evidence: None,
            });
            self.counters.states += 1;
            self.completed_calls.push(Vec::new());
            self.alternatives.push(alternatives);
            self.queue.push_back(id);
            self.queued.insert(id);
            id
        };
        // 即使入口没有变，也观察到了一条有效转移；不能因“不重访”而漏掉这条边。
        self.edges.insert(MachineEdge {
            from,
            to,
            kind: successor.kind,
        });
        self.record_successor(from, to, successor.kind);
        Some(to)
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
            pending.extend(
                self.edges
                    .iter()
                    .filter(|edge| edge.from == id)
                    .map(|edge| edge.to),
            );
        }
        Ok(false)
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
    fn replay_copy_cost(&self) -> Result<usize, MachineError> {
        MachineError::check_cancelled()?;
        let mut cost = self
            .result
            .world
            .work_size()
            .saturating_add(self.result.entry.environment.work_size())
            .saturating_add(self.ids.len())
            .saturating_add(self.queue.len())
            .saturating_add(self.queued.len())
            .saturating_add(self.edges.len())
            .saturating_add(self.diagnostics.len())
            .saturating_add(self.result.edges.len())
            .saturating_add(self.result.diagnostics.len())
            .saturating_add(self.result.frontiers.len());
        cost = cost.saturating_add(self.selectors.len());
        for state in &self.result.states {
            MachineError::check_cancelled()?;
            cost = cost
                .saturating_add(state.entry.work_size())
                .saturating_add(state.exit.as_ref().map_or(0, MachinePayload::work_size))
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
        for alternatives in &self.alternatives {
            MachineError::check_cancelled()?;
            cost = cost.saturating_add(alternatives.work_size());
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
        pending: Arc<MachinePayload>,
    ) -> FrontierReason {
        let reason = if reason == FrontierReason::Work {
            FrontierReason::SummaryWork
        } else {
            reason
        };
        self.result.frontiers.push(MachineFrontier {
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
    fn replay(&mut self, root: usize, certificate: &Certificate) -> Result<(), FrontierReason> {
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
            let reason = super::summary::summary_boundary();
            self.frontier(
                Some(root),
                Some(self.result.states[root].key.clone()),
                reason.clone(),
            );
            return Err(reason);
        }
        let pending = Arc::new(self.result.states[root].entry.clone());
        let prepared = match Replay::new(
            certificate,
            &self.result.states[root].entry,
            self.budget,
            self.result.config.analysis.relations.expressions(),
        )
        .and_then(|replay| replay.prepare(self.budget))
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
        let mut staged = Engine {
            result: self.result.clone(),
            domain: self.domain,
            budget: self.budget,
            counters: &mut counters,
            storage_reads: self.storage_reads,
            alternatives: self.alternatives.clone(),
            selectors: self.selectors.clone(),
            ids: self.ids.clone(),
            queue: self.queue.clone(),
            queued: self.queued.clone(),
            edges: self.edges.clone(),
            diagnostics: self.diagnostics.clone(),
            completed_calls: self.completed_calls.clone(),
            cache: None,
            unknown: transfer::unknown::Table::default(),
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
            drop(staged);
            return Err(self.replay_failure(root, reason, pending));
        }
        self.counters.states = staged.counters.states;
        self.result = staged.result;
        self.alternatives = staged.alternatives;
        self.selectors = staged.selectors;
        self.ids = staged.ids;
        self.queue = staged.queue;
        self.queued = staged.queued;
        self.edges = staged.edges;
        self.diagnostics = staged.diagnostics;
        self.completed_calls = staged.completed_calls;
        Ok(())
    }

    /// This method only mutates a temporary Engine owned by replay().
    fn install_replay(
        &mut self,
        certificate: &Certificate,
        prepared: Prepared,
    ) -> Result<(), FrontierReason> {
        let mut ids = Vec::new();
        for node in prepared.nodes {
            let key = node.entry.key();
            let id = if let Some(existing) = self.ids.get(&key).copied() {
                let joined = self.alternatives[existing]
                    .join_cyclic(
                        &self.result.states[existing].entry,
                        &node.entry,
                        self.domain,
                        self.budget,
                        &self.selectors,
                        false,
                    )
                    .map_err(MachineError::frontier_reason)?;
                // Equality is a coverage proof. A whole-state join containing
                // other alternatives cannot use this certificate's exit alone.
                let compatible = self.result.states[existing]
                    .entry
                    .content_form()
                    .and_then(|entry| Ok(entry == node.entry.content_form()?))
                    .map_err(MachineError::frontier_reason)?;
                if let Some(joined) = joined {
                    let mut previous = self.result.states[existing].entry.fresh_leaves();
                    previous.extend(node.entry.fresh_leaves());
                    self.selectors
                        .extend(joined.fresh_leaves().difference(&previous).cloned());
                    self.alternatives[existing].retain(node.entry.clone());
                    self.result.states[existing].entry = joined;
                    self.result.states[existing].invalidate_execution();
                }
                if compatible {
                    self.queued.remove(&existing);
                    self.result.states[existing].exit = Some(node.exit);
                    self.result.states[existing].exit_stack = node.exit_stack;
                    self.result.states[existing].executed_pcs = node.executed_pcs;
                    self.result.states[existing].execution_evidence = Some(ExecutionEvidence::new(
                        node.instruction_progress,
                        node.instruction_values,
                    ));
                    self.completed_calls[existing] = node.completed_calls;
                } else if self.queued.insert(existing) {
                    self.queue.push_back(existing);
                }
                existing
            } else {
                if self.counters.states >= self.result.config.analysis.max_states {
                    return Err(FrontierReason::Budget(Limit::States));
                }
                let cost = node.entry.work_size().saturating_mul(2);
                if cost == usize::MAX || !self.budget.charge(cost) {
                    return Err(super::summary::summary_boundary());
                }
                let id = self.result.states.len();
                self.alternatives.push(EntryAlternatives::new(&node.entry));
                self.ids.insert(key.clone(), id);
                self.result.states.push(MachineState {
                    id,
                    key,
                    entry: node.entry,
                    exit: Some(node.exit),
                    exit_stack: node.exit_stack,
                    executed_pcs: node.executed_pcs,
                    execution_evidence: Some(ExecutionEvidence::new(
                        node.instruction_progress,
                        node.instruction_values,
                    )),
                });
                self.counters.states += 1;
                self.completed_calls.push(node.completed_calls);
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
                return Err(super::summary::summary_boundary());
            }
            let from = *ids
                .get(edge.from)
                .ok_or(FrontierReason::StateJoin(super::JoinBoundary::Shape))?;
            let to = *ids
                .get(edge.to)
                .ok_or(FrontierReason::StateJoin(super::JoinBoundary::Shape))?;
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
                return Err(super::summary::summary_boundary());
            }
        }
        if !self.budget.charge(0) {
            return Err(super::summary::summary_boundary());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
