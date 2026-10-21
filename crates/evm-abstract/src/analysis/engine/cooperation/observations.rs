//! Publication seams for the interpreter's global structural observations.
use super::{Composition, Cooperation};
use crate::{
    analysis::{
        MachineError,
        cooperation::{
            Continuation, ContinuationId, CooperationError, Direction, Edge, InputTasks, Neighbors,
            Point, ProcedureId, Shared, Summary,
        },
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task},
        },
    },
    domain::WordElement,
    resource::WorkBudget,
};
use analysis_kernel::{
    GenerationEntry, KernelError, Meter, PublishError, Run, RunningJob, Scope, Scoped,
};
use std::{collections::BTreeSet, convert::Infallible, sync::Arc};

impl<N: WordElement, X: Composition<N, E>, E> Cooperation<N, X, E> {
    fn with_shared<R>(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
        operation: impl FnOnce(
            &mut Run<'_, Task<<X as InputTasks>::Task>, MachineMeter<'_>>,
            &mut Shared<N>,
            Scope,
        ) -> Result<R, CooperationError<E>>,
    ) -> Result<R, CooperationError<E>> {
        let mut meter = MachineMeter(budget);
        meter
            .reserve(((usize::BITS as usize) + 1) * 64)
            .map_err(KernelError::Stopped)?;
        let initialize = self.shared.is_none();
        if initialize {
            self.shared = Some(Shared::new(runtime.kernel(), &mut meter)?);
        }
        let universe = runtime.world;
        let mut run = runtime.kernel().resume(consumer, &mut meter)?;
        if initialize {
            run.publish(
                &mut self
                    .shared
                    .as_mut()
                    .ok_or(KernelError::InvalidHandle)?
                    .procedures,
                Scoped::new(universe, ()),
                BTreeSet::new(),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut self
                    .shared
                    .as_mut()
                    .ok_or(KernelError::InvalidHandle)?
                    .subscriptions,
                Scoped::new(universe, ()),
                BTreeSet::new(),
            )
            .map_err(publication_error)?;
        }
        let result = operation(
            &mut run,
            self.shared.as_mut().ok_or(KernelError::InvalidHandle)?,
            universe,
        );
        *consumer = run.suspend();
        result
    }

    pub(super) fn ensure_shared(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if self.shared.is_some() {
            return Ok(());
        }
        self.with_shared(runtime, consumer, budget, |_, _, _| Ok(()))
    }

    pub(super) fn publish_point(
        &mut self,
        point: Point,
        entry: &GenerationEntry,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.points,
                Scoped::new(universe, ()),
                BTreeSet::from([point]),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut shared.generations,
                Scoped::new(universe, point),
                entry.clone(),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut shared.callees,
                Scoped::new(universe, point),
                BTreeSet::new(),
            )
            .map_err(publication_error)?;
            for direction in [Direction::Predecessors, Direction::Successors] {
                run.publish(
                    &mut shared.topology,
                    Scoped::new(universe, Neighbors { point, direction }),
                    BTreeSet::new(),
                )
                .map_err(publication_error)?;
            }
            Ok(())
        })
    }

    /// This is independent of input admission: a covered payload still adds a
    /// topology assumption and wakes every tracked adjacency subscriber.
    pub(in crate::analysis) fn edge(
        &mut self,
        edge: Edge,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.points,
                Scoped::new(universe, ()),
                BTreeSet::from([edge.from, edge.to]),
            )
            .map_err(publication_error)?;
            for point in [edge.from, edge.to] {
                for direction in [Direction::Predecessors, Direction::Successors] {
                    run.publish(
                        &mut shared.topology,
                        Scoped::new(universe, Neighbors { point, direction }),
                        BTreeSet::new(),
                    )
                    .map_err(publication_error)?;
                }
            }
            for (point, direction) in [
                (edge.from, Direction::Successors),
                (edge.to, Direction::Predecessors),
            ] {
                run.publish(
                    &mut shared.topology,
                    Scoped::new(universe, Neighbors { point, direction }),
                    BTreeSet::from([edge]),
                )
                .map_err(publication_error)?;
            }
            Ok(())
        })
    }

    pub(in crate::analysis) fn procedure_entry(
        &mut self,
        id: ProcedureId,
        info: crate::analysis::cooperation::ProcedureInfo,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.procedures,
                Scoped::new(universe, ()),
                BTreeSet::from([id]),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut shared.entries,
                Scoped::new(universe, id),
                Arc::new(info),
            )
            .map_err(publication_error)?;
            Ok(())
        })
    }

    pub(in crate::analysis) fn call(
        &mut self,
        origin: Point,
        callee: ProcedureId,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.callees,
                Scoped::new(universe, origin),
                BTreeSet::from([callee]),
            )
            .map_err(publication_error)?;
            Ok(())
        })
    }

    pub(in crate::analysis) fn summary_closed(&self, id: ProcedureId) -> bool {
        self.summary_publications
            .get(&id)
            .is_some_and(|(_, closed, _)| *closed)
    }

    pub(in crate::analysis) fn summary(
        &mut self,
        id: ProcedureId,
        mut summary: Summary<N>,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        let native_revision = summary.revision;
        let closed = summary.closed;
        analysis_kernel::Meter::reserve(
            &mut MachineMeter(budget),
            ((usize::BITS as usize) + 1) * 16,
        )
        .map_err(KernelError::Stopped)?;
        let previous = self.summary_publications.get(&id).copied();
        if previous.is_some_and(|(native, was_closed, _)| {
            native == native_revision && was_closed == closed
        }) {
            return Ok(());
        }
        let revision = previous
            .map_or(Some(1), |(_, _, revision)| revision.checked_add(1))
            .ok_or(KernelError::VersionExhausted)?;
        summary.revision = revision;
        // Native clone/construction was paid at the adapter seam; retain only
        // one globally visible immutable snapshot for this stable procedure.
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.procedures,
                Scoped::new(universe, ()),
                BTreeSet::from([id]),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut shared.summaries,
                Scoped::new(universe, id),
                Arc::new(summary),
            )
            .map_err(publication_error)?;
            Ok(())
        })?;
        self.summary_publications
            .insert(id, (native_revision, closed, revision));
        Ok(())
    }

    pub(in crate::analysis) fn continuation(
        &mut self,
        id: ContinuationId,
        continuation: Continuation<N>,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        self.with_shared(runtime, consumer, budget, |run, shared, universe| {
            run.publish(
                &mut shared.subscriptions,
                Scoped::new(universe, ()),
                BTreeSet::from([id]),
            )
            .map_err(publication_error)?;
            run.publish(
                &mut shared.continuations,
                Scoped::new(universe, id),
                Arc::new(continuation),
            )
            .map_err(publication_error)?;
            Ok(())
        })
    }
}

fn publication_error<E>(error: PublishError<Infallible, MachineError>) -> CooperationError<E> {
    match error {
        PublishError::Kernel(error) => CooperationError::Protocol(error),
        PublishError::Policy(never) => match never {},
    }
}
