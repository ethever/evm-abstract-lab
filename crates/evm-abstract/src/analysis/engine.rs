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
        ExecutionConfig, FrontierReason, MachineEdge, MachineEdgeKind, MachineFrontier,
        MachineOutcome, MachineState, OutcomeKind, WorldAnalysis,
    },
    summary::{Cache, Certificate, Replay},
    transfer::{self, CompletedCall, Successor, WorkBudget},
};
use crate::{
    domain::Domain,
    world::{Entry, World},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// 从固定世界、入口上下文和执行策略建立初始机器，并运行同一事务的工作表。
///
/// 配置不合法返回 `ConfigError`；缺少入口代码等模型边界则返回带 frontier 的
/// `Incomplete` 分析，让调用者能够区分“不能完成分析”和 EVM 自身执行失败。
pub(super) fn run_world(
    world: World,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<WorldAnalysis, ConfigError> {
    let domain = config.domain()?;
    let mut result = WorldAnalysis {
        world,
        entry,
        config,
        states: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        status: Status::Converged,
        transfers: 0,
        work: 0,
        summary_stats: SummaryStats::default(),
        summaries: Vec::new(),
    };
    let initial = match transfer::initial(&result.world, &result.entry) {
        Ok(payload) => payload,
        Err(reason) => {
            result.frontiers.push(MachineFrontier {
                from: None,
                target: None,
                pc: None,
                reason,
            });
            result.status = Status::Incomplete;
            return Ok(result);
        }
    };
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
    });
    let cache = result.config.use_summaries.then(Cache::new);
    let budget = WorkBudget::new(result.config.max_work);
    let mut engine = Engine {
        result,
        domain,
        budget,
        ids: BTreeMap::from([(key, 0)]),
        queue: VecDeque::from([0]),
        queued: BTreeSet::from([0]),
        edges: BTreeSet::new(),
        diagnostics: BTreeSet::new(),
        completed_calls: vec![Vec::new()],
        cache,
    };
    engine.run();
    Ok(engine.result)
}

/// 拥有一次世界分析的调度状态、执行证据和共享预算。
///
/// `result.states` 保存节点内容，`ids` 提供按结构键查找节点的索引；待处理队列
/// 只保存节点编号。分支、循环、嵌套调用和摘要导入都汇入这同一张固定点图。
struct Engine {
    /// 对外返回的分析结果；状态入口可继续 join，出口保存最近一次转换的证据。
    result: WorldAnalysis,
    /// 所有普通转换、join 和摘要回放共用的有限值域及其容量规则。
    domain: Domain,
    /// 入口复制、执行、join、摘要比较/认证/导入共用的累计工作账本。
    budget: WorkBudget,
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
}

/// 当前节点尝试使用摘要后的调度决定，而非子调用的 Return/Revert/Failure。
enum SummaryAttempt {
    /// 不适用或没有完整证书；本轮仍需执行普通 transfer。
    Miss,
    /// 已导入认证子图并连接当前 caller；本轮无须再执行这个 callee 入口。
    Hit,
    /// 摘要工作没有完成，已经记录未完成前沿；必须停止，不能当作 Miss 回退。
    Interrupted,
}

impl Engine {
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
        });
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

    /// 反复处理入口摘要有新信息的节点，并在结束时整理对外结果。
    ///
    /// 普通执行与摘要复用最终都更新同一状态表。一次完成的 transfer 不代表
    /// 节点永远完成：后续 join 若扩大入口，`successor` 会让它重新参与传播。
    fn run(&mut self) {
        while let Some(id) = self.queue.pop_front() {
            // 回放可能已经用证书满足这个节点，只移除了其 queued 标记。
            // 物理队列里的旧编号仍可存在；没有标记时跳过，不重复执行已满足节点。
            if !self.queued.remove(&id) {
                continue;
            }
            if self.result.transfers >= self.result.config.analysis.max_transfers
                || self.budget.exhausted()
            {
                let reason = if self.budget.exhausted() {
                    FrontierReason::Work
                } else {
                    FrontierReason::Budget(Limit::Transfers)
                };
                self.stop_pending(id, reason);
                break;
            }
            // 先为本轮入口载荷的处理计费；深调用或缓存命中都不能获得新预算。
            if !self.budget.charge(self.result.states[id].entry.work_size()) {
                self.stop_pending(id, FrontierReason::Work);
                break;
            }
            // 暂时移出 cache，使它与 &mut self 能同时用于传播和认证。
            // 每条继续/中断路径都放回同一缓存；take 不是清空或重置分析结果。
            let mut cache = self.cache.take();
            let attempt = if let Some(cache) = &mut cache {
                self.try_summary(id, cache)
            } else {
                SummaryAttempt::Miss
            };
            if matches!(attempt, SummaryAttempt::Interrupted) {
                self.cache = cache;
                break;
            }
            let reused = matches!(attempt, SummaryAttempt::Hit);
            if !reused {
                self.result.transfers += 1;
                let execution = transfer::execute(
                    &self.result.world,
                    &self.result.entry,
                    self.result.states[id].entry.clone(),
                    &self.result.config,
                    self.domain,
                    &mut self.budget,
                );
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
                    &mut self.budget,
                )
            });
            self.cache = cache;
            if !published {
                let target = self
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.pending_target(&self.result));
                self.frontier(Some(id), target, FrontierReason::SummaryWork);
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
        let Some(input) = cache.input(&self.result, id, &mut self.budget) else {
            self.frontier(
                Some(id),
                Some(self.result.states[id].key.clone()),
                FrontierReason::SummaryWork,
            );
            return SummaryAttempt::Interrupted;
        };
        match cache.lookup(&input, &self.result, &mut self.budget) {
            Ok(Some(index)) => {
                let certificate = cache.certificate(index);
                self.result.summary_stats.hits += 1;
                if self.replay(id, certificate) {
                    self.result.summaries[certificate.record].reused_at.push(id);
                    SummaryAttempt::Hit
                } else {
                    SummaryAttempt::Interrupted
                }
            }
            Ok(None) => {
                self.result.summary_stats.misses += 1;
                if cache.begin(id, input, &mut self.budget) {
                    SummaryAttempt::Miss
                } else {
                    self.frontier(
                        Some(id),
                        Some(self.result.states[id].key.clone()),
                        FrontierReason::SummaryWork,
                    );
                    SummaryAttempt::Interrupted
                }
            }
            Err(()) => {
                self.frontier(
                    Some(id),
                    Some(self.result.states[id].key.clone()),
                    FrontierReason::SummaryWork,
                );
                SummaryAttempt::Interrupted
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
        self.result.states[id].exit_stack = execution.payload.active().stack.clone();
        self.result.states[id].executed_pcs = execution.executed_pcs;
        self.result.states[id].exit = Some(execution.payload);
        self.completed_calls[id] = execution.completed_calls;
        for (pc, kind) in execution.diagnostics {
            self.diagnostics.insert(Diagnostic {
                state: id,
                pc,
                kind,
            });
        }
        for (pc, reason, target) in execution.frontiers {
            self.result.frontiers.push(MachineFrontier {
                from: Some(id),
                target,
                pc: Some(pc),
                reason,
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
            let cost = successor
                .payload
                .work_size()
                .saturating_add(self.result.states[existing].entry.work_size());
            if !self.budget.charge(cost) {
                self.frontier(Some(from), Some(key), work_reason);
                return None;
            }
            let joined = self.result.states[existing]
                .entry
                .join(&successor.payload, self.domain);
            // 有节点不等于有固定点；只有比较 join 前后的入口，才知道要不要重访。
            if joined != self.result.states[existing].entry {
                self.result.states[existing].entry = joined;
                if self.queued.insert(existing) {
                    self.queue.push_back(existing);
                }
            }
            existing
        } else {
            if self.result.states.len() >= self.result.config.analysis.max_states {
                self.frontier(Some(from), Some(key), FrontierReason::Budget(Limit::States));
                return None;
            }
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
            });
            self.completed_calls.push(Vec::new());
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
        Some(to)
    }

    /// 在当前 caller 下实例化认证 callee 子图，并重新生成其返回 caller 的转移。
    ///
    /// `root` 是当前世界图中的 callee 入口编号，不是事务的 RootFrame。证书编号
    /// 是局部编号，导入时需要映射到当前状态表；同键节点仍须遵守普通 join 规则。
    /// 返回 `false` 表示导入未完成：已加入的节点/边不会整体撤销，而是通过前沿
    /// 标明结果不完整，禁止把这个部分图当作完整分析或用于构建 SSA。
    fn replay(&mut self, root: usize, certificate: &Certificate) -> bool {
        let Some(replay) = Replay::new(
            certificate,
            &self.result.states[root].entry,
            &mut self.budget,
        ) else {
            self.frontier(
                Some(root),
                Some(self.result.states[root].key.clone()),
                FrontierReason::SummaryWork,
            );
            return false;
        };
        // 证书局部节点编号 -> 当前世界节点编号。它与 self.ids 的“结构键索引”不同。
        let mut ids = Vec::new();
        for index in 0..certificate.nodes.len() {
            let Some(node) = replay.node(index, &mut self.budget) else {
                self.frontier(
                    Some(root),
                    Some(self.result.states[root].key.clone()),
                    FrontierReason::SummaryWork,
                );
                return false;
            };
            let key = node.entry.key();
            let id = if let Some(existing) = self.ids.get(&key).copied() {
                if !self.budget.charge(
                    node.entry
                        .work_size()
                        .saturating_add(self.result.states[existing].entry.work_size()),
                ) {
                    self.frontier(Some(root), Some(key), FrontierReason::SummaryWork);
                    return false;
                }
                let joined = self.result.states[existing]
                    .entry
                    .join(&node.entry, self.domain);
                // join 没有超出证书入口，说明认证出口覆盖现有输入，可以取消调度。
                // 若旧输入带来证书没有覆盖的新值，不能安装其出口，须重访汇合入口。
                let compatible = joined == node.entry;
                self.result.states[existing].entry = joined;
                if compatible {
                    self.queued.remove(&existing);
                    self.result.states[existing].exit = Some(node.exit);
                    self.result.states[existing].exit_stack = node.exit_stack;
                    self.result.states[existing].executed_pcs = node.executed_pcs;
                    self.completed_calls[existing] = node.completed_calls;
                } else if self.queued.insert(existing) {
                    self.queue.push_back(existing);
                }
                existing
            } else {
                if self.result.states.len() >= self.result.config.analysis.max_states {
                    self.frontier(Some(root), Some(key), FrontierReason::Budget(Limit::States));
                    return false;
                }
                // 证书已提供这个新节点的入口与出口证据，直接安装，不再排队重复执行。
                let id = self.result.states.len();
                self.ids.insert(key.clone(), id);
                self.result.states.push(MachineState {
                    id,
                    key,
                    entry: node.entry,
                    exit: Some(node.exit),
                    exit_stack: node.exit_stack,
                    executed_pcs: node.executed_pcs,
                });
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
        // 内部边两端先通过上述编号映射连接；不复制旧 caller 的节点编号。
        for edge in &certificate.edges {
            if !self.budget.charge(1) {
                self.frontier(Some(root), None, FrontierReason::SummaryWork);
                return false;
            }
            self.edges.insert(MachineEdge {
                from: ids[edge.from],
                to: ids[edge.to],
                kind: edge.kind,
            });
        }
        // 证书终结保留的是 callee 原始结果。返回成功位、returndata、输出复制、
        // 回滚与返回位置必须通过当前 caller 的 continuation 重新应用。
        for (index, terminal) in certificate.terminals.iter().enumerate() {
            let Some(payload) = replay.terminal_payload(index, &mut self.budget) else {
                self.frontier(Some(root), None, FrontierReason::SummaryWork);
                return false;
            };
            let execution = transfer::resume_summary(
                payload,
                terminal.raw_kind,
                terminal.raw_data.clone(),
                &self.result.config,
                self.domain,
                &mut self.budget,
            );
            let from = ids[terminal.from];
            // 返回处理也消耗同一预算；其中的 Work 属于本次摘要导入，改记 SummaryWork。
            for (pc, reason, target) in execution.frontiers {
                self.result.frontiers.push(MachineFrontier {
                    from: Some(from),
                    target,
                    pc: Some(pc),
                    reason: if reason == FrontierReason::Work {
                        FrontierReason::SummaryWork
                    } else {
                        reason
                    },
                });
            }
            for successor in execution.successors {
                if self
                    .successor(from, successor, FrontierReason::SummaryWork)
                    .is_none()
                {
                    return false;
                }
            }
            if self.budget.exhausted() {
                return false;
            }
        }
        true
    }
}
