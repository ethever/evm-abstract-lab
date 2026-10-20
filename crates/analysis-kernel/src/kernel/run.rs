//! One retained evaluation and per-publication atomic semantic installation.

use super::{FactId, JobId, JobState, Kernel, Meter, reserve};
use crate::{
    AdmissionPolicy, Answer, Availability, Capability, Change, Channel, KernelError, LocalClosure,
    PublishError, Scope, Scoped, Version, algebra::PublicationPolicy,
};
use std::marker::PhantomData;

/// Metadata for an externally owned initialized semantic value.
///
/// Keep one authoritative value beside this metadata. Borrowed sidecar access
/// allows existing public result fields to retain their concrete representation;
/// all semantic updates must use [`Run::publish_prepared`]. Copying metadata is
/// only for detached snapshots: each active graph has one authoritative value.
pub struct SlotMeta<V> {
    id: FactId,
    scope: Scope,
    marker: PhantomData<fn() -> V>,
}
impl<V> Clone for SlotMeta<V> {
    /// Copy a constant-size handle into a paid detached snapshot, not its value.
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            scope: self.scope,
            marker: PhantomData,
        }
    }
}
impl<V> SlotMeta<V> {
    /// Immutable input/context identity of this external slot.
    pub fn scope(&self) -> Scope {
        self.scope
    }
}

/// A suspended evaluation retaining its in-flight obligation without borrows.
///
/// Dropping this token never certifies completion. Resume it on the same kernel
/// (or its unpublished fork). Tokens cannot be cloned or used for a later begin.
#[derive(Debug)]
pub struct RunningJob {
    job: JobId,
    evaluation: Version,
    evaluation_birth: crate::Epoch,
    reduction: bool,
}

/// Stable current-generation selector owned outside the replaceable generation.
///
/// Read this entry before reading local guarantees. Historical scopes are never
/// joined into it. Clones share the authoritative selector in the same graph;
/// a detached fork has its own staged selection behind that same checked handle.
#[derive(Clone)]
pub struct GenerationEntry {
    slot: SlotMeta<Scope>,
}

impl<J> Kernel<J> {
    /// Reacquire tracked operations without completing the retained evaluation.
    pub fn resume<'a, M: Meter>(
        &'a mut self,
        token: &mut RunningJob,
        meter: &'a mut M,
    ) -> Result<Run<'a, J, M>, KernelError<M::Stop>> {
        self.check_job(token.job)?;
        let job = &self.jobs[token.job.index];
        if job.state != JobState::Suspended
            || job.evaluation != token.evaluation
            || job.evaluation_birth != token.evaluation_birth
        {
            return Err(KernelError::NotRunning);
        }
        reserve(meter, 1)?;
        self.jobs[token.job.index].state = JobState::Running;
        self.touch();
        Ok(Run {
            kernel: self,
            meter,
            job: token.job,
            reduction: token.reduction,
            finished: false,
        })
    }

    /// Initialize a stable entry after paying construction of its first input.
    /// `owner` cannot itself be replaceable and lives until the kernel is dropped.
    /// Each selected scope has one authoritative entry; share that entry with
    /// readers rather than constructing another selector over the same scope.
    pub fn generation_entry<M: Meter>(
        &mut self,
        owner: Scope,
        initial: Scope,
        meter: &mut M,
    ) -> Result<GenerationEntry, KernelError<M::Stop>> {
        self.check_scope(owner)?;
        self.check_scope(initial)?;
        if owner == initial {
            return Err(KernelError::WrongScope);
        }
        if self.scopes[owner.index].entry.is_some()
            || self.scopes[initial.index].entry.is_some()
            || self.scopes[initial.index].entry_owner
        {
            return Err(KernelError::GenerationInUse);
        }
        // Prepay the ownership updates as well as slot allocation. No failure
        // follows slot creation, so a refused constructor cannot claim a scope.
        reserve(meter, 3)?;
        let slot = self.slot(owner, meter)?;
        self.facts[slot.id.index].selected = Some(initial);
        self.scopes[initial.index].entry = Some(slot.id);
        self.scopes[owner.index].entry_owner = true;
        self.touch();
        Ok(GenerationEntry { slot })
    }

    fn selected_generation<S>(&self, entry: &GenerationEntry) -> Result<Scope, KernelError<S>> {
        self.check_scope(entry.slot.scope)?;
        self.check_fact(entry.slot.id)?;
        let current = self.facts[entry.slot.id.index]
            .selected
            .ok_or(KernelError::InvalidHandle)?;
        self.check_scope(current)?;
        if self.scopes[current.index].entry != Some(entry.slot.id) {
            return Err(KernelError::InvalidHandle);
        }
        Ok(current)
    }

    /// Register metadata for an already initialized external semantic value.
    /// The adapter accounts constructing/retaining that value before this call.
    pub fn slot<V, M: Meter>(
        &mut self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<SlotMeta<V>, KernelError<M::Stop>> {
        self.check_scope(scope)?;
        reserve(meter, 3)?;
        let id = self.push_fact(Some(scope), true);
        Ok(SlotMeta {
            id,
            scope,
            marker: PhantomData,
        })
    }
}

/// A live job and shared root meter, with tracked typed read/publication methods.
///
/// Dropping before successful [`Self::finish`] leaves an interrupted obligation.
/// There is no job-wide rollback: earlier atomic sound publications remain valid.
pub struct Run<'a, J, M: Meter> {
    pub(crate) kernel: &'a mut Kernel<J>,
    pub(crate) meter: &'a mut M,
    pub(crate) job: JobId,
    pub(crate) reduction: bool,
    pub(crate) finished: bool,
}
impl<J, M: Meter> Run<'_, J, M> {
    /// Release metadata/ledger borrows while retaining this evaluation in flight.
    /// Only a successful resumed [`Self::finish`] can complete the evaluation.
    pub fn suspend(mut self) -> RunningJob {
        let job = &mut self.kernel.jobs[self.job.index];
        let token = RunningJob {
            job: self.job,
            evaluation: job.evaluation,
            evaluation_birth: job.evaluation_birth,
            reduction: self.reduction,
        };
        if job.state == JobState::Running {
            job.state = JobState::Suspended;
            self.kernel.touch();
        }
        self.finished = true;
        token
    }
    /// Current independently scheduled job identity.
    pub fn job(&self) -> JobId {
        self.job
    }
    /// Current immutable semantic input generation.
    pub fn scope(&self) -> Scope {
        self.kernel.jobs[self.job.index].scope
    }
    /// Shared root meter for adapter operations; it is never replaced or reset.
    pub fn meter(&mut self) -> &mut M {
        self.meter
    }
    /// Register dynamically discovered jobs/providers/obligations in the same core.
    pub fn kernel(&mut self) -> &mut Kernel<J> {
        self.kernel
    }
    /// Split metadata and ledger borrows for nested production adapters.
    /// Nested admission skips this run until it finishes, retaining self-dirty work.
    pub fn parts(&mut self) -> (&mut Kernel<J>, &mut M) {
        (self.kernel, self.meter)
    }
    /// Reserve adapter work using the same checked stop protocol as publications.
    pub fn reserve(&mut self, work: usize) -> Result<(), KernelError<M::Stop>> {
        reserve(self.meter, work)
    }
    fn live(&self) -> Result<(), KernelError<M::Stop>> {
        self.kernel.check_job(self.job)?;
        if self.kernel.jobs[self.job.index].state != JobState::Running {
            return Err(KernelError::NotRunning);
        }
        Ok(())
    }
    fn write_scope<P: PublicationPolicy>(&self, scope: Scope) -> Result<(), KernelError<M::Stop>> {
        self.live()?;
        self.kernel.check_scope(scope)?;
        if P::DESCENDING {
            if !self.reduction {
                return Err(KernelError::UnregisteredReduction);
            }
            if self.scope() != scope {
                return Err(KernelError::WrongScope);
            }
        }
        Ok(())
    }
    /// Query a typed route, tracking pending and explicit negative answers too.
    pub fn query<Q: Capability, P: PublicationPolicy<Value = Q::Value>>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: Scoped<Q::Key>,
    ) -> Result<Answer<Q::Value>, KernelError<M::Stop>> {
        self.live()?;
        let id = self.kernel.intern(channel, &key, self.meter)?;
        self.kernel
            .observe_scoped(self.job, id, key.scope, self.meter)?;
        let cell = channel.cells.get(&key).expect("interned typed cell");
        let clone_work = match &cell.state {
            Availability::Value(value) => Q::value_work(value),
            _ => 1,
        };
        reserve(self.meter, clone_work)?;
        Ok(Answer {
            state: cell.state.clone(),
            version: self.kernel.facts[id.index].version,
            scope: key.scope,
        })
    }
    /// Publish through the explicitly chosen ascending or registered local policy.
    ///
    /// Policy preparation is paid first. Then value, semantic version, first-key
    /// directory change and reader wakes are reserved and installed together.
    /// Interning a previously absent pending slot is a separate atomic operation.
    pub fn publish<Q: Capability, P: PublicationPolicy<Value = Q::Value>>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: Scoped<Q::Key>,
        incoming: Q::Value,
    ) -> Result<Change, PublishError<P::Error, M::Stop>> {
        self.write_scope::<P>(key.scope)?;
        let id = self.kernel.intern(channel, &key, self.meter)?;
        let cell = channel.cells.get(&key).expect("interned typed cell");
        let next = if let Availability::Value(old) = &cell.state {
            reserve(self.meter, channel.policy.work(old, &incoming))?;
            let next = channel
                .policy
                .combine(old, &incoming)
                .map_err(PublishError::Policy)?;
            if channel.policy.equivalent(old, &next) {
                return Ok(Change::Unchanged);
            }
            next
        } else {
            incoming
        };
        let first = matches!(cell.state, Availability::Pending);
        let changes = if first {
            &[id, channel.directory][..]
        } else {
            &[id][..]
        };
        self.kernel
            .prepare_changes(changes, self.meter, Q::value_work(&next).saturating_add(1))?;
        channel
            .cells
            .get_mut(&key)
            .expect("interned typed cell")
            .state = Availability::Value(next);
        self.kernel.install_changes(changes);
        Ok(Change::Changed)
    }
    /// Resolve a pending negative answer; existing values cannot be retracted.
    pub fn resolve_absent<Q: Capability, P: PublicationPolicy<Value = Q::Value>>(
        &mut self,
        channel: &mut Channel<Q, P>,
        key: Scoped<Q::Key>,
    ) -> Result<Change, KernelError<M::Stop>> {
        self.write_scope::<P>(key.scope)?;
        let id = self.kernel.intern(channel, &key, self.meter)?;
        match &channel.cells.get(&key).expect("interned typed cell").state {
            Availability::Value(_) => return Err(KernelError::CannotErase),
            Availability::Absent => return Ok(Change::Unchanged),
            Availability::Pending => {}
        }
        let changes = [id, channel.directory];
        self.kernel.prepare_changes(&changes, self.meter, 1)?;
        channel
            .cells
            .get_mut(&key)
            .expect("interned typed cell")
            .state = Availability::Absent;
        self.kernel.install_changes(&changes);
        Ok(Change::Changed)
    }
    /// Enumerate resolved keys while tracking directory and generation changes.
    /// Pending query interning alone does not create a resolved directory member.
    pub fn keys<Q: Capability, P: PublicationPolicy<Value = Q::Value>>(
        &mut self,
        channel: &mut Channel<Q, P>,
        scope: Scope,
    ) -> Result<Vec<Q::Key>, KernelError<M::Stop>> {
        self.live()?;
        if channel.epoch != self.kernel.epoch {
            return Err(KernelError::WrongEpoch);
        }
        self.kernel
            .observe_scoped(self.job, channel.directory, scope, self.meter)?;
        // Pay the metadata scan before quoting key retention. Collection below
        // scans again and clones keys only after its own full reservation.
        reserve(self.meter, channel.cells.len().saturating_add(1))?;
        let mut cost = channel.cells.len().saturating_add(1);
        for (key, cell) in &channel.cells {
            if key.scope == scope && !matches!(cell.state, Availability::Pending) {
                cost = cost.saturating_add(Q::key_work(&key.key)).saturating_add(1);
            }
        }
        reserve(self.meter, cost)?;
        Ok(channel
            .cells
            .iter()
            .filter(|(key, cell)| {
                key.scope == scope && !matches!(cell.state, Availability::Pending)
            })
            .map(|(key, _)| key.key.clone())
            .collect())
    }
    /// Track and copy an initialized external value after reserving its clone work.
    pub fn read_slot<V: Clone>(
        &mut self,
        slot: &SlotMeta<V>,
        value: &V,
        clone_work: usize,
    ) -> Result<Answer<V>, KernelError<M::Stop>> {
        self.live()?;
        self.kernel
            .observe_scoped(self.job, slot.id, slot.scope, self.meter)?;
        reserve(self.meter, clone_work)?;
        Ok(Answer {
            state: Availability::Value(value.clone()),
            version: self.kernel.facts[slot.id.index].version,
            scope: slot.scope,
        })
    }
    /// Discover the current input generation and depend on its stable selector.
    /// Consumers must reread this entry after a wake before using local facts.
    pub fn current_generation(
        &mut self,
        entry: &GenerationEntry,
    ) -> Result<Answer<Scope>, KernelError<M::Stop>> {
        let current = self.kernel.selected_generation(entry)?;
        self.read_slot(&entry.slot, &current, 1)
    }

    /// Atomically switch a seeded selector and retire the old fixed-input scope.
    ///
    /// The adapter keeps an owner obligation open while constructing/seeding the
    /// replacement and resolves it only after this succeeds. `next` must cover
    /// the old concrete input and be independently seeded by registered rules;
    /// old narrowed values must never be copied into the enlarged input. Pending
    /// jobs/reads in the new generation remain ordinary closure obligations.
    /// A replacement cannot take another entry's scope or a stable owner.
    pub fn replace_generation(
        &mut self,
        entry: &mut GenerationEntry,
        next: Scope,
    ) -> Result<Change, KernelError<M::Stop>> {
        self.live()?;
        let old = self.kernel.selected_generation(entry)?;
        self.kernel.check_scope(next)?;
        if old == next {
            return Ok(Change::Unchanged);
        }
        if old == entry.slot.scope
            || old == self.scope()
            || next == entry.slot.scope
            || old.key.context != next.key.context
            || old.key.policy != next.key.policy
            || old.key.generation == next.key.generation
        {
            return Err(KernelError::WrongScope);
        }
        if self.kernel.scopes[next.index].entry.is_some()
            || self.kernel.scopes[next.index].entry_owner
        {
            return Err(KernelError::GenerationInUse);
        }
        if !self.kernel.scopes[next.index].sealed {
            return Err(KernelError::UnsealedScope);
        }
        let changes = [entry.slot.id, self.kernel.scopes[old.index].head];
        let retire_work = self.kernel.retirement_work(old, self.meter)?;
        self.kernel
            .prepare_changes(&changes, self.meter, retire_work.saturating_add(3))?;
        self.kernel.facts[entry.slot.id.index].selected = Some(next);
        self.kernel.scopes[old.index].entry = None;
        self.kernel.scopes[next.index].entry = Some(entry.slot.id);
        self.kernel.install_retirement(old);
        self.kernel.install_changes(&changes);
        self.kernel.release_generation(old);
        Ok(Change::Changed)
    }

    /// Reserve pure covering preparation, then atomically admit value + evidence.
    /// This is the same publication path used by typed fact channels and adapters.
    pub fn admit<P: AdmissionPolicy>(
        &mut self,
        slot: &SlotMeta<P::Value>,
        current: &mut P::Value,
        metadata: &mut P::Metadata,
        incoming: &P::Value,
        policy: &P,
    ) -> Result<Change, PublishError<P::Error, M::Stop>> {
        self.live()?;
        self.kernel.check_fact(slot.id)?;
        reserve(self.meter, policy.work(current, incoming, metadata))?;
        let prepared = policy
            .prepare(current, incoming, metadata)
            .map_err(PublishError::Policy)?;
        Ok(self.publish_prepared_parts(
            slot,
            current,
            prepared.value,
            metadata,
            prepared.metadata,
            prepared.change,
            prepared.install_work,
        )?)
    }

    /// Reserve all value/version/notification costs, then install several fields.
    ///
    /// Prepare against immutable live inputs first. `install` cannot fail and
    /// must install the prepared value and all corresponding evidence together.
    /// Unchanged may update nonsemantic metadata without advancing the version.
    /// This covers typed failures/cancellation, not panics or allocation failure.
    pub fn publish_prepared_with<V>(
        &mut self,
        slot: &SlotMeta<V>,
        changed: Change,
        install_work: usize,
        install: impl FnOnce(),
    ) -> Result<Change, KernelError<M::Stop>> {
        self.live()?;
        self.kernel.check_scope(slot.scope)?;
        self.kernel.check_fact(slot.id)?;
        if changed == Change::Changed {
            self.kernel
                .prepare_changes(&[slot.id], self.meter, install_work)?;
        } else {
            reserve(self.meter, install_work)?;
        }
        install();
        if changed == Change::Changed {
            self.kernel.install_changes(&[slot.id]);
        } else {
            // A detached adapter's nonsemantic metadata changed, so a previously
            // forked graph must not later overwrite it with an older snapshot.
            self.kernel.touch();
        }
        Ok(changed)
    }

    /// Install a prepared whole value and its detached admission metadata.
    #[expect(
        clippy::too_many_arguments,
        reason = "explicit borrowed state and detached evidence form one atomic publication"
    )]
    pub fn publish_prepared_parts<V, T>(
        &mut self,
        slot: &SlotMeta<V>,
        current: &mut V,
        next: V,
        metadata: &mut T,
        next_metadata: T,
        changed: Change,
        install_work: usize,
    ) -> Result<Change, KernelError<M::Stop>> {
        self.publish_prepared_with(slot, changed, install_work, || {
            *current = next;
            *metadata = next_metadata;
        })
    }
    /// Install an adapter-prepared ascending/accelerated external state atomically.
    ///
    /// `changed` must compare actual semantic bounds, including relevant coverage
    /// evidence. The adapter has already paid construction/comparison and proved
    /// that `next` covers the previous bound and its new input. `install_work`
    /// includes retained-value traversal. An unchanged update preserves the old
    /// representation. This escape hatch never performs a descending refinement.
    pub fn publish_prepared<V>(
        &mut self,
        slot: &SlotMeta<V>,
        current: &mut V,
        next: V,
        changed: Change,
        install_work: usize,
    ) -> Result<Change, KernelError<M::Stop>> {
        self.live()?;
        self.kernel.check_scope(slot.scope)?;
        self.kernel.check_fact(slot.id)?;
        if changed == Change::Unchanged {
            return Ok(changed);
        }
        self.publish_prepared_with(slot, changed, install_work, || *current = next)
    }
    /// Current version of a checked external slot, for certificate guards.
    pub fn slot_version<V>(&self, slot: &SlotMeta<V>) -> Result<Version, KernelError<M::Stop>> {
        self.live()?;
        self.kernel.check_fact(slot.id)?;
        Ok(self.kernel.facts[slot.id.index].version)
    }
    /// Satisfy this evaluation; self-dirty publications retain their queued rerun.
    pub fn finish(mut self, closure: LocalClosure) -> Result<(), KernelError<M::Stop>> {
        self.live()?;
        reserve(self.meter, 1)?;
        let job = &mut self.kernel.jobs[self.job.index];
        job.state = JobState::Idle;
        job.limited |= closure == LocalClosure::Limited;
        self.kernel.touch();
        self.finished = true;
        Ok(())
    }
    pub(crate) fn set_reduction(&mut self, active: bool) -> bool {
        std::mem::replace(&mut self.reduction, active)
    }
}
impl<J, M: Meter> Drop for Run<'_, J, M> {
    fn drop(&mut self) {
        if !self.finished && self.kernel.jobs[self.job.index].state == JobState::Running {
            // No allocation/queue mutation is needed to retain an interruption.
            self.kernel.jobs[self.job.index].state = JobState::Interrupted;
            self.kernel.touch();
        }
    }
}
