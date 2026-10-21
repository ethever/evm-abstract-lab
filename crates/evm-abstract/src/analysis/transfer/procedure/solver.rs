//! Joint known/unknown procedure worklist and per-caller return propagation.

use super::Composition;
use super::admission::SummaryAdmission;
use crate::analysis::engine::{Arrival, ContextPolicy, PartitionPolicy};
use crate::analysis::worklist::{MachineMeter, kernel_failure, runtime::TaskKind};
use crate::domain::EvmNumericAlgebra;
use analysis_kernel::{Availability, Change, PublishError, RunningJob, Version};

use super::coarse_retry::{self, EvmRuntimeRetry};
use super::{
    Arc, BTreeMap, BTreeSet, DiagnosticKind, EvmByteArray, EvmCallStack, EvmExecution,
    EvmExecutionFrontier, EvmMachinePayload, EvmNode, EvmOutput, EvmPayloadSummary, EvmProcedure,
    EvmRootFrame, EvmSolver, EvmStore, EvmSubscriber, EvmValue, FrameCode, FrontierReason, Limit,
    MachineEdgeKind, MachineError, OutcomeKind, execute_inner_with, instantiate_with,
    resume_summary_inner, semantic_eq, transfer_stop,
};

impl<D: EvmNumericAlgebra, C: ContextPolicy, P: PartitionPolicy, X: Composition<D::Element, E>, E>
    EvmSolver<'_, D, C, P, X, E>
{
    fn form(
        &mut self,
        payload: &EvmMachinePayload<D::Element>,
    ) -> Result<EvmPayloadSummary<D::Element>, MachineError> {
        let mut refined = payload.clone();
        let mut projection = super::projection::Projection::new(
            &payload.relations,
            self.domain,
            &self.config.analysis.relations,
            self.budget,
            super::projection::Policy::Summary,
        );
        projection.payload(&mut refined)?;
        projection.finish()?;
        let history_work = refined.call_stack.iter().fold(0usize, |work, frame| {
            work.saturating_add(if frame.code == FrameCode::Unknown {
                0
            } else {
                self.policies
                    .context
                    .normalization_work(frame.key.jump_history.len())
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
        for frame in refined.call_stack.iter_mut() {
            if frame.code != FrameCode::Unknown {
                self.policies.context.normalize(&mut frame.key.jump_history);
            }
        }
        super::payload::form_normalized(&refined, self.entry, self.domain)
    }

    pub(super) fn reserve(&mut self, cost: usize) -> Result<(), MachineError> {
        MachineError::check_cancelled()?;
        if cost == usize::MAX || !self.budget.charge(cost) {
            Err(MachineError::work_failure())
        } else {
            Ok(())
        }
    }

    pub(super) fn frontier(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        reason: FrontierReason,
        pc: usize,
    ) {
        self.table.frontiers.push(EvmExecutionFrontier {
            pc,
            reason,
            target: Some(payload.key()),
            range: None,
            payload: Some(Arc::new(payload)),
        });
    }

    fn frontier_ref(
        &mut self,
        payload: &EvmPayloadSummary<D::Element>,
        reason: FrontierReason,
        pc: usize,
    ) {
        let frontier = parked_bound(payload, reason, pc, self.domain, self.budget);
        self.table.frontiers.push(frontier);
    }

    #[cfg(test)]
    pub(super) fn frontier_summary(
        &mut self,
        payload: EvmPayloadSummary<D::Element>,
        reason: FrontierReason,
        pc: usize,
    ) {
        self.frontier_ref(&payload, reason, pc);
    }

    pub(super) fn frontier_node(&mut self, id: usize, reason: FrontierReason, pc: usize) {
        let frontier = parked_bound(
            &self.table.nodes[id].payload,
            reason,
            pc,
            self.domain,
            self.budget,
        );
        self.table.frontiers.push(frontier);
    }

    fn frontier_subscriber(&mut self, id: usize, reason: FrontierReason, pc: usize) {
        let frontier = parked_bound(
            &self.table.subscribers[id].payload,
            reason,
            pc,
            self.domain,
            self.budget,
        );
        self.table.frontiers.push(frontier);
    }

    fn park_admitted(
        &mut self,
        task: &TaskKind<<X as crate::analysis::cooperation::InputTasks>::Task>,
        reason: FrontierReason,
    ) {
        match task {
            TaskKind::Body(id) => self.frontier_node(*id, reason.clone(), 0),
            TaskKind::Resume(id) => self.frontier_subscriber(*id, reason.clone(), 0),
            _ => self.table.frontiers.push(EvmExecutionFrontier {
                pc: 0,
                reason: reason.clone(),
                target: None,
                range: None,
                payload: None,
            }),
        }
        self.park_pending(reason);
    }

    pub(super) fn park_pending(&mut self, reason: FrontierReason) {
        let pending = self.runtime.pending_relative().collect::<Vec<_>>();
        for task in pending {
            match task {
                TaskKind::Body(id) => self.frontier_node(id, reason.clone(), 0),
                TaskKind::Resume(id) => self.frontier_subscriber(id, reason.clone(), 0),
                _ => {}
            }
        }
    }

    pub(super) fn procedure(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
    ) -> Result<usize, MachineError> {
        self.procedure_from(payload, None)
    }

    fn procedure_from(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        from: Option<usize>,
    ) -> Result<usize, MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        let payload = self.form(&payload)?;
        let key = payload.active().key.clone();
        if let Some(id) = self
            .table
            .procedures_by_key
            .get(&(self.namespace, key.clone()))
            .copied()
        {
            self.table.reused += 1;
            if let Some(from) = from {
                self.call_dependency(from, id)?;
            }
            self.enqueue_summary(id, payload, None, None)?;
            self.observe_procedure(id)?;
            return Ok(id);
        }
        let id = self.table.procedures.len();
        if self.counters.states >= self.config.analysis.max_states {
            return Err(MachineError::States);
        }
        let scope = self.runtime.relative;
        let output = self
            .runtime
            .kernel()
            .slot(scope, &mut MachineMeter(self.budget))
            .map_err(kernel_failure)?;
        self.table
            .procedures_by_key
            .insert((self.namespace, key.clone()), id);
        self.table.procedures.push(EvmProcedure {
            namespace: self.namespace,
            frame: key,
            entry: self.table.nodes.len(),
            nodes: BTreeMap::new(),
            outputs: BTreeMap::new(),
            output,
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
            self.table
                .procedures_by_key
                .remove(&(self.namespace, failed.frame));
            return Err(error);
        }
        if let Some(from) = from {
            self.call_dependency(from, id)?;
        }
        self.observe_procedure(id)?;
        Ok(id)
    }

    pub(super) fn enqueue(
        &mut self,
        procedure: usize,
        payload: EvmMachinePayload<D::Element>,
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
        payload: EvmPayloadSummary<D::Element>,
        resume_status: Option<bool>,
        from: Option<usize>,
    ) -> Result<usize, MachineError> {
        let key = (
            payload.active().key.clone(),
            self.policies.partition.classify(Arrival { resume_status }),
        );
        let previous_updates = self.table.procedures[procedure]
            .nodes
            .get(&key)
            .map(|id| self.table.nodes[*id].updates);
        let id = if let Some(id) = self.table.procedures[procedure].nodes.get(&key).copied() {
            if let Some(from) = from {
                self.node_edge(from, id)?;
            }
            let old = &self.table.nodes[id];
            let widen = self.node_feedback(id)
                && old.updates >= self.domain.spec().widening_after_updates();
            if let EvmRuntimeRetry::Coarse(coarse) = &old.runtime_retry {
                let widened =
                    coarse.join(&old.payload, &payload, self.domain, self.budget, widen)?;
                let changed = !semantic_eq(&widened, &old.payload);
                let updates = if changed {
                    old.updates
                        .checked_add(1)
                        .ok_or(MachineError::IdentityExhausted)?
                } else {
                    old.updates
                };
                let work = widened.work_size().saturating_add(1);
                let old = &mut self.table.nodes[id];
                self.runtime.with_run(
                    self.active
                        .as_mut()
                        .or(self.caller.as_deref_mut())
                        .ok_or(MachineError::Shape)?,
                    self.budget,
                    |run| {
                        run.publish_prepared_parts(
                            &old.input,
                            &mut old.payload,
                            widened,
                            &mut old.updates,
                            updates,
                            if changed {
                                Change::Changed
                            } else {
                                Change::Unchanged
                            },
                            work,
                        )
                        .map_err(kernel_failure)
                    },
                )?;
            } else {
                let old = &mut self.table.nodes[id];
                let policy = SummaryAdmission {
                    domain: self.domain,
                    widen,
                };
                self.runtime.with_run(
                    self.active
                        .as_mut()
                        .or(self.caller.as_deref_mut())
                        .ok_or(MachineError::Shape)?,
                    self.budget,
                    |run| {
                        run.admit(
                            &old.input,
                            &mut old.payload,
                            &mut old.updates,
                            &payload,
                            &policy,
                        )
                        .map_err(admission_failure)
                    },
                )?;
            }
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
            let scope = self.runtime.relative;
            let input = self
                .runtime
                .kernel()
                .slot(scope, &mut MachineMeter(self.budget))
                .map_err(kernel_failure)?;
            self.runtime
                .register_relative(TaskKind::Body(id), self.budget)?;
            self.table.nodes.push(EvmNode {
                procedure,
                input,
                payload,
                updates: 0,
                successors: BTreeSet::new(),
                loop_head: false,
                runtime_retry: EvmRuntimeRetry::Fresh,
                excluded: false,
            });
            self.table.procedures[procedure].nodes.insert(key, id);
            self.counters.states += 1;
            if let Some(from) = from {
                self.table.nodes[from].successors.insert(id);
            }
            id
        };
        if X::ENABLED {
            // Only genuine native-input growth reopens completion. Active jobs
            // and selector movement alone cannot force repeated close/reset cycles.
            if previous_updates != Some(self.table.nodes[id].updates) {
                self.observe_summary(procedure, false)?;
            }
        }
        Ok(id)
    }

    pub(super) fn run(&mut self) {
        loop {
            let (task, token) = match self.runtime.next(true, self.budget) {
                Ok(Some(work)) => work,
                Ok(None) => break,
                Err(error) => {
                    self.park_pending(error.frontier_reason());
                    break;
                }
            };
            if !self.run_admitted(task, token) {
                break;
            }
        }
    }

    pub(super) fn run_admitted(
        &mut self,
        task: Arc<
            crate::analysis::worklist::runtime::Task<
                <X as crate::analysis::cooperation::InputTasks>::Task,
            >,
        >,
        token: RunningJob,
    ) -> bool {
        let procedure = match &task.0 {
            TaskKind::Body(id) => Some(self.table.nodes[*id].procedure),
            TaskKind::Resume(id) => {
                Some(self.table.nodes[self.table.subscribers[*id].from].procedure)
            }
            _ => None,
        };
        if let Some(id) = procedure {
            self.namespace = self.table.procedures[id].namespace;
        }
        self.active = Some(token);
        self.deferred = false;
        self.consumed = None;
        let native_owner = matches!(&task.0, TaskKind::Body(_) | TaskKind::Resume(_));
        let evaluated_stamp = match self.source_stamp(&task.0) {
            Ok(stamp) => stamp,
            Err(error) => {
                self.park_admitted(&task.0, error.frontier_reason());
                return false;
            }
        };
        let frontiers_before = self.table.frontiers.len();
        if native_owner
            && let Err(error) = self.cooperation.begin_owner(
                self.runtime,
                self.active.as_mut().expect("admitted native owner"),
                self.budget,
            )
        {
            let error = self.cooperation.stop(error, self.budget);
            self.park_admitted(&task.0, error.frontier_reason());
            return false;
        }
        let result = match &task.0 {
            TaskKind::Body(id) => self.step(*id),
            TaskKind::Resume(subscriber) => match self.resume(*subscriber) {
                Ok(complete) => complete,
                Err(error) => {
                    let reason = error.frontier_reason();
                    self.frontier_subscriber(*subscriber, reason.clone(), 0);
                    self.park_pending(reason);
                    false
                }
            },
            TaskKind::Extension(_) => {
                let token = self.active.take().expect("external evaluation admitted");
                if let Err(error) =
                    self.cooperation
                        .evaluate(self.runtime, Arc::clone(&task), token, self.budget)
                {
                    let error = self.cooperation.stop(error, self.budget);
                    self.park_pending(error.frontier_reason());
                    false
                } else {
                    true
                }
            }
            TaskKind::SummaryObservation => match self.complete_observations() {
                Ok(()) => true,
                Err(error) => {
                    let reason = error.frontier_reason();
                    self.park_pending(reason.clone());
                    self.table.frontiers.push(EvmExecutionFrontier {
                        pc: 0,
                        reason,
                        target: None,
                        range: None,
                        payload: None,
                    });
                    false
                }
            },
            TaskKind::World { .. } | TaskKind::Seed(_) => {
                unreachable!("uninitialized or foreign task")
            }
        };
        if !result {
            return false;
        }
        let same_source = match self.source_stamp(&task.0) {
            Ok(stamp) => stamp == evaluated_stamp,
            Err(error) => {
                self.park_admitted(&task.0, error.frontier_reason());
                return false;
            }
        };
        if native_owner
            && !self.deferred
            && same_source
            && self.table.frontiers.len() == frontiers_before
            && let Err(error) = self.cooperation.finish_owner(
                self.runtime,
                self.active.as_mut().expect("complete native owner"),
                self.budget,
                self.consumed,
            )
        {
            let error = self.cooperation.stop(error, self.budget);
            self.park_pending(error.frontier_reason());
            return false;
        }
        if native_owner
            && X::ENABLED
            && let Err(error) = self.runtime.schedule_observation(self.budget)
        {
            let reason = error.frontier_reason();
            match &task.0 {
                TaskKind::Body(id) => self.frontier_node(*id, reason.clone(), 0),
                TaskKind::Resume(id) => self.frontier_subscriber(*id, reason.clone(), 0),
                _ => {}
            }
            self.park_pending(reason);
            return false;
        }
        if let Some(token) = self.active.take()
            && let Err(error) = self.runtime.finish(token, self.budget)
        {
            self.park_pending(error.frontier_reason());
            return false;
        }
        true
    }

    /// Perform one admitted body obligation under the transaction ledger.
    ///
    /// Scheduling owns the popped identity. Every interrupted preparation
    /// retains its current bound before the remaining obligations are parked;
    /// callers can therefore share this step with other tracked task families
    /// without losing the currently executing procedure state.
    pub(super) fn step(&mut self, id: usize) -> bool {
        if let Some(reason) = transfer_stop(
            self.counters.transfers,
            self.config.analysis.max_transfers,
            self.budget,
        ) {
            self.frontier_node(id, reason.clone(), 0);
            self.park_pending(reason);
            return false;
        }
        let work = self.table.nodes[id].payload.work_size().saturating_mul(4);
        if let Err(error) = self.reserve(work) {
            let reason = error.frontier_reason();
            self.frontier_node(id, reason.clone(), 0);
            self.park_pending(reason);
            return false;
        }
        let input = self.table.nodes[id].payload.clone();
        let execution = match instantiate_with(&input, self.domain) {
            Ok(payload) => {
                let payload = match self.cooperative_input(id, payload, 0) {
                    Ok(Some(payload)) => payload,
                    Ok(None) => {
                        return self.track_input(id).is_ok();
                    }
                    Err(error) => {
                        let reason = error.frontier_reason();
                        self.frontier_ref(&input, reason.clone(), 0);
                        self.park_pending(reason);
                        return false;
                    }
                };
                self.counters.transfers += 1;
                self.counters.report(self.budget.used());
                let execution = if payload.active().code == FrameCode::Unknown {
                    self.environment(payload)
                } else {
                    execute_inner_with(
                        self.world,
                        self.entry,
                        payload,
                        self.config,
                        self.domain,
                        &self.policies.context,
                        self.budget,
                        self.storage_reads,
                        true,
                    )
                };
                self.retry_runtime(id, &input, execution)
            }
            Err(error) => {
                let Some(execution) = self.retry_instantiation(id, &input, error) else {
                    return true;
                };
                execution
            }
        };
        // No other body runs during this interpreter step. A successful
        // retention promotion has already executed its new covering bound, so
        // subscribe to that bound now; subsequent outgoing joins still wake us.
        let version = match self.track_input(id) {
            Ok(version) => version,
            Err(error) => {
                let reason = error.frontier_reason();
                self.frontier_node(id, reason.clone(), 0);
                self.park_pending(reason);
                return false;
            }
        };
        self.producer = Some(super::Producer {
            kind: 0,
            id,
            case: 0,
            input: version,
            other: None,
            parent: self.consumed,
        });
        if !execution.executed_pcs.is_empty() {
            let procedure = self.table.nodes[id].procedure;
            self.table.procedures[procedure]
                .covered_blocks
                .insert(self.table.nodes[id].payload.active().key.basic_block_index);
        }
        self.accept(id, execution);
        true
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

    /// A failed rehydration has no executable checkpoint. Retry only from the
    /// complete immutable node summary, and retain its original obligation if
    /// either preparation or the replacement transfer fails.
    pub(super) fn retry_instantiation(
        &mut self,
        id: usize,
        original: &EvmPayloadSummary<D::Element>,
        error: MachineError,
    ) -> Option<EvmExecution<D::Element>> {
        let retry = (matches!(&self.table.nodes[id].runtime_retry, EvmRuntimeRetry::Fresh)
            && coarse_retry::supports(original)
            && super::coarse_retry::retention_refusal(&error))
        .then(|| self.execute_coarse(id, original));
        if let Some(Ok(execution)) = &retry
            && execution.frontiers.is_empty()
        {
            return retry.and_then(Result::ok);
        }
        self.frontier_ref(original, error.frontier_reason(), 0);
        let interruptions = match retry {
            Some(Ok(execution)) => execution
                .frontiers
                .into_iter()
                .map(|frontier| frontier.reason)
                .collect::<Vec<_>>(),
            Some(Err(reason)) => vec![reason],
            None => Vec::new(),
        };
        for reason in interruptions {
            if super::coarse_retry::interruption(&reason) {
                self.frontier_ref(original, reason, 0);
            }
        }
        None
    }

    pub(super) fn retry_runtime(
        &mut self,
        id: usize,
        original: &EvmPayloadSummary<D::Element>,
        mut first: EvmExecution<D::Element>,
    ) -> EvmExecution<D::Element> {
        if !matches!(&self.table.nodes[id].runtime_retry, EvmRuntimeRetry::Fresh)
            || !coarse_retry::supports(original)
            || first.retention_failures.is_empty()
            // A mixed execution has independent mandatory obligations. This
            // slice admits only complete, raw-typed retention refusals.
            || first.retention_failures.len() != first.frontiers.len()
            || first.retention_failures.iter().enumerate()
                .any(|(index, (frontier, _))| index != *frontier)
        {
            return first;
        }
        let first_receipt = self.consumed;
        match self.execute_coarse(id, original) {
            Ok(retry) => {
                if !retry.frontiers.is_empty() {
                    self.consumed = first_receipt;
                }
                super::coarse_retry::reconcile(first, retry)
            }
            Err(reason) => {
                self.consumed = first_receipt;
                if super::coarse_retry::interruption(&reason) {
                    super::super::boundary(&mut first, 0, reason);
                }
                first
            }
        }
    }

    fn execute_coarse(
        &mut self,
        id: usize,
        original: &EvmPayloadSummary<D::Element>,
    ) -> Result<EvmExecution<D::Element>, FrontierReason> {
        // Only the attempt bit may survive failed preparation. No execution has
        // reached accept/publish, and the original immutable checkpoint is used.
        self.table.nodes[id].runtime_retry = EvmRuntimeRetry::Attempted;
        let coarse_retry::Prepared {
            representation: coarse,
            bound,
            payload,
        } = self
            .retry_admission()
            .and_then(|()| {
                coarse_retry::prepare_with(original, self.domain, self.budget)
                    .map_err(MachineError::frontier_reason)
            })
            .and_then(|prepared| {
                self.retry_admission()?;
                Ok(prepared)
            })?;
        self.counters.transfers += 1;
        self.counters.report(self.budget.used());
        let payload = match self
            .cooperative_input(id, payload, 1)
            .map_err(MachineError::frontier_reason)?
        {
            Some(payload) => payload,
            None => {
                if self.deferred {
                    self.table.nodes[id].runtime_retry = EvmRuntimeRetry::Fresh;
                }
                return Ok(super::super::empty_execution(
                    instantiate_with(&bound, self.domain).map_err(MachineError::frontier_reason)?,
                ));
            }
        };
        let mut retry = execute_inner_with(
            self.world,
            self.entry,
            payload,
            self.config,
            self.domain,
            &self.policies.context,
            self.budget,
            self.storage_reads,
            true,
        );
        // Cancellation or an exhausted sticky ledger may arrive at the end of
        // the transfer. Recheck before publishing its bound; the transfer cap
        // was checked before execution and permits this last allowed transfer.
        let interruption = MachineError::check_cancelled()
            .err()
            .or_else(|| self.budget.exhausted().then(MachineError::work_failure));
        if let Some(reason) = interruption.map(MachineError::frontier_reason)
            && !retry
                .frontiers
                .iter()
                .any(|frontier| frontier.reason == reason)
        {
            super::super::boundary(&mut retry, 0, reason);
        }
        if retry.frontiers.is_empty() {
            // The coarse transfer covers the full original entry. Commit its
            // input and receipts together; a failed retry leaves only Attempted.
            let work = bound.work_size().saturating_add(coarse.work_size());
            let node = &mut self.table.nodes[id];
            self.runtime
                .with_run(
                    self.active
                        .as_mut()
                        .or(self.caller.as_deref_mut())
                        .ok_or(MachineError::Shape.frontier_reason())?,
                    self.budget,
                    |run| {
                        run.publish_prepared_with(&node.input, Change::Changed, work, || {
                            node.payload = bound;
                            node.runtime_retry = EvmRuntimeRetry::Coarse(coarse);
                        })
                        .map_err(kernel_failure)
                    },
                )
                .map_err(MachineError::frontier_reason)?;
            retry.diagnostics.push((
                0,
                DiagnosticKind::RelationalPrecisionLost(
                    crate::domain::relational::QueryReason::ExpressionLimit,
                ),
            ));
        }
        Ok(retry)
    }

    pub(super) fn accept(&mut self, from: usize, execution: EvmExecution<D::Element>) {
        let procedure = self.table.nodes[from].procedure;
        self.table
            .precision_losses
            .extend(execution.diagnostics.into_iter().filter_map(|(_, kind)| {
                matches!(kind, DiagnosticKind::RelationalPrecisionLost(_)).then_some(kind)
            }));
        self.table.frontiers.extend(execution.frontiers);
        for outcome in execution.outcomes {
            if let Err(error) = self.publish(procedure, outcome.kind, outcome.data, outcome.store) {
                self.frontier_node(from, error.frontier_reason(), 0);
            }
        }
        for (ordinal, mut successor) in execution.successors.into_iter().enumerate() {
            if X::ENABLED {
                if let Err(error) = self.reserve(successor.payload.work_size()) {
                    self.frontier(successor.payload, error.frontier_reason(), 0);
                    continue;
                }
                let original = successor.payload.clone();
                match self.incoming(successor.payload, successor.kind, ordinal) {
                    Ok(Some(payload)) => successor.payload = payload,
                    Ok(None) => continue,
                    Err(error) => {
                        self.frontier(original, error.frontier_reason(), 0);
                        continue;
                    }
                }
            }
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
                        .and_then(EvmValue::singleton)
                        .filter(|status| *status <= alloy_primitives::U256::ONE)
                        .map(|status| status == alloy_primitives::U256::ONE)
                } else {
                    None
                };
                // This finite partition lasts exactly the first resumed block,
                // where bytecode normally tests CALL status. Its successors use
                // their actual new edge kinds and return to ordinary node keys.
                let kind = successor.kind;
                self.enqueue(procedure, successor.payload, resume_status, from)
                    .and_then(|to| self.observe_body_edge(from, to, kind))
            };
            if let Err(error) = operation {
                self.frontier(original, error.frontier_reason(), 0);
            }
        }
    }

    pub(super) fn subscribe(
        &mut self,
        from: usize,
        payload: EvmMachinePayload<D::Element>,
    ) -> Result<(), MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        let callee_payload = EvmMachinePayload {
            call_stack: EvmCallStack::new(EvmRootFrame {
                state: payload.active().clone(),
            }),
            store: payload.store.clone(),
            relations: payload.relations.clone(),
        };
        let callee = self.procedure_from(callee_payload, Some(from))?;
        let payload = self.form(&payload)?;
        let key = (from, callee);
        if let Some(subscription) = self.table.subscribers_by_key.get(&key).copied() {
            let widen = self.node_feedback(from)
                && self.table.subscribers[subscription].updates
                    >= self.domain.spec().widening_after_updates();
            let old = &mut self.table.subscribers[subscription];
            let policy = SummaryAdmission {
                domain: self.domain,
                widen,
            };
            self.runtime.with_run(
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
                |run| {
                    run.admit(
                        &old.input,
                        &mut old.payload,
                        &mut old.updates,
                        &payload,
                        &policy,
                    )
                    .map_err(admission_failure)
                },
            )?;
        } else {
            self.reserve(payload.work_size().saturating_add(3))?;
            let subscription = self.table.subscribers.len();
            let scope = self.runtime.relative;
            let input = self
                .runtime
                .kernel()
                .slot(scope, &mut MachineMeter(self.budget))
                .map_err(kernel_failure)?;
            self.runtime
                .register_relative(TaskKind::Resume(subscription), self.budget)?;
            self.table.subscribers.push(EvmSubscriber {
                from,
                callee,
                payload,
                input,
                updates: 0,
                delivered: BTreeMap::new(),
            });
            self.table.subscribers_by_key.insert(key, subscription);
            self.table.procedures[callee].subscribers.push(subscription);
        }
        let subscription = *self
            .table
            .subscribers_by_key
            .get(&key)
            .ok_or(MachineError::Shape)?;
        self.observe_continuation(subscription)?;
        self.observe_origin(
            crate::analysis::cooperation::Point {
                namespace: 1,
                kind: crate::analysis::cooperation::PointKind::Body,
                index: from,
            },
            callee,
        )?;
        self.observe_body_edge(
            from,
            self.table.procedures[callee].entry,
            MachineEdgeKind::Call,
        )?;
        // Resume reads both slots before using either. Kernel publications wake
        // it for caller-template growth as well as output growth; no manual
        // eager delivery recursion or generation-only receipt can lose a caller.
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        procedure: usize,
        kind: OutcomeKind,
        data: EvmByteArray<D::Element>,
        store: EvmStore<D::Element>,
    ) -> Result<(), MachineError> {
        self.reserve(
            store
                .work_size()
                .saturating_add(data.work_size())
                .saturating_mul(4),
        )?;
        let store = store.summary_projection_with(self.domain)?;
        let data = data.summary_projection_with(self.domain)?;
        let old_work = self.table.procedures[procedure]
            .outputs
            .get(&kind)
            .map_or(0, |old| old.work);
        self.reserve(old_work.saturating_mul(4))?;
        let output = if let Some(old) = self.table.procedures[procedure].outputs.get(&kind) {
            let feedback = self.table.procedures[procedure].recursive
                || self.table.procedures[procedure].local_cycle;
            let widen = feedback && old.generation > self.domain.spec().widening_after_updates();
            let joined_store = old.store.join_with(&store, self.domain, widen);
            let joined_data = old.data.join_with(&data, self.domain, widen);
            if old.store == joined_store && old.data == joined_data {
                return Ok(());
            }
            let work = joined_store
                .work_size()
                .saturating_add(joined_data.work_size());
            EvmOutput {
                store: joined_store,
                data: joined_data,
                work,
                generation: old
                    .generation
                    .checked_add(1)
                    .ok_or(MachineError::IdentityExhausted)?,
            }
        } else {
            let work = store.work_size().saturating_add(data.work_size());
            EvmOutput {
                store,
                data,
                work,
                generation: 1,
            }
        };
        let output_work = self.table.procedures[procedure]
            .outputs
            .values()
            .fold(output.work.saturating_add(1), |work, old| {
                work.saturating_add(old.work)
            });
        self.reserve(output_work)?;
        let mut outputs = self.table.procedures[procedure].outputs.clone();
        outputs.insert(kind, output);
        let old = &mut self.table.procedures[procedure];
        let updates = old
            .output_updates
            .checked_add(1)
            .ok_or(MachineError::IdentityExhausted)?;
        self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_parts(
                    &old.output,
                    &mut old.outputs,
                    outputs,
                    &mut old.output_updates,
                    updates,
                    Change::Changed,
                    output_work,
                )
                .map_err(kernel_failure)
            },
        )?;
        self.observe_summary(procedure, false)?;
        Ok(())
    }

    fn resume(&mut self, subscriber: usize) -> Result<bool, MachineError> {
        let subscription = &self.table.subscribers[subscriber];
        let procedure = &self.table.procedures[subscription.callee];
        let work = procedure
            .outputs
            .values()
            .fold(1usize, |work, output| work.saturating_add(output.work));
        let (template, outputs) = self.runtime.with_run(
            self.active.as_mut().expect("admitted Resume"),
            self.budget,
            |run| {
                let template = run
                    .read_slot(
                        &subscription.input,
                        &subscription.payload,
                        subscription.payload.work_size(),
                    )
                    .map_err(kernel_failure)?;
                let outputs = run
                    .read_slot(&procedure.output, &procedure.outputs, work)
                    .map_err(kernel_failure)?;
                Ok((template, outputs))
            },
        )?;
        let Availability::Value(template_payload) = template.state else {
            return Err(MachineError::Shape);
        };
        let outputs_version = outputs.version;
        let Availability::Value(outputs) = outputs.state else {
            return Err(MachineError::Shape);
        };
        for (kind, output) in outputs {
            self.producer = Some(super::Producer {
                kind: 1,
                id: subscriber,
                case: match kind {
                    OutcomeKind::Return => 0,
                    OutcomeKind::Revert => 1,
                    OutcomeKind::Failure => 2,
                },
                input: template.version,
                other: Some(outputs_version),
                parent: None,
            });
            if !self.deliver(
                subscriber,
                kind,
                template.version,
                &template_payload,
                &output,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn deliver(
        &mut self,
        subscriber: usize,
        kind: OutcomeKind,
        template_version: Version,
        template: &EvmPayloadSummary<D::Element>,
        output: &EvmOutput<D::Element>,
    ) -> Result<bool, MachineError> {
        let subscription = &self.table.subscribers[subscriber];
        let stamp = (template_version, output.generation);
        // Native stamps alone cannot certify cooperative contributions: a
        // foreign premise can retire a namespace2 scope without changing them.
        if !X::ENABLED && subscription.delivered.get(&kind).copied() == Some(stamp) {
            return Ok(true);
        }
        let from = subscription.from;
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
                output.store.clone()
            } else {
                child.frame.saved_store
            };
            // Changing the active frame and EvmStore<D::Element> invalidates current-read templates.
            resumed.storage_observations = Default::default();
            resumed.work = resumed.measure_work();
            let to =
                self.enqueue_summary(self.table.nodes[from].procedure, resumed, None, Some(from))?;
            self.observe_body_edge(from, to, MachineEdgeKind::Summary(kind))?;
            self.table.subscribers[subscriber]
                .delivered
                .insert(kind, stamp);
            self.table.propagations += 1;
            return Ok(true);
        }
        self.reserve(
            template
                .work_size()
                .saturating_add(output.work)
                .saturating_mul(4),
        )?;
        let mut payload = instantiate_with(template, self.domain)?;
        // Installed constraints still refer only to the old immutable snapshot.
        // Never reinstall those read bounds after replacing the EvmStore<D::Element>.
        payload.store = output.store.instantiate_with(self.domain)?;
        let data = output.data.instantiate_with(self.domain)?;
        let execution = resume_summary_inner(
            payload,
            kind,
            data,
            self.config,
            self.domain,
            self.budget,
            true,
        );
        let before = self.table.frontiers.len();
        self.accept(from, execution);
        if self.table.frontiers.len() != before {
            return Ok(false);
        }
        self.table.subscribers[subscriber]
            .delivered
            .insert(kind, stamp);
        self.table.propagations += 1;
        Ok(true)
    }
}

fn admission_failure(error: PublishError<MachineError, MachineError>) -> MachineError {
    match error {
        PublishError::Policy(error) => error,
        PublishError::Kernel(error) => kernel_failure(error),
    }
}

/// A stopped solve exports the already retained location. Construct a native N
/// witness only after paying its complete materialization; never deep-clone a
/// summary merely to discover that the exhausted root ledger cannot afford it.
fn parked_bound<D: EvmNumericAlgebra>(
    summary: &EvmPayloadSummary<D::Element>,
    reason: FrontierReason,
    pc: usize,
    domain: crate::domain::AlgebraRef<'_, D>,
    budget: &mut crate::resource::WorkBudget,
) -> EvmExecutionFrontier<D::Element> {
    let current =
        crate::analysis::state::bounded::reserve(budget, summary.work_size().saturating_mul(4))
            .and_then(|()| instantiate_with(summary, domain))
            .ok();
    let target = current
        .as_ref()
        .map_or_else(|| summary.location.clone(), EvmMachinePayload::key);
    EvmExecutionFrontier {
        pc,
        reason,
        target: Some(target),
        range: None,
        payload: current.map(Arc::new),
    }
}
