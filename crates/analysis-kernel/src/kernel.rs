//! Shared dependency ownership, metered admission and explicit obligations.

use crate::{
    Capability, Channel, Closure, Epoch, IdentityExhausted, KernelError, Open, Scope, ScopeKey,
    Scoped, Version,
    algebra::PublicationPolicy,
    capability::{Availability, Cell, ScopeCells},
    worklist::Worklist,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

mod assumptions;
mod changes;
mod coverage;
mod dependencies;
mod generation;
mod readiness;
mod reclaim;
mod report;
mod run;
mod snapshot;
pub use generation::GenerationEntry;
pub use run::{Run, RunningJob, SlotMeta};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Required,
    Discovery,
}

/// The stable selector is the sole selection/absence authority. The retired
/// last scope preserves checked context/policy lineage through an empty interval.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Active(Scope),
    Absent { last: Scope },
}
impl Selection {
    fn last(self) -> Scope {
        match self {
            Self::Active(scope) | Self::Absent { last: scope } => scope,
        }
    }
}

#[derive(Clone)]
struct Fact {
    id: FactId,
    scope: Option<Scope>,
    version: Version,
    ready: bool,
    readers: BTreeSet<JobId>,
    // Required subset of the wake readers; discoveries alone never block closure.
    demands: BTreeSet<JobId>,
    providers: BTreeSet<JobId>,
    // Reverse semantic assumptions, separate from ordinary wake dependencies.
    assumptions: BTreeSet<Scope>,
    // One last-covered generation per stable source selector, never per job.
    coverage: BTreeMap<FactId, Scope>,
    // A selector's current scope is lifecycle metadata shared by every handle.
    // Keeping it here also gives a fork its own authoritative staged selection.
    selected: Option<Selection>,
}
impl Fact {
    fn new(id: FactId, scope: Option<Scope>, ready: bool) -> Self {
        Self {
            id,
            scope,
            version: Version::default(),
            ready,
            readers: BTreeSet::new(),
            demands: BTreeSet::new(),
            providers: BTreeSet::new(),
            assumptions: BTreeSet::new(),
            coverage: BTreeMap::new(),
            selected: None,
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
    // Retired identities remain checked tombstones without owning task data.
    task: Option<Arc<J>>,
    reads: BTreeMap<FactId, Version>,
    required: BTreeSet<FactId>,
    provides: BTreeSet<FactId>,
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
    facts: BTreeSet<FactId>,
    assumptions: BTreeSet<FactId>,
    open_obligations: BTreeSet<ObligationId>,
    entry: Option<FactId>,
    entry_owner: bool,
    // Only selected generations acquire this automatic invalidation contract.
    stale: bool,
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
    meter.reserve(cost).map_err(KernelError::Stopped)?;
    meter.checkpoint().map_err(KernelError::Stopped)
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
        self.check_usable_scope(job.scope)
    }
    fn check_usable_scope<S>(&self, scope: Scope) -> Result<(), KernelError<S>> {
        self.check_scope(scope)?;
        if self.scopes[scope.index].stale {
            return Err(KernelError::StaleGeneration);
        }
        Ok(())
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
        if let Some(scope) = scope {
            self.scopes[scope.index].facts.insert(id);
        }
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
            facts: [head].into_iter().collect(),
            assumptions: BTreeSet::new(),
            open_obligations: BTreeSet::new(),
            entry: None,
            entry_owner: false,
            stale: false,
        });
        self.facts[head.index].scope = Some(scope);
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
    /// A selected generation must instead be retired by `replace_generation`;
    /// stable entry owners live until the kernel is dropped.
    pub fn retire_scope<M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_scope(scope)?;
        if self.scopes[scope.index].entry.is_some() || self.scopes[scope.index].entry_owner {
            return Err(KernelError::GenerationInUse);
        }
        let head = self.scopes[scope.index].head;
        let work = self.retirement_work(scope, meter)?;
        let changes = self.prepare_changes(&[head], &[head], None, meter, work)?;
        self.install_retirement(scope);
        self.install_changes(changes);
        self.release_generation(scope);
        Ok(())
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
            scopes: BTreeMap::new(),
            len: 0,
            policy,
        })
    }
    /// Register and queue one independently implemented job.
    ///
    /// Task data is moved, not deeply cloned. Its construction belongs to the
    /// adapter's ledger, including eventual deep destruction (also on refusal,
    /// retirement or abandoned forks). This call pays handle/graph operations;
    /// the kernel cannot infer the destruction cost of an opaque `J`.
    pub fn register<M: Meter>(
        &mut self,
        scope: Scope,
        task: J,
        meter: &mut M,
    ) -> Result<JobId, KernelError<M::Stop>> {
        self.check_usable_scope(scope)?;
        reserve(
            meter,
            7usize
                .saturating_add(tree_work(self.scopes[scope.index].jobs.len()))
                .saturating_add(self.worklist.enqueue_work()),
        )?;
        let id = JobId {
            epoch: self.epoch,
            birth: self.allocation,
            index: self.jobs.len(),
        };
        self.jobs.push(Job {
            id,
            scope,
            task: Some(Arc::new(task)),
            reads: BTreeMap::new(),
            required: BTreeSet::new(),
            provides: BTreeSet::new(),
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
        self.jobs[id.index]
            .task
            .as_deref()
            .ok_or(KernelError::InvalidJob)
    }
    /// Retain a constant-size task handle for evaluation while metadata mutates.
    pub fn task_handle<M: Meter>(
        &self,
        id: JobId,
        meter: &mut M,
    ) -> Result<Arc<J>, KernelError<M::Stop>> {
        self.check_job(id)?;
        reserve(meter, 1)?;
        let task = self.jobs[id.index]
            .task
            .as_ref()
            .ok_or(KernelError::InvalidJob)?;
        Ok(Arc::clone(task))
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
        let scopes = &self.scopes;
        let id = self.worklist.pop_eligible(meter, |id| {
            matches!(jobs[id.index].state, JobState::Idle | JobState::Interrupted)
                && !scopes[jobs[id.index].scope.index].stale
        })?;
        if let Some(id) = id {
            self.jobs[id.index].state = JobState::Admitted;
            self.touch();
        }
        Ok(id)
    }
    /// Metered atomic deepest selection over live obligations, with FIFO ties.
    pub fn next_deepest<M: Meter>(
        &mut self,
        meter: &mut M,
        depth: impl Fn(&J) -> usize,
    ) -> Result<Option<JobId>, KernelError<M::Stop>> {
        let jobs = &self.jobs;
        let scopes = &self.scopes;
        let id = self.worklist.pop_deepest(meter, |id| {
            if matches!(jobs[id.index].state, JobState::Idle | JobState::Interrupted)
                && !scopes[jobs[id.index].scope.index].stale
            {
                depth(jobs[id.index].task.as_deref().expect("live job task"))
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
        let work = if self.jobs[id.index].state == JobState::Admitted {
            1
        } else {
            self.worklist.removal_work().saturating_add(1)
        };
        reserve(meter, work)?;
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
    /// Live queue metadata for adapter-owned replay-copy/accounting.
    pub fn worklist(&self) -> &Worklist<JobId> {
        &self.worklist
    }

    fn check_channel<Q: Capability, P: PublicationPolicy<Value = Q::Value>, S>(
        &self,
        channel: &Channel<Q, P>,
    ) -> Result<(), KernelError<S>> {
        if channel.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        self.check_fact(channel.directory)
    }

    fn channel_scope<Q: Capability, P: PublicationPolicy<Value = Q::Value>, M: Meter>(
        &mut self,
        channel: &mut Channel<Q, P>,
        scope: Scope,
        meter: &mut M,
    ) -> Result<FactId, KernelError<M::Stop>> {
        self.check_channel(channel)?;
        self.check_usable_scope(scope)?;
        reserve(meter, tree_work(channel.scopes.len()))?;
        if let Some(table) = channel.scopes.get(&scope) {
            return Ok(table.directory);
        }
        reserve(
            meter,
            5usize
                .saturating_add(tree_work(channel.scopes.len()))
                .saturating_add(tree_work(self.scopes[scope.index].facts.len())),
        )?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        let directory = self.push_fact(Some(scope), true);
        channel.scopes.insert(
            scope,
            ScopeCells {
                directory,
                cells: BTreeMap::new(),
            },
        );
        Ok(directory)
    }

    pub(crate) fn intern<Q: Capability, P: PublicationPolicy<Value = Q::Value>, M: Meter>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: &Scoped<Q::Key>,
        meter: &mut M,
    ) -> Result<FactId, KernelError<M::Stop>> {
        self.channel_scope(channel, key.scope, meter)?;
        let table = channel
            .scopes
            .get(&key.scope)
            .expect("interned scope table");
        reserve(
            meter,
            tree_work(table.cells.len())
                .saturating_mul(Q::key_work(&key.key).max(1))
                .saturating_mul(3),
        )?;
        if let Some(cell) = table.cells.get(&key.key) {
            return Ok(cell.id);
        }
        reserve(
            meter,
            5usize
                .saturating_add(tree_work(table.cells.len()))
                .saturating_add(Q::key_work(&key.key))
                .saturating_add(tree_work(self.scopes[key.scope.index].facts.len())),
        )?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        let id = self.push_fact(Some(key.scope), false);
        channel
            .scopes
            .get_mut(&key.scope)
            .expect("interned scope table")
            .cells
            .insert(
                key.key.clone(),
                Cell {
                    id,
                    state: Availability::Pending,
                },
            );
        channel.len += 1;
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
                .saturating_add(tree_work(self.jobs[provider.index].provides.len()))
                .saturating_add(1),
        )?;
        self.facts[id.index].providers.insert(provider);
        self.jobs[provider.index].provides.insert(id);
        self.worklist.enqueue(provider);
        self.touch();
        Ok(())
    }
    /// Open an adapter-owned semantic frontier independently of queue contents.
    pub fn obligation<M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<ObligationId, KernelError<M::Stop>> {
        self.check_usable_scope(scope)?;
        reserve(
            meter,
            3usize.saturating_add(tree_work(self.scopes[scope.index].open_obligations.len())),
        )?;
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
        self.scopes[scope.index].open_obligations.insert(id);
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
        let scope = obligation.scope;
        self.check_scope(scope)?;
        reserve(
            meter,
            tree_work(self.scopes[scope.index].open_obligations.len()).saturating_add(1),
        )?;
        self.obligations[id.index].resolved = true;
        self.scopes[scope.index].open_obligations.remove(&id);
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
            if !self.scopes[job.scope.index].active
                || self.scopes[job.scope.index].stale
                || !selected(job)
            {
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
                !self.fact_ready(fact)
                    && fact
                        .scope
                        .is_none_or(|scope| self.scopes[scope.index].active)
                    && fact.demands.iter().any(|id| {
                        self.jobs[id.index].state != JobState::Retired
                            && self.scopes[self.jobs[id.index].scope.index].active
                            && !self.scopes[self.jobs[id.index].scope.index].stale
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
            .scopes
            .iter()
            .filter(|g| g.active && g.stale && scope.is_none_or(|s| s == g.scope))
            .count();
        open.obligations += self
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
