//! One worklist spans every active and suspended execution frame. Complete
//! callee summaries reuse this same fixed-point graph and its shared ledger.

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
    result.outcomes.push(MachineOutcome {
        state: 0,
        kind: OutcomeKind::Failure,
        data: crate::world::ByteArray::empty(),
        store: initial.store.clone(),
    });
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

struct Engine {
    result: WorldAnalysis,
    domain: Domain,
    budget: WorkBudget,
    ids: BTreeMap<super::MachineKey, usize>,
    queue: VecDeque<usize>,
    queued: BTreeSet<usize>,
    edges: BTreeSet<MachineEdge>,
    diagnostics: BTreeSet<Diagnostic>,
    completed_calls: Vec<Vec<CompletedCall>>,
    cache: Option<Cache>,
}

enum SummaryAttempt {
    Miss,
    Hit,
    Interrupted,
}

impl Engine {
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

    fn run(&mut self) {
        while let Some(id) = self.queue.pop_front() {
            // Cached graph import can satisfy a node that was already queued.
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
            if !self.budget.charge(self.result.states[id].entry.work_size()) {
                self.stop_pending(id, FrontierReason::Work);
                break;
            }
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
        if let Some(cache) = self.cache.take() {
            cache.finish(&mut self.result);
        }
        self.result.status = if self.result.frontiers.is_empty() {
            Status::Converged
        } else {
            Status::Incomplete
        };
        self.result.edges = std::mem::take(&mut self.edges).into_iter().collect();
        self.result.diagnostics = std::mem::take(&mut self.diagnostics).into_iter().collect();
        self.result.work = self.budget.used();
    }

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
        self.edges.insert(MachineEdge {
            from,
            to,
            kind: successor.kind,
        });
        Some(to)
    }

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
