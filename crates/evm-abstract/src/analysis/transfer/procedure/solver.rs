//! Joint known/unknown procedure worklist and per-caller return propagation.

use super::coarse_retry::{CoarseRuntime, RuntimeRetry};
use super::{
    AbstractValue, Arc, BTreeMap, BTreeSet, ByteArray, CallStack, DiagnosticKind, Execution,
    ExecutionFrontier, FrameCode, FrontierReason, Limit, MachineEdgeKind, MachineError,
    MachinePayload, Node, OutcomeKind, Output, PayloadSummary, Procedure, RootFrame, Solver, Store,
    Subscriber, execute_inner, form, instantiate, join, resume_summary_inner, semantic_eq,
};

impl Solver<'_> {
    fn form(&mut self, payload: &MachinePayload) -> Result<PayloadSummary, MachineError> {
        let mut refined = payload.clone();
        let mut projection = super::projection::Projection::new(
            &payload.relations,
            self.domain,
            &self.config.analysis.relations,
            self.budget,
        );
        projection.payload(&mut refined);
        projection.finish()?;
        let history_work = refined.call_stack.iter().fold(0usize, |work, frame| {
            work.saturating_add(if frame.code == FrameCode::Unknown {
                0
            } else {
                super::context::work_size(frame.key.jump_history.len())
            })
        });
        self.reserve(
            history_work
                .saturating_add(super::payload::alias_form_work(&refined))
                .saturating_add(super::storage_observations::form_work(
                    &refined,
                    self.domain,
                )),
        )?;
        form(&refined, self.entry, self.domain)
    }

    pub(super) fn reserve(&mut self, cost: usize) -> Result<(), MachineError> {
        MachineError::check_cancelled()?;
        if cost == usize::MAX || !self.budget.charge(cost) {
            Err(MachineError::work_failure())
        } else {
            Ok(())
        }
    }

    pub(super) fn frontier(&mut self, payload: MachinePayload, reason: FrontierReason, pc: usize) {
        self.table.frontiers.push(ExecutionFrontier {
            pc,
            reason,
            target: Some(payload.key()),
            range: None,
            payload: Some(Arc::new(payload)),
        });
    }

    pub(super) fn frontier_summary(
        &mut self,
        payload: PayloadSummary,
        reason: FrontierReason,
        pc: usize,
    ) {
        // A pre-join input cannot witness the current invariant. Materialize
        // its complete current bound only when the shared ledger permits it;
        // a location without payload is honest evidence when construction fails.
        let current = self
            .reserve(payload.work_size().saturating_mul(4))
            .and_then(|()| instantiate(&payload, self.domain))
            .ok();
        let target = current
            .as_ref()
            .map_or(payload.location, MachinePayload::key);
        self.table.frontiers.push(ExecutionFrontier {
            pc,
            reason,
            target: Some(target),
            range: None,
            payload: current.map(Arc::new),
        });
    }

    pub(super) fn park_pending(&mut self, reason: FrontierReason) {
        let pending = self.table.queued.iter().copied().collect::<Vec<_>>();
        for pending in pending {
            self.frontier_summary(self.table.nodes[pending].payload.clone(), reason.clone(), 0);
        }
    }

    pub(super) fn procedure(&mut self, payload: MachinePayload) -> Result<usize, MachineError> {
        self.procedure_from(payload, None)
    }

    fn procedure_from(
        &mut self,
        payload: MachinePayload,
        from: Option<usize>,
    ) -> Result<usize, MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        let payload = self.form(&payload)?;
        let key = payload.active().key.clone();
        if let Some(id) = self.table.procedures_by_key.get(&key).copied() {
            self.table.reused += 1;
            if let Some(from) = from {
                self.call_dependency(from, id)?;
            }
            self.enqueue_summary(id, payload, None, None)?;
            return Ok(id);
        }
        let id = self.table.procedures.len();
        if self.counters.states >= self.config.analysis.max_states {
            return Err(MachineError::States);
        }
        self.table.procedures_by_key.insert(key.clone(), id);
        self.table.procedures.push(Procedure {
            frame: key,
            entry: self.table.nodes.len(),
            nodes: BTreeMap::new(),
            outputs: BTreeMap::new(),
            subscribers: Vec::new(),
            dependencies: BTreeSet::new(),
            recursive: false,
            local_cycle: false,
            output_updates: 0,
            covered_blocks: BTreeSet::new(),
        });
        if let Err(error) = self.enqueue_summary(id, payload, None, None) {
            let failed = self
                .table
                .procedures
                .pop()
                .expect("just allocated procedure");
            self.table.procedures_by_key.remove(&failed.frame);
            return Err(error);
        }
        if let Some(from) = from {
            self.call_dependency(from, id)?;
        }
        Ok(id)
    }

    pub(super) fn enqueue(
        &mut self,
        procedure: usize,
        payload: MachinePayload,
        resume_status: Option<bool>,
        from: usize,
    ) -> Result<usize, MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        let payload = self.form(&payload)?;
        self.enqueue_summary(procedure, payload, resume_status, Some(from))
    }

    pub(super) fn enqueue_summary(
        &mut self,
        procedure: usize,
        payload: PayloadSummary,
        resume_status: Option<bool>,
        from: Option<usize>,
    ) -> Result<usize, MachineError> {
        let key = (payload.active().key.clone(), resume_status);
        let id = if let Some(id) = self.table.procedures[procedure].nodes.get(&key).copied() {
            if let Some(from) = from {
                self.node_edge(from, id)?;
            }
            let old = &self.table.nodes[id];
            let widen = self.node_feedback(id)
                && old.updates >= self.domain.spec().widening_after_updates();
            let widened = if let RuntimeRetry::Coarse(coarse) = &old.runtime_retry {
                coarse.join(&old.payload, &payload, self.domain, self.budget, widen)?
            } else {
                let cost = old
                    .payload
                    .work_size()
                    .saturating_add(payload.work_size())
                    .saturating_mul(4);
                self.reserve(cost)?;
                let old = &self.table.nodes[id];
                join(&old.payload, &payload, self.domain, widen)?
            };
            let old = &self.table.nodes[id];
            if semantic_eq(&widened, &old.payload) {
                return Ok(id);
            }
            let old = &mut self.table.nodes[id];
            old.payload = widened;
            old.updates += 1;
            id
        } else {
            if self.counters.states >= self.config.analysis.max_states {
                return Err(MachineError::States);
            }
            if let Some(from) = from
                && self.table.nodes[from].procedure != procedure
            {
                return Err(MachineError::Shape);
            }
            // A fresh target has no outgoing edge and cannot close a cycle.
            // Reserve its graph edge together with the node before installation.
            self.reserve(
                payload
                    .work_size()
                    .saturating_add(usize::from(from.is_some())),
            )?;
            let id = self.table.nodes.len();
            self.table.nodes.push(Node {
                procedure,
                payload,
                updates: 0,
                successors: BTreeSet::new(),
                loop_head: false,
                runtime_retry: RuntimeRetry::Fresh,
            });
            self.table.procedures[procedure].nodes.insert(key, id);
            self.counters.states += 1;
            if let Some(from) = from {
                self.table.nodes[from].successors.insert(id);
            }
            id
        };
        if self.table.queued.insert(id) {
            self.table.queue.push_back(id);
        }
        Ok(id)
    }

    pub(super) fn run(&mut self) {
        while let Some(id) = self.table.queue.pop_front() {
            if !self.table.queued.remove(&id) {
                continue;
            }
            if self.counters.transfers >= self.config.analysis.max_transfers
                || self.budget.exhausted()
            {
                let reason = if self.budget.exhausted() {
                    MachineError::work_failure().frontier_reason()
                } else {
                    FrontierReason::Budget(Limit::Transfers)
                };
                self.frontier_summary(self.table.nodes[id].payload.clone(), reason.clone(), 0);
                self.park_pending(reason);
                break;
            }
            let input = self.table.nodes[id].payload.clone();
            if let Err(error) = self.reserve(input.work_size().saturating_mul(4)) {
                let reason = error.frontier_reason();
                self.frontier_summary(input, reason.clone(), 0);
                self.park_pending(reason);
                break;
            }
            let payload = match instantiate(&input, self.domain) {
                Ok(payload) => payload,
                Err(error) => {
                    self.frontier_summary(input, error.frontier_reason(), 0);
                    continue;
                }
            };
            self.counters.transfers += 1;
            self.counters.report(self.budget.used());
            let execution = if payload.active().code == FrameCode::Unknown {
                self.environment(payload)
            } else {
                execute_inner(
                    self.world,
                    self.entry,
                    payload,
                    self.config,
                    self.domain,
                    self.budget,
                    self.storage_reads,
                    true,
                )
            };
            let execution = self.retry_runtime(id, &input, execution);
            if !execution.executed_pcs.is_empty() {
                let procedure = self.table.nodes[id].procedure;
                self.table.procedures[procedure]
                    .covered_blocks
                    .insert(self.table.nodes[id].payload.active().key.basic_block_index);
            }
            self.accept(id, execution);
        }
    }

    fn retry_admission(&self) -> Result<(), FrontierReason> {
        MachineError::check_cancelled().map_err(MachineError::frontier_reason)?;
        if self.budget.exhausted() {
            Err(MachineError::work_failure().frontier_reason())
        } else if self.counters.transfers >= self.config.analysis.max_transfers {
            Err(FrontierReason::Budget(Limit::Transfers))
        } else {
            Ok(())
        }
    }

    pub(super) fn retry_runtime(
        &mut self,
        id: usize,
        original: &PayloadSummary,
        mut first: Execution,
    ) -> Execution {
        if !matches!(&self.table.nodes[id].runtime_retry, RuntimeRetry::Fresh)
            || original.active().code != FrameCode::Runtime
            || first.retention_failures.is_empty()
            // A mixed execution has independent mandatory obligations. This
            // slice admits only complete, raw-typed retention refusals.
            || first.retention_failures.len() != first.frontiers.len()
            || first.retention_failures.iter().enumerate()
                .any(|(index, (frontier, _))| index != *frontier)
        {
            return first;
        }
        // Only the attempt bit may survive failed preparation. No execution has
        // reached accept/publish, and the original immutable checkpoint is used.
        self.table.nodes[id].runtime_retry = RuntimeRetry::Attempted;
        let prepared = self
            .retry_admission()
            .and_then(|()| {
                CoarseRuntime::prepare(original, self.domain, self.budget)
                    .map_err(MachineError::frontier_reason)
            })
            .and_then(|prepared| {
                self.retry_admission()?;
                Ok(prepared)
            });
        let (coarse, bound, payload) = match prepared {
            Ok(prepared) => prepared,
            Err(reason) => {
                super::super::boundary(&mut first, 0, reason);
                return first;
            }
        };
        self.counters.transfers += 1;
        self.counters.report(self.budget.used());
        let mut retry = execute_inner(
            self.world,
            self.entry,
            payload,
            self.config,
            self.domain,
            self.budget,
            self.storage_reads,
            true,
        );
        if retry.frontiers.is_empty() {
            // The coarse transfer covers the full original entry. Commit its
            // input and receipts together; a failed retry leaves only Attempted.
            self.table.nodes[id].payload = bound;
            self.table.nodes[id].runtime_retry = RuntimeRetry::Coarse(coarse);
            retry.diagnostics.push((
                0,
                DiagnosticKind::RelationalPrecisionLost(
                    crate::domain::relational::QueryReason::ExpressionLimit,
                ),
            ));
        }
        super::coarse_retry::reconcile(first, retry)
    }

    pub(super) fn accept(&mut self, from: usize, execution: Execution) {
        let procedure = self.table.nodes[from].procedure;
        self.table
            .precision_losses
            .extend(execution.diagnostics.into_iter().filter_map(|(_, kind)| {
                matches!(kind, DiagnosticKind::RelationalPrecisionLost(_)).then_some(kind)
            }));
        self.table.frontiers.extend(execution.frontiers);
        for outcome in execution.outcomes {
            if let Err(error) = self.publish(procedure, outcome.kind, outcome.data, outcome.store) {
                self.frontier_summary(
                    self.table.nodes[from].payload.clone(),
                    error.frontier_reason(),
                    0,
                );
            }
        }
        for successor in execution.successors {
            let original = successor.payload.clone();
            let operation = if successor.kind == MachineEdgeKind::Call {
                self.subscribe(from, successor.payload)
            } else {
                let resume_status = if successor.payload.active().code == FrameCode::Runtime
                    && matches!(
                        successor.kind,
                        MachineEdgeKind::Return
                            | MachineEdgeKind::Revert
                            | MachineEdgeKind::Failure
                    ) {
                    successor
                        .payload
                        .active()
                        .stack
                        .last()
                        .and_then(AbstractValue::singleton)
                        .filter(|status| *status <= alloy_primitives::U256::ONE)
                        .map(|status| status == alloy_primitives::U256::ONE)
                } else {
                    None
                };
                // This finite partition lasts exactly the first resumed block,
                // where bytecode normally tests CALL status. Its successors use
                // their actual new edge kinds and return to ordinary node keys.
                self.enqueue(procedure, successor.payload, resume_status, from)
                    .map(|_| ())
            };
            if let Err(error) = operation {
                self.frontier(original, error.frontier_reason(), 0);
            }
        }
    }

    pub(super) fn subscribe(
        &mut self,
        from: usize,
        payload: MachinePayload,
    ) -> Result<(), MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        let callee_payload = MachinePayload {
            call_stack: CallStack::new(RootFrame {
                state: payload.active().clone(),
            }),
            store: payload.store.clone(),
            relations: payload.relations.clone(),
        };
        let callee = self.procedure_from(callee_payload, Some(from))?;
        let payload = self.form(&payload)?;
        let key = (from, callee);
        let subscription =
            if let Some(subscription) = self.table.subscribers_by_key.get(&key).copied() {
                self.reserve(
                    self.table.subscribers[subscription]
                        .payload
                        .work_size()
                        .saturating_add(payload.work_size())
                        .saturating_mul(4),
                )?;
                let old = &self.table.subscribers[subscription];
                let grown = join(
                    &old.payload,
                    &payload,
                    self.domain,
                    self.node_feedback(from)
                        && old.updates >= self.domain.spec().widening_after_updates(),
                )?;
                if semantic_eq(&grown, &old.payload) {
                    return Ok(());
                }
                let old = &mut self.table.subscribers[subscription];
                old.payload = grown;
                old.updates += 1;
                old.delivered.clear();
                subscription
            } else {
                self.reserve(payload.work_size())?;
                let subscription = self.table.subscribers.len();
                self.table.subscribers.push(Subscriber {
                    from,
                    callee,
                    payload,
                    updates: 0,
                    delivered: BTreeMap::new(),
                });
                self.table.subscribers_by_key.insert(key, subscription);
                self.table.procedures[callee].subscribers.push(subscription);
                subscription
            };
        let kinds = self.table.procedures[callee]
            .outputs
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for kind in kinds {
            self.deliver(subscription, kind)?;
        }
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        procedure: usize,
        kind: OutcomeKind,
        data: ByteArray,
        store: Store,
    ) -> Result<(), MachineError> {
        self.reserve(
            store
                .work_size()
                .saturating_add(data.work_size())
                .saturating_mul(4),
        )?;
        let store = store.summary_projection(self.domain)?;
        let data = data.summary_projection(self.domain)?;
        let output = if let Some(old) = self.table.procedures[procedure].outputs.get(&kind) {
            let feedback = self.table.procedures[procedure].recursive
                || self.table.procedures[procedure].local_cycle;
            let widen = feedback && old.generation > self.domain.spec().widening_after_updates();
            let joined_store = old.store.join(&store, self.domain, widen);
            let joined_data = old.data.join(&data, self.domain, widen);
            if old.store == joined_store && old.data == joined_data {
                return Ok(());
            }
            let work = joined_store
                .work_size()
                .saturating_add(joined_data.work_size());
            Output {
                store: joined_store,
                data: joined_data,
                work,
                generation: old.generation + 1,
            }
        } else {
            let work = store.work_size().saturating_add(data.work_size());
            Output {
                store,
                data,
                work,
                generation: 1,
            }
        };
        self.table.procedures[procedure]
            .outputs
            .insert(kind, output);
        self.table.procedures[procedure].output_updates += 1;
        let subscribers = self.table.procedures[procedure].subscribers.clone();
        for subscriber in subscribers {
            self.deliver(subscriber, kind)?;
        }
        Ok(())
    }

    pub(super) fn deliver(
        &mut self,
        subscriber: usize,
        kind: OutcomeKind,
    ) -> Result<(), MachineError> {
        let subscription = &self.table.subscribers[subscriber];
        let output = &self.table.procedures[subscription.callee].outputs[&kind];
        if subscription.delivered.get(&kind).copied() == Some(output.generation) {
            return Ok(());
        }
        let from = subscription.from;
        let generation = output.generation;
        if subscription.payload.frames.len() == 2
            && subscription.payload.frames[0].code == FrameCode::Unknown
            && subscription.payload.continuations[0].creation.is_none()
            && subscription.payload.continuations[0]
                .output_size
                .singleton()
                == Some(alloy_primitives::U256::ZERO)
        {
            // The environment observes callback termination, then may perform
            // arbitrary further steps. Its numerical form intentionally erases
            // the result stack, private memory and returndata. Advance that
            // same bound directly; reconstructing erased arrays adds no facts.
            let work =
                subscription
                    .payload
                    .work_size()
                    .saturating_add(if kind == OutcomeKind::Return {
                        output.store.work_size()
                    } else {
                        subscription.payload.active().saved_store.work_size()
                    });
            self.reserve(work.saturating_mul(4))?;
            let subscription = &self.table.subscribers[subscriber];
            let mut resumed = subscription.payload.clone();
            let child = resumed.frames.pop().expect("a subscribed child");
            resumed.continuations.pop();
            resumed.location.frames.pop();
            resumed.store = if kind == OutcomeKind::Return {
                self.table.procedures[subscription.callee].outputs[&kind]
                    .store
                    .clone()
            } else {
                child.saved_store
            };
            // Changing the active frame and Store invalidates current-read templates.
            resumed.storage_observations = Default::default();
            resumed.work = resumed.measure_work();
            self.table.subscribers[subscriber]
                .delivered
                .insert(kind, generation);
            self.table.propagations += 1;
            return self
                .enqueue_summary(self.table.nodes[from].procedure, resumed, None, Some(from))
                .map(|_| ());
        }
        let template = subscription.payload.clone();
        let store = output.store.clone();
        let data = output.data.clone();
        self.reserve(
            template
                .work_size()
                .saturating_add(output.work)
                .saturating_mul(4),
        )?;
        let mut payload = instantiate(&template, self.domain)?;
        // Installed constraints still refer only to the old immutable snapshot.
        // Never reinstall those read bounds after replacing the Store.
        payload.store = store.instantiate(self.domain)?;
        let data = data.instantiate(self.domain)?;
        let execution = resume_summary_inner(
            payload,
            kind,
            data,
            self.config,
            self.domain,
            self.budget,
            true,
        );
        self.table.subscribers[subscriber]
            .delivered
            .insert(kind, generation);
        self.table.propagations += 1;
        self.accept(from, execution);
        Ok(())
    }
}
