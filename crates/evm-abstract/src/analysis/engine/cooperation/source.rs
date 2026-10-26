//! Positive currency requires the exact foreign universe consumed by an owner.
//! Native versions alone miss retirement or growth of adopted foreign premises.

use super::{Composition, Cooperation};
use crate::{
    analysis::{
        MachineError,
        cooperation::{CooperationError, InputTasks, PointKind, ProcessFacts, Subject},
        worklist::{
            MachineMeter,
            runtime::{Runtime, TaskKind},
        },
    },
    domain::WordElement,
    resource::WorkBudget,
};
use analysis_kernel::{Availability, Closure, JobId, KernelError, Meter, RunningJob, Scoped};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Owner {
    World(usize),
    Body(usize),
    Resume(usize),
}
impl Owner {
    pub(super) fn of<T>(task: &TaskKind<T>) -> Option<Self> {
        match task {
            TaskKind::World { node, .. } => Some(Self::World(*node)),
            TaskKind::Body(id) => Some(Self::Body(*id)),
            TaskKind::Resume(id) => Some(Self::Resume(*id)),
            _ => None,
        }
    }
}

pub(super) struct Source {
    job: JobId,
    epoch: usize,
    consumed: Option<Subject>,
}

impl<N: WordElement, X: Composition<N, Error = E>, E> Cooperation<N, X, E> {
    pub(super) fn source_slot(
        &mut self,
        owner: Owner,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<analysis_kernel::SlotMeta<usize>, CooperationError<E>> {
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 8)
            .map_err(KernelError::Stopped)?;
        if let Some(slot) = self.source_slots.get(&owner) {
            return Ok(slot.clone());
        }
        let scope = match owner {
            Owner::World(_) => runtime.world,
            Owner::Body(_) | Owner::Resume(_) => runtime.relative,
        };
        let slot = runtime.kernel().slot(scope, &mut MachineMeter(budget))?;
        self.source_slots.insert(owner, slot.clone());
        Ok(slot)
    }

    /// Observe actual regional owner evaluations on the suspended World job.
    /// An equal-numeric reproof still changes its source basis. Keeping these
    /// clocks separate from numerical slots preserves row/cache identity while
    /// waking World to retain fresh evidence after a Body/Resume retry.
    pub(in crate::analysis) fn observe_regional_sources(
        &mut self,
        cfg: &crate::analysis::transfer::procedure::EvmRegionalCfg,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        caller: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        MachineMeter(budget)
            .reserve(
                cfg.nodes
                    .len()
                    .saturating_add(cfg.calls.len())
                    .saturating_mul(4)
                    .saturating_add(8),
            )
            .map_err(KernelError::Stopped)?;
        for owner in cfg
            .nodes
            .iter()
            .map(|node| Owner::Body(node.id))
            .chain(cfg.calls.iter().map(|call| Owner::Resume(call.id)))
        {
            let slot = self.source_slot(owner, runtime, budget)?;
            let mut meter = MachineMeter(budget);
            let mut run = runtime.kernel().resume(caller, &mut meter)?;
            run.observe_slot(&slot)?;
            *caller = run.suspend();
        }
        Ok(())
    }

    /// Capture only after complete transfer/admission and source stamp checks.
    /// Later guard checks include selector identity, staleness, client closure,
    /// current owner epoch and pending native work. No numerical equality test
    /// can restore an old guarantee after its universe changes.
    pub(in crate::analysis) fn record_source(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        token: &mut RunningJob,
        budget: &mut WorkBudget,
        consumed: Option<Subject>,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 16)
            .map_err(KernelError::Stopped)?;
        let job = self.consumer_job(runtime, token, budget)?;
        let owner = Owner::of(&runtime.view().task(job)?.0).ok_or(KernelError::InvalidJob)?;
        let epoch = *self.owner_epochs.get(&job).ok_or(KernelError::InvalidJob)?;
        self.sources.insert(
            owner,
            Source {
                job,
                epoch,
                consumed,
            },
        );
        Ok(())
    }

    pub(in crate::analysis) fn source_current(
        &self,
        task: &TaskKind<<X as InputTasks>::Task>,
        runtime: &Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CooperationError<E>> {
        if !X::ENABLED {
            return Ok(true);
        }
        let meter = &mut SourceMeter(budget);
        meter
            .reserve(((usize::BITS as usize) + 1) * 16)
            .map_err(KernelError::Stopped)?;
        let Some(source) = Owner::of(task).and_then(|owner| self.sources.get(&owner)) else {
            return Ok(false);
        };
        if self.owner_epochs.get(&source.job) != Some(&source.epoch)
            || runtime.view().worklist().contains(&source.job)
            || self.deferred.contains_key(&source.job)
        {
            return Ok(false);
        }
        if let Some(consumed) = source.consumed
            && !self.receipt_valid_metered(consumed, runtime, meter)?
        {
            return Ok(false);
        }
        let points = self.owner_points.get(&source.job);
        meter
            .reserve(
                points
                    .map_or(0, |p| p.len())
                    .saturating_mul(((usize::BITS as usize) + 1) * 16),
            )
            .map_err(KernelError::Stopped)?;
        for point in points.into_iter().flatten() {
            let Some(binding) = self.bindings.get(point) else {
                return Ok(false);
            };
            if binding.consumer != source.job
                || runtime.view().generation_stale(&binding.entry, meter)?
                || runtime
                    .view()
                    .scope_stale(binding.input.subject().scope, meter)?
                || runtime
                    .view()
                    .closure_in_metered(binding.input.subject().scope, meter)?
                    != Closure::Closed
                || (point.namespace == 2 && binding.producer != Some((source.job, source.epoch)))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A World observation contains regional work performed by other owners.
    /// Its own input receipt does not inherit those owners' selected scopes:
    /// native World/relative scopes are persistent, not replaceable generations.
    /// Check the actual retained model's Body/Resume basis before certifying it.
    pub(in crate::analysis) fn regional_sources_current(
        &self,
        cfg: &crate::analysis::transfer::procedure::EvmRegionalCfg,
        runtime: &Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CooperationError<E>> {
        if !X::ENABLED {
            return Ok(true);
        }
        SourceMeter(budget)
            .reserve(
                cfg.nodes
                    .len()
                    .saturating_add(cfg.calls.len())
                    .saturating_mul(4)
                    .saturating_add(8),
            )
            .map_err(KernelError::Stopped)?;
        for node in &cfg.nodes {
            if !self.source_current(&TaskKind::Body(node.id), runtime, budget)? {
                return Ok(false);
            }
        }
        for call in &cfg.calls {
            if !self.source_current(&TaskKind::Resume(call.id), runtime, budget)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Tracked feedback read on the actual Body owner; consumers never patch
    /// authoritative input or Store. input() has already waited for readiness.
    pub(in crate::analysis) fn process_facts(
        &mut self,
        subject: Subject,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        token: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<ProcessFacts, CooperationError<E>> {
        if subject.point.namespace != 1 || subject.point.kind != PointKind::Body {
            return Err(crate::analysis::cooperation::RefinementError::ProcessScope.into());
        }
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(token, &mut meter)?;
        let facts = run.query(
            &mut self
                .shared
                .as_mut()
                .ok_or(KernelError::InvalidHandle)?
                .process_refinements,
            Scoped::new(subject.scope, ()),
        )?;
        *token = run.suspend();
        match facts.state {
            Availability::Value(facts) => Ok(facts),
            _ => Err(CooperationError::Protocol(
                KernelError::<MachineError>::InvalidHandle,
            )),
        }
    }
}

/// Currency is a paid metadata observation. A refused optional World certificate
/// withholds its flag without poisoning the already-completed semantic ledger.
/// This meter spends the original budget; it creates no allowance or rescue.
struct SourceMeter<'a>(&'a mut WorkBudget);
impl Meter for SourceMeter<'_> {
    type Stop = MachineError;
    fn reserve(&mut self, work: usize) -> Result<(), MachineError> {
        self.checkpoint()?;
        if work == usize::MAX || !self.0.can_afford(work) {
            return Err(MachineError::work_failure());
        }
        if !self.0.charge(work) {
            return Err(MachineError::work_failure());
        }
        self.checkpoint()
    }
    fn checkpoint(&self) -> Result<(), MachineError> {
        crate::resource::clone_payment::checkpoint(self.0).map_err(Into::into)
    }
}
