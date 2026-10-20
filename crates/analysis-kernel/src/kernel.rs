//! Shared dependency ownership, metered admission and explicit obligations.

use crate::{
    Capability, Channel, Closure, Epoch, IdentityExhausted, KernelError, Open, Scope, ScopeKey,
    Scoped, Version,
    algebra::PublicationPolicy,
    capability::{Availability, Cell},
    worklist::Worklist,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
};

mod run;
mod snapshot;
pub use run::{GenerationEntry, Run, RunningJob, SlotMeta};
pub use snapshot::KernelFork;

/// A cumulative root ledger; refusing work never grants a fresh budget.
pub trait Meter {
    /// Original typed stop identity, including cancellation when applicable.
    type Stop;
    /// Reserve a trustworthy work upper bound before performing an operation.
    fn reserve(&mut self, cost: usize) -> Result<(), Self::Stop>;
    /// Check cancellation without mutating scheduling state.
    fn checkpoint(&self) -> Result<(), Self::Stop>;
}

/// One job in one analysis epoch; semantic task data remains statically typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId {
    epoch: Epoch,
    birth: Epoch,
    index: usize,
}
/// An explicit adapter-owned uncovered semantic obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObligationId {
    epoch: Epoch,
    birth: Epoch,
    index: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FactId {
    epoch: Epoch,
    birth: Epoch,
    index: usize,
}

#[derive(Clone)]
struct Fact {
    id: FactId,
    scope: Option<Scope>,
    version: Version,
    ready: bool,
    readers: BTreeSet<JobId>,
    providers: BTreeSet<JobId>,
}
impl Fact {
    fn new(id: FactId, scope: Option<Scope>, ready: bool) -> Self {
        Self {
            id,
            scope,
            version: Version::default(),
            ready,
            readers: BTreeSet::new(),
            providers: BTreeSet::new(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum JobState {
    Idle,
    Admitted,
    Running,
    Suspended,
    Interrupted,
    Retired,
}
struct Job<J> {
    id: JobId,
    scope: Scope,
    task: Arc<J>,
    reads: BTreeMap<FactId, Version>,
    state: JobState,
    evaluation: Version,
    evaluation_birth: Epoch,
    limited: bool,
}
#[derive(Clone)]
struct Generation {
    scope: Scope,
    head: FactId,
    active: bool,
    sealed: bool,
    jobs: BTreeSet<JobId>,
}
#[derive(Clone)]
struct Obligation {
    id: ObligationId,
    scope: Scope,
    resolved: bool,
}

/// Typed-task scheduler and fact metadata; no semantic value is erased here.
///
/// All allocating transitions reserve before installation. IDs are never reused
/// within the epoch. Individual publications commit atomically; jobs may publish
/// multiple times, and interruption retains earlier publications and the current
/// obligation. Retired generations remain as checked tombstones.
pub struct Kernel<J> {
    epoch: Epoch,
    allocation: Epoch,
    revision: u64,
    stats: Statistics,
    facts: Vec<Fact>,
    jobs: Vec<Job<J>>,
    scopes: Vec<Generation>,
    scope_ids: BTreeMap<ScopeKey, Scope>,
    obligations: Vec<Obligation>,
    worklist: Worklist<JobId>,
}

/// Observable graph retention and propagation costs within one run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    /// Retained fact identities, including retired tombstones.
    pub facts: usize,
    /// Retained task identities.
    pub jobs: usize,
    /// Retained scope identities.
    pub scopes: usize,
    /// Currently retained read edges.
    pub dependencies: usize,
    /// Notifications examined, including duplicate readers across publications.
    pub wake_attempts: usize,
    /// Successfully begun evaluations, excluding temporary resumes.
    pub evaluations: usize,
}

pub(crate) fn reserve<M: Meter>(meter: &mut M, cost: usize) -> Result<(), KernelError<M::Stop>> {
    if cost == usize::MAX {
        return Err(KernelError::CostOverflow);
    }
    meter.checkpoint().map_err(KernelError::Stopped)?;
    meter.reserve(cost).map_err(KernelError::Stopped)
}
fn tree_work(len: usize) -> usize {
    len.saturating_add(1).ilog2() as usize + 2
}

impl<J> Kernel<J> {
    /// Create an independent analysis epoch without semantic allocations.
    pub fn new() -> Result<Self, IdentityExhausted> {
        let epoch = Epoch::fresh()?;
        Ok(Self {
            epoch,
            allocation: epoch,
            revision: 0,
            stats: Statistics::default(),
            facts: Vec::new(),
            jobs: Vec::new(),
            scopes: Vec::new(),
            scope_ids: BTreeMap::new(),
            obligations: Vec::new(),
            worklist: Worklist::default(),
        })
    }
    /// Namespace used to reject cross-analysis handles.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    // A saturated revision permanently disables fork/commit; ordinary solving
    // still works. Drop cannot fail, so it must also be able to invalidate forks.
    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn check_scope<S>(&self, scope: Scope) -> Result<(), KernelError<S>> {
        if scope.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        match self.scopes.get(scope.index) {
            Some(g) if g.scope == scope && g.active => Ok(()),
            _ => Err(KernelError::InactiveScope),
        }
    }
    fn check_job<S>(&self, id: JobId) -> Result<(), KernelError<S>> {
        if id.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        let job = self.jobs.get(id.index).ok_or(KernelError::InvalidJob)?;
        if job.id != id || job.state == JobState::Retired {
            return Err(KernelError::InvalidJob);
        }
        self.check_scope(job.scope)
    }
    fn check_fact<S>(&self, id: FactId) -> Result<(), KernelError<S>> {
        if id.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        let fact = self.facts.get(id.index).ok_or(KernelError::InvalidHandle)?;
        if fact.id != id {
            return Err(KernelError::InvalidHandle);
        }
        if let Some(scope) = fact.scope {
            self.check_scope(scope)?;
        }
        Ok(())
    }
    fn push_fact(&mut self, scope: Option<Scope>, ready: bool) -> FactId {
        let id = FactId {
            epoch: self.epoch,
            birth: self.allocation,
            index: self.facts.len(),
        };
        self.facts.push(Fact::new(id, scope, ready));
        self.touch();
        id
    }

    /// Intern a semantic input generation and open its registration admission.
    ///
    /// Seal it after registering mandatory jobs/providers. Reusing a retired
    /// key is rejected; an expanded global input needs a fresh generation key.
    pub fn scope<M: Meter>(
        &mut self,
        key: ScopeKey,
        meter: &mut M,
    ) -> Result<Scope, KernelError<M::Stop>> {
        reserve(meter, tree_work(self.scope_ids.len()))?;
        if let Some(scope) = self.scope_ids.get(&key).copied() {
            self.check_scope(scope)?;
            return Ok(scope);
        }
        reserve(meter, 7 + tree_work(self.scope_ids.len()))?;
        let scope = Scope {
            epoch: self.epoch,
            birth: self.allocation,
            index: self.scopes.len(),
            key,
        };
        let head = self.push_fact(None, true);
        self.scopes.push(Generation {
            scope,
            head,
            active: true,
            sealed: false,
            jobs: BTreeSet::new(),
        });
        self.scope_ids.insert(key, scope);
        Ok(scope)
    }
    /// Attest the initial/external inventory is complete, not that solving ended.
    ///
    /// Dynamically discovered jobs/providers remain admissible and are counted
    /// immediately. Future external arrivals must retain an explicit obligation.
    pub fn seal_scope<M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_scope(scope)?;
        reserve(meter, 1)?;
        self.scopes[scope.index].sealed = true;
        self.touch();
        Ok(())
    }
    /// Retire one generation after the adapter has admitted its sound replacement.
    ///
    /// The adapter asserts replacement covers the old input; global contributions
    /// are not retracted. Cross-scope readers of its head are awakened. New local
    /// facts must be seeded independently, rather than copying old must-facts.
    pub fn retire_scope<M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_scope(scope)?;
        let head = self.scopes[scope.index].head;
        let work = self.retirement_work(scope, meter)?;
        self.prepare_changes(&[head], meter, work)?;
        self.install_retirement(scope);
        self.install_changes(&[head]);
        self.release_generation(scope);
        Ok(())
    }
    fn retirement_work<M: Meter>(
        &self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        let jobs = &self.scopes[scope.index].jobs;
        // Quote edge removals only after paying the per-job metadata scan.
        reserve(
            meter,
            jobs.len()
                .saturating_add(self.facts.len())
                .saturating_add(1),
        )?;
        let head = self.scopes[scope.index].head;
        let cleanup = self.facts.iter().fold(0usize, |cost, fact| {
            cost.saturating_add(fact.providers.len()).saturating_add(
                if fact.scope == Some(scope) || fact.id == head {
                    fact.readers
                        .len()
                        .saturating_mul(tree_work(self.facts.len()).saturating_add(2))
                } else {
                    0
                },
            )
        });
        Ok(jobs
            .iter()
            .fold(cleanup.saturating_add(self.facts.len()), |cost, id| {
                cost.saturating_add(tree_work(self.worklist.len()))
                    .saturating_add(
                        self.jobs[id.index]
                            .reads
                            .len()
                            .saturating_mul(tree_work(self.jobs.len()).saturating_add(2)),
                    )
            }))
    }
    fn install_retirement(&mut self, scope: Scope) {
        self.scopes[scope.index].active = false;
        for id in &self.scopes[scope.index].jobs {
            let job = &mut self.jobs[id.index];
            job.state = JobState::Retired;
            self.worklist.satisfy(*id);
            for (fact, _) in std::mem::take(&mut job.reads) {
                self.facts[fact.index].readers.remove(id);
            }
        }
        self.touch();
    }
    fn release_generation(&mut self, scope: Scope) {
        let head = self.scopes[scope.index].head;
        for fact in &mut self.facts {
            // Retiring a producer does not retract any already published global
            // contribution, but its provider bookkeeping no longer has a role.
            fact.providers
                .retain(|id| self.jobs[id.index].scope != scope);
            if fact.scope == Some(scope) || fact.id == head {
                for reader in std::mem::take(&mut fact.readers) {
                    self.jobs[reader.index].reads.remove(&fact.id);
                }
            }
        }
        self.touch();
    }
    /// Construct an explicitly routed typed channel, including its directory fact.
    pub fn channel<Q: Capability, P: PublicationPolicy<Value = Q::Value>, M: Meter>(
        &mut self,
        policy: P,
        meter: &mut M,
    ) -> Result<Channel<Q, P>, KernelError<M::Stop>> {
        reserve(meter, 4)?;
        let directory = self.push_fact(None, true);
        Ok(Channel {
            epoch: self.epoch,
            directory,
            cells: BTreeMap::new(),
            policy,
            marker: PhantomData,
        })
    }
    /// Register and queue one independently implemented job.
    ///
    /// Task data is moved, not deeply cloned. Its construction belongs to the
    /// adapter's ledger; this call accounts the retained handle and graph data.
    pub fn register<M: Meter>(
        &mut self,
        scope: Scope,
        task: J,
        meter: &mut M,
    ) -> Result<JobId, KernelError<M::Stop>> {
        self.check_scope(scope)?;
        reserve(
            meter,
            7 + tree_work(self.scopes[scope.index].jobs.len()) + self.worklist.enqueue_work(),
        )?;
        let id = JobId {
            epoch: self.epoch,
            birth: self.allocation,
            index: self.jobs.len(),
        };
        self.jobs.push(Job {
            id,
            scope,
            task: Arc::new(task),
            reads: BTreeMap::new(),
            state: JobState::Idle,
            evaluation: Version::default(),
            evaluation_birth: self.allocation,
            limited: false,
        });
        self.scopes[scope.index].jobs.insert(id);
        self.worklist.enqueue(id);
        self.touch();
        Ok(id)
    }
    /// Borrow task data without copying an application carrier.
    pub fn task<S>(&self, id: JobId) -> Result<&J, KernelError<S>> {
        self.check_job(id)?;
        Ok(&self.jobs[id.index].task)
    }
    /// Retain a constant-size task handle for evaluation while metadata mutates.
    pub fn task_handle<M: Meter>(
        &self,
        id: JobId,
        meter: &mut M,
    ) -> Result<Arc<J>, KernelError<M::Stop>> {
        self.check_job(id)?;
        reserve(meter, 1)?;
        Ok(Arc::clone(&self.jobs[id.index].task))
    }
    /// Scope of a live job, including admitted/in-flight jobs.
    pub fn job_scope<S>(&self, id: JobId) -> Result<Scope, KernelError<S>> {
        self.check_job(id)?;
        Ok(self.jobs[id.index].scope)
    }
    /// Requeue interrupted or externally changed work without losing its reads.
    pub fn retry<M: Meter>(
        &mut self,
        id: JobId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_job(id)?;
        if matches!(
            self.jobs[id.index].state,
            JobState::Running | JobState::Suspended
        ) {
            return Err(KernelError::AlreadyRunning);
        }
        reserve(meter, self.worklist.enqueue_work())?;
        self.jobs[id.index].state = JobState::Idle;
        self.worklist.enqueue(id);
        self.touch();
        Ok(())
    }
    /// Admit the oldest live FIFO obligation, retaining it as in-flight work.
    pub fn next<M: Meter>(&mut self, meter: &mut M) -> Result<Option<JobId>, KernelError<M::Stop>> {
        if self.worklist.is_empty() {
            return Ok(None);
        }
        let jobs = &self.jobs;
        let id = self.worklist.pop_eligible(meter, |id| {
            matches!(jobs[id.index].state, JobState::Idle | JobState::Interrupted)
        })?;
        if let Some(id) = id {
            self.jobs[id.index].state = JobState::Admitted;
            self.touch();
        }
        Ok(id)
    }
    /// Metered atomic deepest selection with FIFO ties and retained stale records.
    pub fn next_deepest<M: Meter>(
        &mut self,
        meter: &mut M,
        depth: impl Fn(&J) -> usize,
    ) -> Result<Option<JobId>, KernelError<M::Stop>> {
        let jobs = &self.jobs;
        let id = self.worklist.pop_deepest(meter, |id| {
            if matches!(jobs[id.index].state, JobState::Idle | JobState::Interrupted) {
                depth(&jobs[id.index].task)
            } else {
                0
            }
        })?;
        if let Some(id) = id {
            self.jobs[id.index].state = JobState::Admitted;
            self.touch();
        }
        Ok(id)
    }
    /// Start an admitted or explicitly selected job.
    pub fn begin<'a, M: Meter>(
        &'a mut self,
        id: JobId,
        meter: &'a mut M,
    ) -> Result<Run<'a, J, M>, KernelError<M::Stop>> {
        self.check_job(id)?;
        if matches!(
            self.jobs[id.index].state,
            JobState::Running | JobState::Suspended
        ) {
            return Err(KernelError::AlreadyRunning);
        }
        let evaluation = self.jobs[id.index]
            .evaluation
            .next()
            .ok_or(KernelError::VersionExhausted)?;
        reserve(meter, 1)?;
        // next() already removed the admitted occurrence. A later publication
        // may have queued a *new* obligation while the task awaited begin().
        if self.jobs[id.index].state != JobState::Admitted {
            self.worklist.satisfy(id);
        }
        self.jobs[id.index].state = JobState::Running;
        self.jobs[id.index].evaluation = evaluation;
        self.jobs[id.index].evaluation_birth = self.allocation;
        self.stats.evaluations = self.stats.evaluations.saturating_add(1);
        self.touch();
        Ok(Run {
            kernel: self,
            meter,
            job: id,
            reduction: false,
            finished: false,
        })
    }
    /// Physical queue metadata for adapter-owned replay-copy/accounting.
    pub fn worklist(&self) -> &Worklist<JobId> {
        &self.worklist
    }

    pub(crate) fn intern<Q: Capability, P: PublicationPolicy<Value = Q::Value>, M: Meter>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: &Scoped<Q::Key>,
        meter: &mut M,
    ) -> Result<FactId, KernelError<M::Stop>> {
        if channel.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        self.check_fact(channel.directory)?;
        self.check_scope(key.scope)?;
        reserve(
            meter,
            tree_work(channel.cells.len())
                .saturating_mul(Q::key_work(&key.key).max(1))
                .saturating_mul(3),
        )?;
        if let Some(cell) = channel.cells.get(key) {
            return Ok(cell.id);
        }
        reserve(
            meter,
            5usize
                .saturating_add(tree_work(channel.cells.len()))
                .saturating_add(Q::key_work(&key.key)),
        )?;
        let id = self.push_fact(Some(key.scope), false);
        channel.cells.insert(
            key.clone(),
            Cell {
                id,
                state: Availability::Pending,
            },
        );
        Ok(id)
    }
    /// Register a provider for a requested slot, including after its first read.
    ///
    /// Several explicitly bound providers aggregate through this route's policy;
    /// separate channels plus a typed aggregation analyzer are also supported.
    pub fn provide<Q: Capability, P: PublicationPolicy<Value = Q::Value>, M: Meter>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: Scoped<Q::Key>,
        provider: JobId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_job(provider)?;
        let id = self.intern(channel, &key, meter)?;
        reserve(meter, tree_work(self.facts[id.index].providers.len()))?;
        if self.facts[id.index].providers.contains(&provider) {
            return Ok(());
        }
        reserve(
            meter,
            tree_work(self.facts[id.index].providers.len())
                .saturating_add(self.worklist.enqueue_work())
                .saturating_add(1),
        )?;
        self.facts[id.index].providers.insert(provider);
        self.worklist.enqueue(provider);
        self.touch();
        Ok(())
    }
    fn observe<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_job(job)?;
        self.check_fact(id)?;
        // Reserve lookup + both possible insertions before looking up the edge.
        let cost = tree_work(self.jobs[job.index].reads.len())
            .saturating_mul(2)
            .saturating_add(tree_work(self.facts[id.index].readers.len()))
            .saturating_add(2);
        reserve(meter, cost)?;
        let new = !self.jobs[job.index].reads.contains_key(&id);
        self.jobs[job.index]
            .reads
            .insert(id, self.facts[id.index].version);
        if new {
            self.facts[id.index].readers.insert(job);
        }
        self.touch();
        Ok(())
    }
    fn observe_scoped<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        scope: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_scope(scope)?;
        self.observe(job, self.scopes[scope.index].head, meter)?;
        self.observe(job, id, meter)
    }
    fn prepare_changes<M: Meter>(
        &self,
        ids: &[FactId],
        meter: &mut M,
        extra: usize,
    ) -> Result<(), KernelError<M::Stop>> {
        let mut cost = extra;
        // Include all possible wakes before installing any value/version. Repeated
        // readers across two facts are deliberately over-reserved, never omitted.
        let total_wakes = ids.iter().fold(0usize, |sum, id| {
            sum.saturating_add(self.facts.get(id.index).map_or(0, |f| f.readers.len()))
        });
        let enqueue = tree_work(self.worklist.len().saturating_add(total_wakes))
            .saturating_add(1)
            .saturating_add(tree_work(self.jobs.len()))
            .saturating_add(2);
        for id in ids {
            self.check_fact(*id)?;
            if self.facts[id.index].version.next().is_none() {
                return Err(KernelError::VersionExhausted);
            }
            cost = cost
                .saturating_add(self.facts[id.index].readers.len().saturating_mul(enqueue))
                .saturating_add(1);
        }
        reserve(meter, cost)
    }
    fn install_changes(&mut self, ids: &[FactId]) {
        for id in ids {
            let fact = &mut self.facts[id.index];
            fact.version = fact.version.next().expect("preflight checked versions");
            fact.ready = true;
            for reader in &fact.readers {
                self.stats.wake_attempts = self.stats.wake_attempts.saturating_add(1);
                let job = &mut self.jobs[reader.index];
                if job.state == JobState::Retired || !self.scopes[job.scope.index].active {
                    continue;
                }
                self.worklist.enqueue(*reader);
            }
        }
        self.touch();
    }
    /// Open an adapter-owned semantic frontier independently of queue contents.
    pub fn obligation<M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<ObligationId, KernelError<M::Stop>> {
        self.check_scope(scope)?;
        reserve(meter, 2)?;
        let id = ObligationId {
            epoch: self.epoch,
            birth: self.allocation,
            index: self.obligations.len(),
        };
        self.obligations.push(Obligation {
            id,
            scope,
            resolved: false,
        });
        self.touch();
        Ok(id)
    }
    /// Resolve a frontier only after the adapter has represented its behavior.
    pub fn resolve_obligation<M: Meter>(
        &mut self,
        id: ObligationId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        if id.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        let obligation = self
            .obligations
            .get(id.index)
            .ok_or(KernelError::InvalidHandle)?;
        if obligation.id != id {
            return Err(KernelError::InvalidHandle);
        }
        self.check_scope(obligation.scope)?;
        reserve(meter, 1)?;
        self.obligations[id.index].resolved = true;
        self.touch();
        Ok(())
    }
    /// Inspect closure of active generations, never just queue emptiness.
    /// This read-only traversal allocates nothing; adapters account report work.
    pub fn closure(&self) -> Closure {
        self.inspect_closure(None)
    }
    /// Closure of one relative generation, including that generation's demands
    /// on facts in other scopes. An outer suspended job is counted globally.
    pub fn closure_in<S>(&self, scope: Scope) -> Result<Closure, KernelError<S>> {
        self.check_scope(scope)?;
        Ok(self.inspect_closure(Some(scope)))
    }
    fn inspect_closure(&self, scope: Option<Scope>) -> Closure {
        let selected = |job: &Job<J>| scope.is_none_or(|scope| job.scope == scope);
        let mut open = Open {
            pending: self
                .worklist
                .pending()
                .filter(|id| selected(&self.jobs[id.index]))
                .count(),
            ..Open::default()
        };
        for job in &self.jobs {
            if !self.scopes[job.scope.index].active || !selected(job) {
                continue;
            }
            match job.state {
                JobState::Admitted | JobState::Running | JobState::Suspended => open.in_flight += 1,
                JobState::Interrupted => open.interrupted += 1,
                _ => {}
            }
        }
        open.missing = self
            .facts
            .iter()
            .filter(|fact| {
                !fact.ready
                    && fact
                        .scope
                        .is_none_or(|scope| self.scopes[scope.index].active)
                    && fact.readers.iter().any(|id| {
                        self.jobs[id.index].state != JobState::Retired
                            && self.scopes[self.jobs[id.index].scope.index].active
                            && selected(&self.jobs[id.index])
                    })
            })
            .count();
        open.unsealed = self
            .scopes
            .iter()
            .filter(|g| g.active && !g.sealed && scope.is_none_or(|s| s == g.scope))
            .count();
        open.obligations = self
            .obligations
            .iter()
            .filter(|o| {
                !o.resolved
                    && self.scopes[o.scope.index].active
                    && scope.is_none_or(|s| s == o.scope)
            })
            .count();
        if open.is_empty() {
            Closure::Closed
        } else {
            Closure::Open(open)
        }
    }
    /// Whether active completed jobs retained a safe optional precision limit.
    pub fn local_limits(&self) -> bool {
        self.jobs
            .iter()
            .any(|job| job.limited && self.scopes[job.scope.index].active)
    }
    /// Number of retained read-dependency edges, including retired tombstones.
    pub fn dependency_count(&self) -> usize {
        self.jobs.iter().map(|job| job.reads.len()).sum()
    }

    /// Graph sizes and propagation counts; this allocation-free inspection is
    /// charged by the adapter as part of reporting, like [`Self::closure`].
    pub fn statistics(&self) -> Statistics {
        Statistics {
            facts: self.facts.len(),
            jobs: self.jobs.len(),
            scopes: self.scopes.len(),
            dependencies: self.dependency_count(),
            ..self.stats
        }
    }
}
