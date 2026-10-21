//! EVM task adapter over the shared kernel's admission and dependency protocol.
//!
//! A suspended World evaluation remains live while Body/Resume jobs drain the
//! same graph. Semantic stores own their typed slots; this owner keeps no second
//! copy of inputs or outputs. Every operation borrows the original root ledger.

use super::{MachineMeter, kernel_failure};
use crate::{analysis::MachineError, resource::WorkBudget};
use analysis_kernel::{
    Closure, JobId, Kernel, LocalClosure, Run, RunningJob, Scope, ScopeKey, kernel::KernelFork,
};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

/// Opaque static task sum of the production interpreter and its extension.
/// External components name this type through cooperation::EvmContext; only
/// the interpreter constructs machine/initialization tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task<T = std::convert::Infallible>(pub(in crate::analysis) TaskKind<T>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::analysis) enum TaskKind<T = std::convert::Infallible> {
    World { node: usize, depth: usize },
    Body(usize),
    Resume(usize),
    Seed(crate::analysis::cooperation::Subject),
    Extension(T),
    SummaryObservation,
}

type Scheduled<T> = Option<(std::sync::Arc<Task<T>>, RunningJob)>;

enum Graph<T> {
    Live(Kernel<Task<T>>),
    Staged(KernelFork<Task<T>>),
}

impl<T> Graph<T> {
    fn view(&self) -> &Kernel<Task<T>> {
        match self {
            Self::Live(kernel) => kernel,
            Self::Staged(fork) => fork.view(),
        }
    }

    fn kernel(&mut self) -> &mut Kernel<Task<T>> {
        match self {
            Self::Live(kernel) => kernel,
            Self::Staged(fork) => fork.kernel(),
        }
    }
}

/// One immutable composition run, including both ordinary and relative work.
pub(in crate::analysis) struct Runtime<T = std::convert::Infallible> {
    graph: Graph<T>,
    pub world: Scope,
    pub relative: Scope,
    world_jobs: BTreeMap<usize, JobId>,
    admission: Option<JobId>,
    observation: Option<JobId>,
}

impl<T> Runtime<T> {
    pub fn new(budget: &mut WorkBudget) -> Result<Self, MachineError> {
        let mut kernel = Kernel::new().map_err(|_| MachineError::IdentityExhausted)?;
        let mut meter = MachineMeter(budget);
        let world = kernel
            .scope(
                ScopeKey {
                    context: 0,
                    generation: 0,
                    policy: 0,
                },
                &mut meter,
            )
            .map_err(kernel_failure)?;
        let relative = kernel
            .scope(
                ScopeKey {
                    context: 1,
                    generation: 0,
                    policy: 0,
                },
                &mut meter,
            )
            .map_err(kernel_failure)?;
        // External inventories are complete. Dynamically discovered successors
        // still register through active work, as the kernel sealing contract allows.
        kernel
            .seal_scope(world, &mut meter)
            .map_err(kernel_failure)?;
        kernel
            .seal_scope(relative, &mut meter)
            .map_err(kernel_failure)?;
        Ok(Self {
            graph: Graph::Live(kernel),
            world,
            relative,
            world_jobs: BTreeMap::new(),
            admission: None,
            observation: None,
        })
    }

    pub fn view(&self) -> &Kernel<Task<T>> {
        self.graph.view()
    }

    pub fn kernel(&mut self) -> &mut Kernel<Task<T>> {
        self.graph.kernel()
    }

    pub fn register_world(
        &mut self,
        node: usize,
        depth: usize,
        budget: &mut WorkBudget,
    ) -> Result<JobId, MachineError> {
        if self.world_jobs.contains_key(&node) {
            return Err(MachineError::Shape);
        }
        let scope = self.world;
        let job = self
            .kernel()
            .register(
                scope,
                Task(TaskKind::World { node, depth }),
                &mut MachineMeter(budget),
            )
            .map_err(kernel_failure)?;
        self.world_jobs.insert(node, job);
        Ok(job)
    }

    /// Schedule the persistent native-summary producer on this kernel. Its
    /// owner is outside the relative scope, so it can report relative completion
    /// without including its own evaluation in that completion condition.
    pub fn schedule_observation(&mut self, budget: &mut WorkBudget) -> Result<(), MachineError> {
        if let Some(job) = self.observation {
            self.kernel()
                .retry(job, &mut MachineMeter(budget))
                .map_err(kernel_failure)
        } else {
            let scope = self.world;
            let job = self
                .kernel()
                .register(
                    scope,
                    Task(TaskKind::SummaryObservation),
                    &mut MachineMeter(budget),
                )
                .map_err(kernel_failure)?;
            self.observation = Some(job);
            Ok(())
        }
    }

    pub fn register_relative(
        &mut self,
        task: TaskKind<T>,
        budget: &mut WorkBudget,
    ) -> Result<JobId, MachineError> {
        let scope = self.relative;
        self.kernel()
            .register(scope, Task(task), &mut MachineMeter(budget))
            .map_err(kernel_failure)
    }

    /// Priority zero excludes World while a relative caller is suspended.
    pub fn next(
        &mut self,
        relative_only: bool,
        budget: &mut WorkBudget,
    ) -> Result<Scheduled<T>, MachineError> {
        let mut meter = MachineMeter(budget);
        let job = self
            .kernel()
            .next_deepest(&mut meter, |task| match &task.0 {
                TaskKind::World { depth, .. } if !relative_only => *depth,
                TaskKind::World { .. } => 0,
                TaskKind::SummaryObservation => 1,
                TaskKind::Body(_)
                | TaskKind::Resume(_)
                | TaskKind::Extension(_)
                | TaskKind::Seed(_) => usize::MAX,
            })
            .map_err(kernel_failure)?;
        let Some(job) = job else {
            return Ok(None);
        };
        self.admission = Some(job);
        let task = self
            .kernel()
            .task_handle(job, &mut meter)
            .map_err(kernel_failure)?;
        let token = self
            .kernel()
            .begin(job, &mut meter)
            .map_err(kernel_failure)?
            .suspend();
        self.admission = None;
        Ok(Some((task, token)))
    }

    pub fn next_external(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Scheduled<T>, analysis_kernel::KernelError<MachineError>> {
        let mut meter = MachineMeter(budget);
        let job = self
            .kernel()
            .next_deepest(&mut meter, |task| match &task.0 {
                TaskKind::Extension(_) | TaskKind::Seed(_) => 1,
                _ => 0,
            })?;
        let Some(job) = job else {
            return Ok(None);
        };
        self.admission = Some(job);
        let task = self.kernel().task_handle(job, &mut meter)?;
        let token = self.kernel().begin(job, &mut meter)?.suspend();
        self.admission = None;
        Ok(Some((task, token)))
    }

    /// Temporary borrows never finish an evaluation before its interpreter step.
    pub fn with_run<R>(
        &mut self,
        token: &mut RunningJob,
        budget: &mut WorkBudget,
        operation: impl FnOnce(&mut Run<'_, Task<T>, MachineMeter<'_>>) -> Result<R, MachineError>,
    ) -> Result<R, MachineError> {
        let mut meter = MachineMeter(budget);
        let mut run = self
            .kernel()
            .resume(token, &mut meter)
            .map_err(kernel_failure)?;
        let result = operation(&mut run);
        *token = run.suspend();
        result
    }

    pub fn finish(
        &mut self,
        mut token: RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), MachineError> {
        self.kernel()
            .resume(&mut token, &mut MachineMeter(budget))
            .map_err(kernel_failure)?
            .finish(LocalClosure::Stable)
            .map_err(kernel_failure)
    }

    pub fn contains_world(&self, node: &usize) -> bool {
        self.world_jobs
            .get(node)
            .is_some_and(|job| self.graph.view().worklist().contains(job))
    }

    pub fn pending_world(&self) -> impl Iterator<Item = usize> + '_ {
        let mut seen = BTreeSet::new();
        let admitted =
            self.admission
                .and_then(|job| match self.graph.view().task::<MachineError>(job) {
                    Ok(Task(TaskKind::World { node, .. })) => Some(*node),
                    _ => None,
                });
        admitted
            .into_iter()
            .chain(self.graph.view().worklist().in_order().filter_map(|job| {
                match self.graph.view().task::<MachineError>(job) {
                    Ok(Task(TaskKind::World { node, .. })) => Some(*node),
                    _ => None,
                }
            }))
            .filter(move |node| seen.insert(*node))
    }

    #[cfg(test)]
    pub fn pending_body(&self) -> impl Iterator<Item = usize> + '_ {
        self.graph.view().worklist().pending().filter_map(|job| {
            match self.graph.view().task::<MachineError>(job) {
                Ok(Task(TaskKind::Body(node))) => Some(*node),
                _ => None,
            }
        })
    }

    pub fn pending_relative(&self) -> impl Iterator<Item = TaskKind<T>> + '_ {
        let task = |job| match self.graph.view().task::<MachineError>(job) {
            Ok(Task(TaskKind::Body(node))) => Some(TaskKind::Body(*node)),
            Ok(Task(TaskKind::Resume(subscriber))) => Some(TaskKind::Resume(*subscriber)),
            _ => None,
        };
        self.admission
            .and_then(task)
            .into_iter()
            .chain(self.graph.view().worklist().pending().filter_map(task))
    }

    pub fn relative_closed(&self, budget: &mut WorkBudget) -> Result<bool, MachineError> {
        self.graph
            .view()
            .closure_in_metered(self.relative, &mut MachineMeter(budget))
            .map(|closure| closure == Closure::Closed)
            .map_err(kernel_failure)
    }

    pub fn closed(&self) -> bool {
        self.graph.view().closure() == Closure::Closed
    }

    pub fn work_size(&self) -> usize {
        self.world_jobs
            .len()
            .saturating_add(self.graph.view().worklist().work_size())
    }

    /// A certified exit can satisfy a queued World job in the staged graph.
    pub fn satisfy_world<V: Clone>(
        &mut self,
        node: usize,
        slot: &analysis_kernel::SlotMeta<V>,
        value: &V,
        clone_work: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), MachineError> {
        if !self.contains_world(&node) {
            return Ok(());
        }
        let job = self.world_jobs[&node];
        // Only non-running queued imports are satisfied here. A self-dirty
        // active importer retains its next evaluation rather than erasing it.
        match self.kernel().begin(job, &mut MachineMeter(budget)) {
            Ok(mut run) => {
                run.read_slot(slot, value, clone_work)
                    .map_err(kernel_failure)?;
                run.finish(LocalClosure::Stable).map_err(kernel_failure)
            }
            Err(analysis_kernel::KernelError::AlreadyRunning) => Ok(()),
            Err(error) => Err(kernel_failure(error)),
        }
    }

    pub fn fork(&self, budget: &mut WorkBudget) -> Result<Self, MachineError> {
        let fork = self
            .graph
            .view()
            .fork(&mut MachineMeter(budget))
            .map_err(kernel_failure)?;
        Ok(Self {
            graph: Graph::Staged(fork),
            world: self.world,
            relative: self.relative,
            world_jobs: self.world_jobs.clone(),
            admission: self.admission,
            observation: self.observation,
        })
    }

    #[cfg(test)]
    pub fn queue_snapshot(&self) -> (Vec<Task<T>>, usize)
    where
        T: Clone,
    {
        (
            self.graph
                .view()
                .worklist()
                .pending()
                .filter_map(|job| self.graph.view().task::<MachineError>(job).ok().cloned())
                .collect(),
            self.graph.view().worklist().records(),
        )
    }

    #[cfg(test)]
    pub fn scan_work(&self) -> usize {
        self.graph.view().worklist().records() * 2 + 1
    }

    #[cfg(test)]
    pub fn requeue_world(
        &mut self,
        node: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), MachineError> {
        let job = self.world_jobs[&node];
        self.kernel()
            .retry(job, &mut MachineMeter(budget))
            .map_err(kernel_failure)
    }

    #[cfg(test)]
    pub fn test_driver(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<(JobId, RunningJob), MachineError> {
        let scope = self.world;
        let job = self
            .kernel()
            .register(
                scope,
                Task(TaskKind::World {
                    node: usize::MAX,
                    depth: 0,
                }),
                &mut MachineMeter(budget),
            )
            .map_err(kernel_failure)?;
        let token = self
            .kernel()
            .begin(job, &mut MachineMeter(budget))
            .map_err(kernel_failure)?
            .suspend();
        Ok((job, token))
    }

    /// Semantic graph and dependency graph become visible in one final commit.
    pub fn commit(
        &mut self,
        staged: Self,
        budget: &mut WorkBudget,
        install: impl FnOnce(),
    ) -> Result<(), MachineError> {
        let Graph::Staged(fork) = staged.graph else {
            return Err(MachineError::Shape);
        };
        let jobs = &mut self.world_jobs;
        let admitted = &mut self.admission;
        let observation = &mut self.observation;
        self.graph
            .kernel()
            .commit_with(fork, &mut MachineMeter(budget), 1, || {
                *jobs = staged.world_jobs;
                *admitted = staged.admission;
                *observation = staged.observation;
                install();
            })
            .map_err(kernel_failure)
    }
}
