//! Joint known/unknown procedure worklist and per-caller return propagation.

use super::Composition;
use std::borrow::Cow;

mod caller_template;
use super::admission;
use crate::analysis::engine::policy::{self, KeyIndex};
use crate::analysis::engine::{
    Acceleration, AccelerationInput, Arrival, ContextPolicy, PartitionPolicy, PartitionedRelative,
    PolicyContext, PolicySite, ProcedureOutput, StatePolicy,
};
use crate::analysis::state::payload;
use crate::analysis::worklist::{MachineMeter, kernel_failure, runtime::TaskKind};
use crate::domain::EvmNumericAlgebra;
use analysis_kernel::{Availability, Change, RunningJob, Version, algebra::Prepared};

use super::coarse_retry::{self, EvmRuntimeRetry};
use super::{
    Arc, BTreeMap, BTreeSet, DiagnosticKind, EvmByteArray, EvmCallStack, EvmExecution,
    EvmExecutionFrontier, EvmMachinePayload, EvmNode, EvmOutput, EvmPayloadSummary, EvmProcedure,
    EvmRootFrame, EvmSolver, EvmStore, EvmSubscriber, FrameCode, FrontierReason, Limit,
    MachineEdgeKind, MachineError, OutcomeKind, execute_inner_with, resume_summary_inner,
    transfer_stop,
};

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    fn form(
        &mut self,
        mut payload: EvmMachinePayload<D::Element>,
    ) -> Result<EvmPayloadSummary<D::Element>, MachineError> {
        let runtime_process =
            self.policies.state.runtime_execution() == crate::analysis::RuntimeExecution::Process;
        if runtime_process && X::ENABLED {
            return Err(MachineError::Process(
                crate::analysis::ProcessBoundary::CooperativeInputs,
            ));
        }
        if runtime_process {
            self.freeze_process_catalogue(&payload)?;
        }
        let mut refined = if runtime_process || X::ENABLED {
            super::payload::caller::copy_with_scalar_guarantees(
                &payload,
                self.entry,
                self.domain,
                self.budget,
            )?
        } else {
            // Legacy callers already retain a separately paid original failure
            // frontier. Project this owned input without another whole copy.
            // Projection still walks every retained structural field, even
            // when unrelated relations cause no native query. Fund that walk
            // separately from its actual native queries and Cow copies.
            if !payload.relations.is_empty() {
                self.reserve(payload.policy_work_size())?;
            }
            let relations = std::mem::take(&mut payload.relations);
            let limits = self.domain.spec().relations();
            let mut projection = crate::analysis::state::projection::Projection::new(
                &relations,
                self.domain,
                &limits,
                self.budget,
                crate::analysis::state::projection::Policy::Summary,
            );
            let projected = projection.payload(&mut payload);
            let finished = projection.finish();
            payload.relations = relations;
            projected?;
            finished?;
            payload
        };
        let history_work = refined.call_stack.iter().fold(0usize, |work, frame| {
            work.saturating_add(if frame.code == FrameCode::Unknown {
                0
            } else {
                self.policies
                    .context
                    .normalization_work(frame.key.jump_history.len())
            })
        });
        // The alias and observation builders below pay their own complete
        // discovery. Only context normalization runs outside those builders.
        self.reserve(history_work)?;
        for frame in refined.call_stack.iter_mut() {
            if frame.code != FrameCode::Unknown {
                self.policies.context.normalize(&mut frame.key.jump_history);
            }
        }
        let summary = super::payload::caller::form_with_runtime(
            &refined,
            self.entry,
            self.domain,
            self.budget,
            runtime_process,
        )?;
        if runtime_process {
            self.prepare_process_summary(summary)
        } else {
            Ok(summary)
        }
    }

    pub(super) fn reserve(&mut self, cost: usize) -> Result<(), MachineError> {
        // Use the same retained payer and both cancellation identities before,
        // on refusal and after charging; a live ambient token cannot mask the
        // cancellation captured by this transaction's ledger.
        PolicyContext::new(self.domain, self.entry, self.budget).reserve(cost)
    }

    fn partition_relative(
        &mut self,
        payload: EvmPayloadSummary<D::Element>,
        site: PolicySite,
        resume_status: Option<bool>,
    ) -> Result<PartitionedRelative<S::Key, D::Element>, MachineError> {
        // The execute/accept/resume boundary retains the original complete
        // obligation on refusal. This adapter owns the formed numerical copy.
        let class = policy::classify(
            &self.policies.partition,
            Arrival { resume_status },
            self.budget,
        )?;
        self.policies
            .state
            .prepare_relative_owned(
                payload,
                site,
                class,
                &mut PolicyContext::new(self.domain, self.entry, self.budget),
            )
            .map_err(|refusal| refusal.error)
    }

    pub(super) fn acceleration(
        &self,
        site: PolicySite,
        feedback: bool,
        updates: usize,
    ) -> Acceleration {
        self.policies.state.decide(AccelerationInput {
            site,
            feedback,
            updates,
            default_delay: self.domain.spec().widening_after_updates(),
        })
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

    #[cfg(test)]
    pub(super) fn procedure(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
    ) -> Result<usize, MachineError> {
        crate::resource::clone_payment::checkpoint(self.budget)?;
        let creation = payload
            .call_stack
            .active_child()
            .is_some_and(|child| child.continuation.creation.is_some());
        self.procedure_entry(payload, creation)
    }

    pub(super) fn procedure_entry(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        creation: bool,
    ) -> Result<usize, MachineError> {
        self.procedure_from(payload, None, creation)
    }

    fn procedure_from(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        from: Option<usize>,
        creation: bool,
    ) -> Result<usize, MachineError> {
        let payload = self.form(payload)?;
        let PartitionedRelative { key, payload } =
            self.partition_relative(payload, PolicySite::Procedure, None)?;
        let lookup_key = (self.namespace, policy::clone_key(&key, self.budget)?);
        if let Some(id) = self.table.procedures_by_key.get(&lookup_key, self.budget)? {
            self.table.reused += 1;
            if let Some(from) = from {
                self.call_dependency(from, id)?;
            }
            let old = &self.table.procedures[id];
            let acceleration =
                self.acceleration(PolicySite::Procedure, old.recursive, old.entry_updates);
            let prepared = admission::prepare(
                &self.policies.state,
                &old.entry_payload,
                &payload,
                PolicySite::Procedure,
                acceleration,
                old.entry_updates,
                &mut PolicyContext::new(self.domain, self.entry, self.budget),
            )?;
            self.reserve(prepared.value.work_size())?;
            let admitted = prepared.value.clone_with_payment(self.budget)?;
            self.enqueue_admitted(id, admitted, None, None, Some(prepared))?;
            self.observe_procedure(id)?;
            return Ok(id);
        }
        let acceleration = self.acceleration(PolicySite::Procedure, false, 0);
        let prepared = admission::prepare(
            &self.policies.state,
            &payload,
            &payload,
            PolicySite::Procedure,
            acceleration,
            0,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        self.reserve(prepared.value.work_size().saturating_mul(2))?;
        let payload = prepared.value.clone_with_payment(self.budget)?;
        let id = self.table.procedures.len();
        if self.counters.states >= self.config.analysis.max_states {
            return Err(MachineError::States);
        }
        let frame = payload.active();
        let store_effect = super::effects::classify(
            frame.code,
            frame.key.mode,
            frame.program.as_ref(),
            creation,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        let process = match payload.active().semantics {
            crate::analysis::TransferSemantics::KnownProcess => {
                Some(super::process::Capabilities::scan(
                    payload
                        .active()
                        .program
                        .as_ref()
                        .ok_or(MachineError::Shape)?,
                    self.budget,
                )?)
            }
            crate::analysis::TransferSemantics::Environment
                if self.policies.state.runtime_execution()
                    == crate::analysis::RuntimeExecution::Process =>
            {
                Some(super::process::Capabilities::unknown())
            }
            crate::analysis::TransferSemantics::ConstructorProcess => {
                Some(match payload.active().program.as_ref() {
                    Some(program) => super::process::Capabilities::scan(program, self.budget)?,
                    None => super::process::Capabilities::unknown(),
                })
            }
            crate::analysis::TransferSemantics::BoundaryProcess => Some(
                super::process::Capabilities::boundary(payload.active().code),
            ),
            _ => None,
        };
        let stored_payload = payload.clone_with_payment(self.budget)?;
        self.table.regional_snapshot = None;
        PolicyContext::new(self.domain, self.entry, self.budget).reserve(0)?;
        let mut index = std::mem::take(&mut self.table.procedures_by_key);
        let created = index.insert_with((self.namespace, key), self.budget, |budget| {
            let scope = self.runtime.relative;
            let output = self
                .runtime
                .kernel()
                .slot(scope, &mut MachineMeter(budget))
                .map_err(kernel_failure)?;
            let entry_source = self
                .runtime
                .kernel()
                .slot(scope, &mut MachineMeter(budget))
                .map_err(kernel_failure)?;
            self.table.procedures.push(EvmProcedure {
                namespace: self.namespace,
                frame: payload.active().key.clone(),
                entry_payload: stored_payload,
                store_effect,
                process,
                entry_updates: prepared.metadata,
                entry_feedback: false,
                entry_revision: 0,
                entry_source,
                entries: BTreeSet::new(),
                nodes: KeyIndex::new(),
                outputs: BTreeMap::new(),
                output,
                dependencies: BTreeSet::new(),
                recursive: false,
                local_cycle: false,
                output_updates: 0,
                covered_blocks: BTreeSet::new(),
            });
            Ok(id)
        });
        self.table.procedures_by_key = index;
        let id = created?;
        self.source_inventory_changed()?;
        self.observe_procedure(id)?;
        self.observe_summary(id, false)?;
        // Its entry source is now a permanent native identity. A later Body
        // refusal retains this unexecuted entry and the caller's frontier;
        // recycling the ID would invalidate tracked entry/navigation readers.
        self.enqueue_admitted(id, payload, None, None, Some(prepared))?;
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
        let payload = self.form(payload)?;
        self.enqueue_summary(procedure, payload, resume_status, Some(from))
    }

    pub(super) fn enqueue_summary(
        &mut self,
        procedure: usize,
        payload: EvmPayloadSummary<D::Element>,
        resume_status: Option<bool>,
        from: Option<usize>,
    ) -> Result<usize, MachineError> {
        self.enqueue_admitted(procedure, payload, resume_status, from, None)
    }

    /// Entry admission and its selected body's bound are installed under one
    /// body-slot publication. A site-specific partition need not select the
    /// procedure's first body again on a later entry.
    fn enqueue_admitted(
        &mut self,
        procedure: usize,
        payload: EvmPayloadSummary<D::Element>,
        resume_status: Option<bool>,
        from: Option<usize>,
        entry: Option<Prepared<EvmPayloadSummary<D::Element>, usize>>,
    ) -> Result<usize, MachineError> {
        let PartitionedRelative { key, payload } =
            self.partition_relative(payload, PolicySite::Body, resume_status)?;
        let existing = self.table.procedures[procedure]
            .nodes
            .get(&key, self.budget)?;
        let previous_updates = existing.map(|id| self.table.nodes[id].updates);
        let id = if let Some(id) = existing {
            if let Some(from) = from {
                self.node_edge(from, id)?;
            }
            let feedback = self.node_feedback(id)
                || (entry.is_some() && self.table.procedures[procedure].recursive);
            self.admit_node(id, &payload, feedback, false, entry)?;
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
            let feedback = entry.is_some() && self.table.procedures[procedure].recursive;
            let acceleration = self.acceleration(PolicySite::Body, feedback, 0);
            let prepared = admission::prepare(
                &self.policies.state,
                &payload,
                &payload,
                PolicySite::Body,
                acceleration,
                0,
                &mut PolicyContext::new(self.domain, self.entry, self.budget),
            )?;
            let payload = prepared.value;
            // A fresh target has no outgoing edge and cannot close a cycle.
            // Reserve its graph edge together with the node before installation.
            self.reserve(
                payload
                    .work_size()
                    .saturating_add(usize::from(from.is_some()))
                    .saturating_add(entry.as_ref().map_or(0, |_| 1)),
            )?;
            self.table.regional_snapshot = None;
            let is_entry = entry.is_some();
            if let Some(entry) = entry {
                self.publish_entry(procedure, entry)?;
            }
            let mut nodes = std::mem::take(&mut self.table.procedures[procedure].nodes);
            let admitted = nodes.insert_with(key, self.budget, |budget| {
                let id = self.table.nodes.len();
                let scope = self.runtime.relative;
                let input = self
                    .runtime
                    .kernel()
                    .slot(scope, &mut MachineMeter(budget))
                    .map_err(kernel_failure)?;
                self.runtime.register_relative(TaskKind::Body(id), budget)?;
                self.table.nodes.push(EvmNode {
                    procedure,
                    input,
                    payload,
                    updates: prepared.metadata,
                    successors: BTreeSet::new(),
                    loop_head: false,
                    feedback,
                    runtime_retry: EvmRuntimeRetry::Fresh,
                    excluded: false,
                    transfer: None,
                });
                if is_entry {
                    self.table.procedures[procedure].entries.insert(id);
                }
                self.counters.states += 1;
                Ok(id)
            });
            self.table.procedures[procedure].nodes = nodes;
            let id = admitted?;
            self.source_inventory_changed()?;
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

    /// The runtime layout is a covering closure, not an admission policy. Its
    /// candidate still passes through the injected policy. Requiring closure
    /// stability prevents a result from silently leaving coarse mode or
    /// regrowing coordinates; incompatibility remains a typed refusal.
    pub(super) fn admit_node(
        &mut self,
        id: usize,
        incoming: &EvmPayloadSummary<D::Element>,
        feedback: bool,
        loop_head: bool,
        entry: Option<Prepared<EvmPayloadSummary<D::Element>, usize>>,
    ) -> Result<(), MachineError> {
        let node = &self.table.nodes[id];
        let site = if matches!(&node.runtime_retry, EvmRuntimeRetry::Coarse(_)) {
            PolicySite::RuntimeRetry
        } else {
            PolicySite::Body
        };
        let acceleration = self.acceleration(site, feedback, node.updates);
        let candidate;
        let incoming = if let EvmRuntimeRetry::Coarse(coarse) = &node.runtime_retry {
            candidate = coarse.join_with_payment(
                &node.payload,
                incoming,
                self.domain,
                self.budget,
                false,
            )?;
            &candidate
        } else {
            incoming
        };
        let mut prepared = admission::prepare(
            &self.policies.state,
            &node.payload,
            incoming,
            site,
            acceleration,
            node.updates,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        if let EvmRuntimeRetry::Coarse(coarse) = &node.runtime_retry {
            coarse_retry::check_closed(coarse, &prepared.value, self.domain, self.budget)?;
        }
        let procedure = node.procedure;
        let entry_change = entry.as_ref().is_some_and(|entry| {
            entry.change == Change::Changed
                || !self.table.procedures[procedure].entries.contains(&id)
                || self.table.procedures[procedure].entry_feedback
                    != self.table.procedures[procedure].recursive
        });
        if entry_change || feedback != node.feedback || (loop_head && !node.loop_head) {
            prepared.change = Change::Changed;
        }
        let is_entry = entry.is_some();
        let work = prepared.install_work.saturating_add(usize::from(is_entry));
        if let Some(entry) = entry {
            self.publish_entry(procedure, entry)?;
        }
        if prepared.change == Change::Changed {
            self.table.regional_snapshot = None;
        }
        let node = &mut self.table.nodes[id];
        let procedure = &mut self.table.procedures[procedure];
        self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_with(&node.input, prepared.change, work, || {
                    node.payload = prepared.value;
                    node.updates = prepared.metadata;
                    node.feedback = feedback;
                    node.loop_head |= loop_head;
                    if is_entry {
                        procedure.entries.insert(id);
                    }
                })
                .map_err(kernel_failure)
            },
        )?;
        Ok(())
    }

    /// Entry is an independently observable covering fact. Its stable anchor
    /// cannot follow whichever Body key a later preparation happens to choose.
    /// A later Body refusal still retains the original caller/input frontier.
    fn publish_entry(
        &mut self,
        procedure: usize,
        mut prepared: Prepared<EvmPayloadSummary<D::Element>, usize>,
    ) -> Result<(), MachineError> {
        let entry = &mut self.table.procedures[procedure];
        if entry.entry_feedback != entry.recursive {
            prepared.change = Change::Changed;
        }
        let revision = entry
            .entry_revision
            .checked_add(usize::from(prepared.change == Change::Changed))
            .ok_or(MachineError::IdentityExhausted)?;
        let changed = prepared.change == Change::Changed;
        self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_with(
                    &entry.entry_source,
                    prepared.change,
                    prepared.install_work.saturating_add(1),
                    || {
                        entry.entry_payload = prepared.value;
                        entry.entry_updates = prepared.metadata;
                        entry.entry_feedback = entry.recursive;
                        entry.entry_revision = revision;
                    },
                )
                .map_err(kernel_failure)
            },
        )?;
        if changed {
            self.table.regional_snapshot = None;
            // Losing an old entry universe invalidates its completion proof
            // even when this publication leaves the native output bound equal.
            self.observe_summary(procedure, false)?;
        }
        Ok(())
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
        // Retrying an obligation invalidates its old receipt even if preparing
        // the new input subsequently refuses without a native slot change.
        self.regional_begin(&task.0);
        // A Body producer is installed only after this evaluation has executed
        // and retained its new transfer. Older same-version receipts are not a
        // reuse certificate for a preparation that never reached execution.
        self.producer = None;
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
                    self.park_admitted(&task.0, error.frontier_reason());
                    false
                }
            },
            TaskKind::Extension(_) | TaskKind::Seed(_) => {
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
            TaskKind::World { .. } | TaskKind::SummaryProof => {
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
            self.park_admitted(&task.0, error.frontier_reason());
            return false;
        }
        if let Some(token) = self.active.take()
            && let Err(error) = self.runtime.finish(token, self.budget)
        {
            self.park_pending(error.frontier_reason());
            return false;
        }
        let transferred = match &task.0 {
            TaskKind::Body(id) => self
                .producer
                .is_some_and(|producer| producer.kind == 0 && producer.id == *id),
            // A matching native delivery may be skipped without rerunning the
            // callee. Resume still revalidates its committed delivery stamps.
            _ => true,
        };
        self.regional_current(
            &task.0,
            transferred
                && !self.deferred
                && same_source
                && (!X::ENABLED || self.table.frontiers.len() == frontiers_before),
        );
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
            self.park_admitted(&TaskKind::Body(id), reason);
            return false;
        }
        // The following full copy and replay separately own their complete
        // invoices. Launch bookkeeping is fixed, not another native envelope.
        let work = 1;
        if let Err(error) = self.reserve(work) {
            self.park_admitted(&TaskKind::Body(id), error.frontier_reason());
            return false;
        }
        let input = match self.table.nodes[id].payload.clone_with_payment(self.budget) {
            Ok(input) => input,
            Err(error) => {
                self.park_admitted(&TaskKind::Body(id), error.frontier_reason());
                return false;
            }
        };
        #[cfg(test)]
        let instantiated = super::evidence::refuse_body_instantiation().map_or_else(
            || payload::caller::instantiate_with_payment(&input, self.domain, self.budget),
            Err,
        );
        #[cfg(not(test))]
        let instantiated =
            payload::caller::instantiate_with_payment(&input, self.domain, self.budget);
        let execution = match instantiated {
            Ok(payload) => {
                let payload = match self.cooperative_input(id, payload, 0) {
                    Ok(Some(payload)) => payload,
                    Ok(None) => {
                        if let Err(error) = self.track_input(id) {
                            self.park_admitted(&TaskKind::Body(id), error.frontier_reason());
                            return false;
                        }
                        return true;
                    }
                    Err(error) => {
                        let reason = error.frontier_reason();
                        self.frontier_ref(&input, reason.clone(), 0);
                        self.park_pending(reason);
                        return false;
                    }
                };
                #[cfg(test)]
                super::tests::native_engine::body_input(&payload);
                self.counters.transfers += 1;
                self.counters.report(self.budget.used());
                let execution = if self.policies.state.runtime_execution()
                    == crate::analysis::RuntimeExecution::Process
                    && input.active().semantics != crate::analysis::TransferSemantics::Native
                {
                    self.process(id, payload)
                } else if payload.active().code == FrameCode::Unknown {
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
                self.park_admitted(&TaskKind::Body(id), error.frontier_reason());
                return false;
            }
        };
        if let Err(error) = self.regional_transfer(id, version, &execution) {
            self.park_admitted(&TaskKind::Body(id), error.frontier_reason());
            return false;
        }
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
        let frontiers_before = self.table.frontiers.len();
        self.accept(id, execution);
        self.regional_transfer_complete(
            id,
            !self.deferred && self.table.frontiers.len() == frontiers_before,
        );
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
        match retry {
            Some(Ok(execution)) => {
                for frontier in execution.frontiers {
                    if coarse_retry::interruption(&frontier.reason) {
                        self.frontier_ref(original, frontier.reason, 0);
                    }
                }
            }
            Some(Err(reason)) => {
                // A summary-policy refusal is an unfinished obligation in its
                // own right, even when the original failure was retention.
                self.frontier_ref(original, reason, 0);
            }
            None => {}
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
                if !first
                    .frontiers
                    .iter()
                    .any(|frontier| frontier.reason == reason)
                {
                    first.frontiers.push(parked_bound(
                        original,
                        reason,
                        0,
                        self.domain,
                        self.budget,
                    ));
                }
                first
            }
        }
    }

    pub(super) fn prepare_runtime_retry(
        &mut self,
        id: usize,
        original: &EvmPayloadSummary<D::Element>,
    ) -> Result<coarse_retry::Admitted<D::Element>, FrontierReason> {
        let coarse_retry::Prepared {
            representation: coarse,
            bound,
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
        let node = &self.table.nodes[id];
        let feedback = self.node_feedback(id);
        let acceleration = self.acceleration(PolicySite::RuntimeRetry, feedback, node.updates);
        let prepared = admission::prepare(
            &self.policies.state,
            original,
            &bound,
            PolicySite::RuntimeRetry,
            acceleration,
            node.updates,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )
        .map_err(MachineError::frontier_reason)?;
        coarse_retry::check_closed(&coarse, &prepared.value, self.domain, self.budget)
            .map_err(MachineError::frontier_reason)?;
        self.reserve(prepared.value.work_size().saturating_mul(4))
            .map_err(MachineError::frontier_reason)?;
        let payload =
            payload::caller::instantiate_with_payment(&prepared.value, self.domain, self.budget)
                .map_err(MachineError::frontier_reason)?;
        self.retry_admission()?;
        Ok(coarse_retry::Admitted {
            representation: coarse,
            preparation: prepared,
            payload,
        })
    }

    fn execute_coarse(
        &mut self,
        id: usize,
        original: &EvmPayloadSummary<D::Element>,
    ) -> Result<EvmExecution<D::Element>, FrontierReason> {
        // Only the attempt bit may survive failed preparation. No execution has
        // reached accept/publish, and the original immutable checkpoint is used.
        self.table.nodes[id].runtime_retry = EvmRuntimeRetry::Attempted;
        let coarse_retry::Admitted {
            representation: coarse,
            preparation: prepared,
            payload,
        } = self.prepare_runtime_retry(id, original)?;
        let bound = prepared.value;
        let feedback = self.node_feedback(id);
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
                self.reserve(bound.work_size().saturating_mul(4))
                    .map_err(MachineError::frontier_reason)?;
                return Ok(super::super::empty_execution(
                    payload::caller::instantiate_with_payment(&bound, self.domain, self.budget)
                        .map_err(MachineError::frontier_reason)?,
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
            let work = prepared.install_work.saturating_add(coarse.work_size());
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
                            node.updates = prepared.metadata;
                            node.feedback = feedback;
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
        // Retain only typed continuation/instruction evidence for precision
        // classification; no scalar proof query belongs to admission routing.
        let source = &self.table.nodes[from].payload;
        let source_depth = source.frames.len();
        let local_call = executed_call(source, &execution);
        let completed_calls = execution.completed_calls;
        let mut process_calls = execution.process_calls.into_iter().peekable();
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
                let original = match successor.payload.copy_for_policy(&mut PolicyContext::new(
                    self.domain,
                    self.entry,
                    self.budget,
                )) {
                    Ok(original) => original,
                    Err(error) => {
                        self.frontier(successor.payload, error.frontier_reason(), 0);
                        continue;
                    }
                };
                match self.incoming(successor.payload, successor.kind, ordinal) {
                    Ok(Some(payload)) => successor.payload = payload,
                    Ok(None) => continue,
                    Err(error) => {
                        self.frontier(original, error.frontier_reason(), 0);
                        continue;
                    }
                }
            }
            let original = match successor.payload.copy_for_policy(&mut PolicyContext::new(
                self.domain,
                self.entry,
                self.budget,
            )) {
                Ok(original) => original,
                Err(error) => {
                    self.frontier(successor.payload, error.frontier_reason(), 0);
                    continue;
                }
            };
            let operation = if successor.kind == MachineEdgeKind::Call {
                let action = if process_calls
                    .peek()
                    .is_some_and(|(index, _, _)| *index == ordinal)
                {
                    process_calls
                        .next()
                        .map(|(_, action, target)| (action, target))
                } else {
                    None
                };
                self.subscribe_with_action(from, successor.payload, action)
            } else {
                let resume_status =
                    call_resume_status(local_call, source_depth, &completed_calls, &successor);
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

    #[cfg(test)]
    pub(super) fn subscribe(
        &mut self,
        from: usize,
        payload: EvmMachinePayload<D::Element>,
    ) -> Result<(), MachineError> {
        self.subscribe_with_action(from, payload, None)
    }

    fn subscribe_with_action(
        &mut self,
        from: usize,
        payload: EvmMachinePayload<D::Element>,
        action: Option<super::ProcessCall>,
    ) -> Result<(), MachineError> {
        self.reserve(payload.work_size().saturating_mul(4))?;
        // This complete invoice owns exactly the detached frame/store/relation
        // copies below (a subset of the full payload), never later queries.
        payload.pay_clone_with(self.budget)?;
        let callee_payload = EvmMachinePayload {
            call_stack: EvmCallStack::new(EvmRootFrame {
                state: payload.active().clone(),
            }),
            store: payload.store.clone(),
            relations: payload.relations.clone(),
        };
        crate::resource::clone_payment::checkpoint(self.budget)?;
        let creation = payload
            .call_stack
            .active_child()
            .is_some_and(|child| child.continuation.creation.is_some());
        let callee = self.procedure_from(callee_payload, Some(from), creation)?;
        let payload = self.form(payload)?;
        let PartitionedRelative { key, payload } =
            self.partition_relative(payload, PolicySite::Subscriber, None)?;
        let key = (from, callee, (action, key));
        let subscription =
            if let Some(subscription) = self.table.subscribers_by_key.get(&key, self.budget)? {
                self.admit_subscriber(subscription, &payload)?;
                subscription
            } else {
                let feedback = self.node_feedback(from) || self.table.procedures[callee].recursive;
                let acceleration = self.acceleration(PolicySite::Subscriber, feedback, 0);
                let prepared = admission::prepare(
                    &self.policies.state,
                    &payload,
                    &payload,
                    PolicySite::Subscriber,
                    acceleration,
                    0,
                    &mut PolicyContext::new(self.domain, self.entry, self.budget),
                )?;
                // Kernel dependencies replace the former callee-local append.
                self.reserve(prepared.value.work_size().saturating_add(2))?;
                let mut index = std::mem::take(&mut self.table.subscribers_by_key);
                let admitted = index.insert_with(key, self.budget, |budget| {
                    let subscription = self.table.subscribers.len();
                    let scope = self.runtime.relative;
                    let input = self
                        .runtime
                        .kernel()
                        .slot(scope, &mut MachineMeter(budget))
                        .map_err(kernel_failure)?;
                    self.runtime
                        .register_relative(TaskKind::Resume(subscription), budget)?;
                    self.table.subscribers.push(EvmSubscriber {
                        from,
                        callee,
                        action,
                        payload: prepared.value,
                        input,
                        updates: prepared.metadata,
                        feedback,
                        delivered: BTreeMap::new(),
                        returns: BTreeMap::new(),
                    });
                    Ok(subscription)
                });
                self.table.subscribers_by_key = index;
                let id = admitted?;
                self.source_inventory_changed()?;
                id
            };
        self.observe_continuation(subscription)?;
        self.observe_origin(
            crate::analysis::cooperation::Point {
                namespace: 1,
                kind: crate::analysis::cooperation::PointKind::Body,
                index: from,
            },
            callee,
        )?;
        self.observe_call_edge(from, callee)?;
        // Resume reads both slots before using either. Kernel publications wake
        // it for caller-template growth as well as output growth; no manual
        // eager delivery recursion or generation-only receipt can lose a caller.
        Ok(())
    }

    pub(super) fn admit_subscriber(
        &mut self,
        id: usize,
        payload: &EvmPayloadSummary<D::Element>,
    ) -> Result<(), MachineError> {
        let old = &self.table.subscribers[id];
        let feedback = self.node_feedback(old.from) || self.table.procedures[old.callee].recursive;
        let acceleration = self.acceleration(PolicySite::Subscriber, feedback, old.updates);
        let mut prepared = admission::prepare(
            &self.policies.state,
            &old.payload,
            payload,
            PolicySite::Subscriber,
            acceleration,
            old.updates,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        if feedback != old.feedback {
            prepared.change = Change::Changed;
        }
        let old = &mut self.table.subscribers[id];
        self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.publish_prepared_with(
                    &old.input,
                    prepared.change,
                    prepared.install_work,
                    || {
                        old.payload = prepared.value;
                        old.updates = prepared.metadata;
                        old.feedback = feedback;
                    },
                )
                .map_err(kernel_failure)
            },
        )?;
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        procedure: usize,
        kind: OutcomeKind,
        data: EvmByteArray<D::Element>,
        store: EvmStore<D::Element>,
    ) -> Result<(), MachineError> {
        let incoming = ProcedureOutput::project(
            &data,
            &store,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        let procedure_state = &self.table.procedures[procedure];
        let feedback = procedure_state.recursive || procedure_state.local_cycle;
        let old = procedure_state.outputs.get(&kind);
        let acceleration = self.acceleration(
            PolicySite::Output,
            feedback,
            old.map_or(0, |old| old.updates),
        );
        let Some(output) = admission::prepare_output(
            &self.policies.state,
            old,
            &incoming,
            acceleration,
            false,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?
        else {
            return Ok(());
        };
        let output_work = self.table.procedures[procedure]
            .outputs
            .values()
            .fold(output.work.saturating_add(1), |work, old| {
                work.saturating_add(old.work)
            });
        let copy_work =
            super::output_clone_work(&self.table.procedures[procedure].outputs, self.budget)?;
        self.reserve(copy_work)?;
        let mut outputs = self.table.procedures[procedure].outputs.clone();
        outputs.insert(kind, output);
        let old = &self.table.procedures[procedure];
        self.publish_outputs(
            procedure,
            outputs,
            output_work,
            old.recursive,
            old.local_cycle,
        )
    }

    /// Ordinary growth and feedback share one output-slot transaction. In
    /// particular, failed feedback cannot install flags ahead of its outcomes.
    pub(super) fn publish_outputs(
        &mut self,
        procedure: usize,
        outputs: BTreeMap<OutcomeKind, EvmOutput<D::Element>>,
        install_work: usize,
        recursive: bool,
        local_cycle: bool,
    ) -> Result<(), MachineError> {
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
                run.publish_prepared_with(&old.output, Change::Changed, install_work, || {
                    old.outputs = outputs;
                    old.output_updates = updates;
                    old.recursive = recursive;
                    old.local_cycle = local_cycle;
                })
                .map_err(kernel_failure)
            },
        )?;
        self.table.regional_snapshot = None;
        self.observe_summary(procedure, false)?;
        Ok(())
    }

    pub(super) fn refresh_entry(&mut self, procedure: usize) -> Result<(), MachineError> {
        let old = &self.table.procedures[procedure];
        if !old.recursive || old.entry_feedback {
            return Ok(());
        }
        let acceleration = self.acceleration(PolicySite::Procedure, true, old.entry_updates);
        let prepared = admission::prepare(
            &self.policies.state,
            &old.entry_payload,
            &old.entry_payload,
            PolicySite::Procedure,
            acceleration,
            old.entry_updates,
            &mut PolicyContext::new(self.domain, self.entry, self.budget),
        )?;
        self.reserve(prepared.value.work_size())?;
        let admitted = prepared.value.clone_with_payment(self.budget)?;
        self.enqueue_admitted(procedure, admitted, None, None, Some(prepared))?;
        Ok(())
    }

    fn resume(&mut self, subscriber: usize) -> Result<bool, MachineError> {
        let subscription = &self.table.subscribers[subscriber];
        let procedure = &self.table.procedures[subscription.callee];
        let work = super::output_clone_work(&procedure.outputs, self.budget)?;
        let template_work = subscription.payload.clone_work_with(self.budget)?;
        let (template, outputs) = self.runtime.with_run(
            self.active.as_mut().expect("admitted Resume"),
            self.budget,
            |run| {
                let template = run
                    .read_slot(&subscription.input, &subscription.payload, template_work)
                    .map_err(kernel_failure)?;
                let outputs = run
                    .read_slot(&procedure.output, &procedure.outputs, work)
                    .map_err(kernel_failure)?;
                Ok((template, outputs))
            },
        )?;
        self.reserve(0)?;
        let Availability::Value(template_payload) = template.state else {
            return Err(MachineError::Shape);
        };
        let outputs_version = outputs.version;
        let Availability::Value(outputs) = outputs.state else {
            return Err(MachineError::Shape);
        };
        // The snapshot is detached from the subscriber slot and stays stable
        // through all outputs. Only its final consumer can take ownership;
        // previous deliveries retain their separately paid copies.
        let mut template_payload = Some(template_payload);
        let output_count = outputs.len();
        for (ordinal, (kind, output)) in outputs.into_iter().enumerate() {
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
            let payload = if ordinal + 1 == output_count {
                Cow::Owned(template_payload.take().expect("last detached template"))
            } else {
                Cow::Borrowed(
                    template_payload
                        .as_ref()
                        .expect("retained detached template"),
                )
            };
            if !self.deliver(subscriber, kind, template.version, payload, &output)? {
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
        template: Cow<'_, EvmPayloadSummary<D::Element>>,
        output: &EvmOutput<D::Element>,
    ) -> Result<bool, MachineError> {
        self.reserve(0)?;
        let subscription = &self.table.subscribers[subscriber];
        let callee = subscription.callee;
        let creation = template
            .continuations
            .last()
            .is_some_and(|continuation| continuation.creation.is_some());
        let stamp = (template_version, output.generation);
        // Native stamps alone cannot certify cooperative contributions: a
        // foreign premise can retire a namespace2 scope without changing them.
        if !X::ENABLED && subscription.delivered.get(&kind).copied() == Some(stamp) {
            return Ok(true);
        }
        let from = subscription.from;
        self.reserve(super::effects::StoreEffect::MATCH_WORK)?;
        let frame = template.active();
        let keep_store = self.table.procedures[callee].store_effect.keeps_store(
            frame.code,
            frame.key.mode,
            frame.program.as_ref(),
            kind,
            creation,
        );
        self.regional_delivery(subscriber, kind, template_version, output.generation)?;
        if let Some(crate::analysis::CreationContinuation::Process(creation)) = template
            .continuations
            .last()
            .and_then(|continuation| continuation.creation)
        {
            self.deliver_process_creation(subscriber, from, kind, &template, output, creation)?;
            self.table.subscribers[subscriber]
                .delivered
                .insert(kind, stamp);
            self.regional_delivered(subscriber, kind);
            self.table.propagations += 1;
            return Ok(true);
        }
        if template.frames.len() == 2
            && template.frames[0].semantics != crate::analysis::TransferSemantics::Native
            && template.continuations[0].creation.is_none()
            && crate::analysis::state::query::singleton(
                &template.continuations[0].output_size,
                self.budget,
            )? == Some(alloy_primitives::U256::ZERO)
        {
            // The environment observes callback termination, then may perform
            // arbitrary further steps. Its numerical form intentionally erases
            // the result stack, private memory and returndata. Advance that
            // same bound directly; reconstructing erased arrays adds no facts.
            // Copies and Store operations below own their full invoices. Only
            // the fixed frame/continuation/location pops run outside them.
            self.reserve(3)?;
            let template_work = template.work_size();
            let parent_process = template.frames[0].key.process.is_some();
            let mut resumed = caller_template::into_owned_with_payment(template, self.budget)?;
            let child = resumed.frames.pop().expect("a subscribed child");
            resumed.continuations.pop();
            resumed.location.frames.pop();
            if !keep_store {
                resumed.store = if kind == OutcomeKind::Return {
                    output.summary.store.clone_with_payment(self.budget)?
                } else if parent_process {
                    // Rebind failure to this subscriber's checkpoint, retaining
                    // the native sticky-touch exception from the failed child.
                    // Other subscribers' unwritten data must not be replayed.
                    let mut failed = output
                        .summary
                        .store
                        .instantiate_with_payment(self.domain, self.budget)?;
                    let saved = child
                        .frame
                        .saved_store
                        .instantiate_with_payment(self.domain, self.budget)?;
                    // Rollback/touch traversal precedes the later census and
                    // needs its own allowance, beyond both native copies.
                    self.reserve(template_work.saturating_add(output.work))?;
                    failed.restore(crate::world::EvmSnapshot::from_state(saved));
                    failed.summary_projection_with_payment(self.domain, self.budget)?
                } else {
                    child.frame.saved_store
                };
            }
            // Changing the active frame and EvmStore<D::Element> invalidates current-read templates.
            resumed.storage_observations = Default::default();
            // This separate census visits the surviving input/checkpoint and
            // incoming effects. Their complete cached bounds fund the walk;
            // the earlier copy invoices do not pay for a second traversal.
            self.reserve(template_work.saturating_add(output.work))?;
            resumed.work = resumed.measure_work();
            let to =
                self.enqueue_summary(self.table.nodes[from].procedure, resumed, None, Some(from))?;
            self.observe_body_edge(from, to, MachineEdgeKind::Summary(kind))?;
            self.table.subscribers[subscriber]
                .delivered
                .insert(kind, stamp);
            self.regional_delivered(subscriber, kind);
            self.table.propagations += 1;
            return Ok(true);
        }
        let mut payload =
            payload::caller::instantiate_with_payment(&template, self.domain, self.budget)?;
        // Installed constraints still refer only to the old immutable snapshot.
        // Never reinstall those read bounds after replacing the EvmStore<D::Element>.
        if !keep_store {
            payload.store = output
                .summary
                .store
                .instantiate_with_payment(self.domain, self.budget)?;
        }
        let data = output
            .summary
            .data
            .instantiate_with_payment(self.domain, self.budget)?;
        #[cfg(test)]
        let native_input = super::tests::native_engine::delivery_input(&payload);
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
        self.regional_delivered(subscriber, kind);
        self.table.propagations += 1;
        #[cfg(test)]
        super::tests::native_engine::delivery_complete(native_input);
        Ok(true)
    }
}

pub(super) fn executed_call<N: crate::domain::WordElement>(
    source: &EvmPayloadSummary<N>,
    execution: &EvmExecution<N>,
) -> bool {
    execution.executed_pcs.last().is_some_and(|pc| {
        source.active().program.as_ref().is_some_and(|program| {
            program
                .blocks()
                .get(source.active().key.basic_block_index)
                .is_some_and(|block| {
                    block.instructions.iter().any(|instruction| {
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
        })
    })
}

pub(super) fn call_resume_status<N: crate::domain::WordElement>(
    local_call: bool,
    source_depth: usize,
    completed: &[crate::analysis::transfer::EvmCompletedCall<N>],
    successor: &crate::analysis::transfer::EvmSuccessor<N>,
) -> Option<bool> {
    if successor.payload.active().code != FrameCode::Runtime {
        return None;
    }
    let status = crate::analysis::engine::policy::call_result(successor.kind)?;
    let depth = successor.payload.call_stack.depth();
    let completed_call = completed.iter().any(|call| {
        call.payload.call_stack.depth() == depth.saturating_add(1)
            && call.payload.call_stack.active_child().is_some_and(|child| {
                child.continuation.creation.is_none()
                    && child.continuation.return_block
                        == Some(successor.payload.active().key.basic_block_index)
            })
            && (call.output_kind == OutcomeKind::Return) == status
    });
    (completed_call || (local_call && source_depth == depth)).then_some(status)
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
    // A refusal keeps the native node/subscriber bound in the table. Do not
    // invoke another native hook merely to decorate its frontier after denial.
    let current = if matches!(
        reason,
        FrontierReason::Work
            | FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
    ) || budget.exhausted()
    {
        None
    } else {
        payload::caller::instantiate_with_payment(summary, domain, budget).ok()
    };
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
