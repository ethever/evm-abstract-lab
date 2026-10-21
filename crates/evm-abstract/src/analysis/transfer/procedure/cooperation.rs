//! Input contribution and body views for the shared production cooperation ports.
//!
//! Raw successor scopes preserve computed-expression relationships before
//! numerical template projection. Body scopes describe the retained ascending
//! bound. Neither view can replace a live anchor with a narrowed value.

use super::{
    Composition, ContextPolicy, DiagnosticKind, EvmMachinePayload, EvmSolver, MachineEdgeKind,
    MachineError, PartitionPolicy, StatePolicy,
};
use crate::{analysis::worklist::kernel_failure, domain::EvmNumericAlgebra};
use analysis_kernel::Version;

#[derive(Clone, Copy)]
pub(super) struct Producer {
    pub(super) kind: u8,
    pub(super) id: usize,
    pub(super) case: u8,
    pub(super) input: Version,
    pub(super) other: Option<Version>,
    pub(super) parent: Option<crate::analysis::cooperation::Subject>,
}

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    /// A publication/withdrawal receipt must still refer to the native snapshot
    /// evaluated by this job. This metadata-only check does not clone a payload
    /// or stand in for the tracked semantic input reads/assumption bridge.
    pub(super) fn source_stamp(
        &mut self,
        task: &crate::analysis::worklist::runtime::TaskKind<
            <X as crate::analysis::cooperation::InputTasks>::Task,
        >,
    ) -> Result<Option<(Version, Option<Version>)>, MachineError> {
        use crate::analysis::worklist::runtime::TaskKind;
        if !X::ENABLED || !matches!(task, TaskKind::Body(_) | TaskKind::Resume(_)) {
            return Ok(None);
        }
        let token = self.active.as_mut().ok_or(MachineError::Shape)?;
        match task {
            TaskKind::Body(id) => self.runtime.with_run(token, self.budget, |run| {
                Ok(Some((
                    run.slot_version(&self.table.nodes[*id].input)
                        .map_err(crate::analysis::worklist::kernel_failure)?,
                    None,
                )))
            }),
            TaskKind::Resume(id) => {
                let subscription = &self.table.subscribers[*id];
                self.runtime.with_run(token, self.budget, |run| {
                    Ok(Some((
                        run.slot_version(&subscription.input)
                            .map_err(crate::analysis::worklist::kernel_failure)?,
                        Some(
                            run.slot_version(&self.table.procedures[subscription.callee].output)
                                .map_err(crate::analysis::worklist::kernel_failure)?,
                        ),
                    )))
                })
            }
            _ => Ok(None),
        }
    }

    pub(super) fn cooperative_input(
        &mut self,
        id: usize,
        payload: EvmMachinePayload<D::Element>,
        phase: u8,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        if !X::ENABLED {
            return Ok(Some(payload));
        }
        let slot = &self.table.nodes[id].input;
        let version = self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| run.slot_version(slot).map_err(kernel_failure),
        )?;
        let view = self
            .cooperation
            .input(
                crate::analysis::cooperation::Point {
                    namespace: 1,
                    kind: crate::analysis::cooperation::PointKind::Body,
                    index: id,
                },
                version,
                phase,
                None,
                None,
                payload,
                self.domain,
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
                |run| {
                    if run.observe_slot(slot)? != version {
                        return Err(analysis_kernel::KernelError::InvalidHandle);
                    }
                    Ok(())
                },
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        if !matches!(view, crate::analysis::cooperation::LocalInput::Deferred) {
            self.consumed = self
                .cooperation
                .subject(crate::analysis::cooperation::Point {
                    namespace: 1,
                    kind: crate::analysis::cooperation::PointKind::Body,
                    index: id,
                });
        }
        match view {
            crate::analysis::cooperation::LocalInput::Deferred => {
                self.deferred = true;
                self.table.nodes[id].excluded = false;
                Ok(None)
            }
            crate::analysis::cooperation::LocalInput::Unreachable => {
                self.table.nodes[id].excluded = true;
                Ok(None)
            }
            crate::analysis::cooperation::LocalInput::Reachable { payload, limited } => {
                self.reserve(limited.len().saturating_mul(16))?;
                self.table
                    .precision_losses
                    .extend(limited.into_iter().map(DiagnosticKind::FactExchangeLimited));
                self.table.nodes[id].excluded = false;
                Ok(Some(*payload))
            }
        }
    }

    pub(super) fn track_input(&mut self, id: usize) -> Result<Version, MachineError> {
        // The body already owns its immutable executable snapshot. Observe its
        // retained source and assumptions without copying the numerical bound.
        self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                run.observe_slot(&self.table.nodes[id].input)
                    .map_err(kernel_failure)
            },
        )
    }

    pub(super) fn incoming(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        kind: MachineEdgeKind,
        ordinal: usize,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        if !X::ENABLED {
            return Ok(Some(payload));
        }
        let source = self.producer.ok_or(MachineError::Shape)?;
        self.reserve(payload.work_size().saturating_mul(2))?;
        let key_work = payload.call_stack.iter().fold(1usize, |work, frame| {
            work.saturating_add(frame.key.jump_history.len().saturating_add(32))
        });
        self.reserve(
            key_work
                .saturating_mul((usize::BITS as usize) + 1)
                .saturating_mul(4),
        )?;
        let key = (
            source.kind,
            source.id,
            source.case,
            payload.key(),
            kind,
            ordinal,
        );
        let next = self.table.exchange_inputs.len();
        let index = *self.table.exchange_inputs.entry(key).or_insert(next);
        let node_slot = (source.kind == 0).then(|| &self.table.nodes[source.id].input);
        let subscription = (source.kind == 1).then(|| &self.table.subscribers[source.id]);
        let output_slot =
            subscription.map(|subscription| &self.table.procedures[subscription.callee].output);
        let view = self
            .cooperation
            .input(
                crate::analysis::cooperation::Point {
                    namespace: 2,
                    kind: crate::analysis::cooperation::PointKind::Body,
                    index,
                },
                source.input,
                0,
                source.other,
                source.parent,
                payload,
                self.domain,
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
                |run| {
                    let version = if let Some(slot) = node_slot {
                        run.observe_slot(slot)?
                    } else {
                        run.observe_slot(
                            &subscription
                                .ok_or(analysis_kernel::KernelError::InvalidHandle)?
                                .input,
                        )?
                    };
                    if version != source.input {
                        return Err(analysis_kernel::KernelError::InvalidHandle);
                    }
                    if let Some(slot) = output_slot
                        && Some(run.observe_slot(slot)?) != source.other
                    {
                        return Err(analysis_kernel::KernelError::InvalidHandle);
                    }
                    Ok(())
                },
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        match view {
            crate::analysis::cooperation::LocalInput::Deferred => {
                self.deferred = true;
                Ok(None)
            }
            crate::analysis::cooperation::LocalInput::Unreachable => Ok(None),
            crate::analysis::cooperation::LocalInput::Reachable { payload, limited } => {
                self.reserve(limited.len().saturating_mul(16))?;
                self.table
                    .precision_losses
                    .extend(limited.into_iter().map(DiagnosticKind::FactExchangeLimited));
                Ok(Some(*payload))
            }
        }
    }
}

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    pub(super) fn source_inventory_changed(&mut self) -> Result<(), MachineError> {
        self.cooperation
            .structure_changed(
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    fn body_point(index: usize) -> crate::analysis::cooperation::Point {
        crate::analysis::cooperation::Point {
            namespace: 1,
            kind: crate::analysis::cooperation::PointKind::Body,
            index,
        }
    }

    pub(super) fn observe_procedure(&mut self, id: usize) -> Result<(), MachineError> {
        if !X::ENABLED {
            return Ok(());
        }
        let work = self.table.procedures[id]
            .frame
            .jump_history
            .len()
            .saturating_add(64);
        self.reserve(work.saturating_mul(2))?;
        let info = crate::analysis::cooperation::ProcedureInfo {
            entry: crate::analysis::cooperation::Point {
                namespace: 1,
                kind: crate::analysis::cooperation::PointKind::ProcedureEntry,
                index: id,
            },
            frame: self.table.procedures[id].frame.clone(),
        };
        self.cooperation
            .procedure_entry(
                crate::analysis::cooperation::ProcedureId {
                    namespace: 1,
                    index: id,
                },
                info,
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    pub(super) fn observe_origin(
        &mut self,
        origin: crate::analysis::cooperation::Point,
        id: usize,
    ) -> Result<(), MachineError> {
        self.cooperation
            .call(
                origin,
                crate::analysis::cooperation::ProcedureId {
                    namespace: 1,
                    index: id,
                },
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    pub(super) fn observe_body_edge(
        &mut self,
        from: usize,
        to: usize,
        kind: MachineEdgeKind,
    ) -> Result<(), MachineError> {
        let edge = crate::analysis::cooperation::Edge {
            from: Self::body_point(from),
            to: Self::body_point(to),
            kind,
        };
        self.cooperation
            .edge(
                edge,
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    pub(super) fn observe_call_edge(
        &mut self,
        from: usize,
        callee: usize,
    ) -> Result<(), MachineError> {
        self.cooperation
            .edge(
                crate::analysis::cooperation::Edge {
                    from: Self::body_point(from),
                    to: crate::analysis::cooperation::Point {
                        namespace: 1,
                        kind: crate::analysis::cooperation::PointKind::ProcedureEntry,
                        index: callee,
                    },
                    kind: MachineEdgeKind::Call,
                },
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    fn summary_snapshot(
        &mut self,
        id: usize,
    ) -> Result<crate::analysis::cooperation::Summary<D::Element>, MachineError> {
        let procedure = &self.table.procedures[id];
        let work = procedure.outputs.values().fold(8usize, |work, output| {
            work.saturating_add(output.work).saturating_add(4)
        });
        self.reserve(work.saturating_mul(2))?;
        let procedure = &self.table.procedures[id];
        let outputs = procedure
            .outputs
            .iter()
            .map(|(kind, output)| {
                (
                    *kind,
                    crate::analysis::cooperation::SummaryOutput {
                        data: output.summary.data.clone(),
                        store: output.summary.store.clone(),
                        generation: output.generation,
                    },
                )
            })
            .collect();
        let summary =
            crate::analysis::cooperation::Summary::new(procedure.output_updates, outputs, work);
        Ok(summary)
    }

    pub(super) fn observe_summary(&mut self, _id: usize, closed: bool) -> Result<(), MachineError> {
        if X::ENABLED && !closed {
            self.runtime.schedule_observation(self.budget)?;
        }
        Ok(())
    }

    pub(super) fn observe_continuation(&mut self, id: usize) -> Result<(), MachineError> {
        if !X::ENABLED {
            return Ok(());
        }
        let work = self.table.subscribers[id]
            .payload
            .work_size()
            .saturating_add(8);
        self.reserve(work.saturating_mul(2))?;
        let subscription = &self.table.subscribers[id];
        let continuation = crate::analysis::cooperation::Continuation::new(
            subscription
                .updates
                .checked_add(1)
                .ok_or(MachineError::IdentityExhausted)?,
            Self::body_point(subscription.from),
            crate::analysis::cooperation::ProcedureId {
                namespace: 1,
                index: subscription.callee,
            },
            subscription.payload.clone(),
            work,
        );
        self.cooperation
            .continuation(
                crate::analysis::cooperation::ContinuationId {
                    namespace: 1,
                    index: id,
                },
                continuation,
                self.runtime,
                self.active
                    .as_mut()
                    .or(self.caller.as_deref_mut())
                    .ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))
    }

    /// This persistent producer runs after Body/Resume work. Its world-owned
    /// token is a real semantic job, independent of any suspended World caller.
    /// The persistent native owner publishes incremental MAY snapshots before
    /// evaluating the separately guarded whole-table completion proof.
    pub(super) fn complete_observations(&mut self) -> Result<(), MachineError> {
        if !X::ENABLED {
            return Ok(());
        }
        // Publish the actual retained entries first. This can invalidate old
        // guarantees and enqueue callbacks/native owners anywhere in this run.
        for id in 0..self.table.procedures.len() {
            let source = self.table.entry_source(id).ok_or(MachineError::Shape)?;
            let payload = source.payload;
            let domain = self.domain;
            self.cooperation
                .publish_entry_input(
                    crate::analysis::cooperation::ProcedureId {
                        namespace: 1,
                        index: id,
                    },
                    *source.revision,
                    source.slot,
                    self.runtime,
                    self.active.as_mut().ok_or(MachineError::Shape)?,
                    self.budget,
                    |budget| {
                        if !budget.charge(payload.work_size().saturating_mul(4)) {
                            return Err(MachineError::work_failure());
                        }
                        super::instantiate_with(payload, domain)
                    },
                )
                .map_err(|error| self.cooperation.stop(error, self.budget))?;
        }
        self.cooperation
            .drain_observers(
                self.runtime,
                self.active.as_mut().ok_or(MachineError::Shape)?,
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        let closed = self.runtime.relative_closed(self.budget)?
            && self.table.frontiers.is_empty()
            && !self.table.incomplete;
        self.reserve(self.table.procedures.len().saturating_mul(4))?;
        let mut summaries = Vec::with_capacity(self.table.procedures.len());
        for id in 0..self.table.procedures.len() {
            summaries.push((
                crate::analysis::cooperation::ProcedureId {
                    namespace: 1,
                    index: id,
                },
                self.summary_snapshot(id)?,
            ));
        }
        let table = &self.table;
        self.cooperation
            .publish_summaries(
                summaries,
                closed,
                self.runtime,
                self.active.as_mut().ok_or(MachineError::Shape)?,
                self.budget,
                |run| {
                    run.reserve(
                        table
                            .procedures
                            .len()
                            .saturating_mul(2)
                            .saturating_add(table.nodes.len())
                            .saturating_add(table.subscribers.len()),
                    )?;
                    for procedure in &table.procedures {
                        run.observe_slot(&procedure.entry_source)?;
                        run.observe_slot(&procedure.output)?;
                    }
                    for node in &table.nodes {
                        run.observe_slot(&node.input)?;
                    }
                    for subscriber in &table.subscribers {
                        run.observe_slot(&subscriber.input)?;
                    }
                    Ok(())
                },
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        Ok(())
    }
}

impl<N: crate::domain::WordElement, Key: super::PolicyKey> super::EvmTable<N, Key> {
    /// Reenter the persistent relative interpreter with an already admitted job.
    /// There is no synthetic World job, duplicate registration or second queue.
    #[expect(
        clippy::too_many_arguments,
        reason = "same persistent interpreter environment and admitted kernel token"
    )]
    pub(in crate::analysis) fn resume_task<
        D: EvmNumericAlgebra<Element = N>,
        C: ContextPolicy,
        P: PartitionPolicy,
        S: StatePolicy<D, P::Class, Key = Key>,
        X: Composition<N, E>,
        E,
    >(
        &mut self,
        task: std::sync::Arc<
            crate::analysis::worklist::runtime::Task<
                <X as crate::analysis::cooperation::InputTasks>::Task,
            >,
        >,
        token: analysis_kernel::RunningJob,
        world: &crate::world::EvmWorld<N>,
        entry: &crate::world::EvmEntry<N>,
        config: &crate::analysis::ExecutionConfig,
        domain: crate::domain::AlgebraRef<'_, D>,
        policies: &super::EnginePolicies<C, P, S>,
        budget: &mut crate::resource::WorkBudget,
        counters: &mut super::Counters,
        storage_reads: super::StorageReadPolicy,
        runtime: &mut crate::analysis::worklist::runtime::Runtime<
            <X as crate::analysis::cooperation::InputTasks>::Task,
        >,
        cooperation: &mut crate::analysis::engine::cooperation::Cooperation<N, X, E>,
    ) -> Vec<super::EvmExecutionFrontier<N>> {
        let mut solver = EvmSolver {
            table: self,
            world,
            entry,
            config,
            domain,
            policies,
            budget,
            counters,
            storage_reads,
            runtime,
            cooperation,
            active: None,
            caller: None,
            namespace: 0,
            producer: None,
            deferred: false,
            consumed: None,
        };
        if solver.run_admitted(task, token) {
            solver.run();
        }
        solver.table.incomplete |= !solver.table.frontiers.is_empty();
        std::mem::take(&mut solver.table.frontiers)
    }
}
