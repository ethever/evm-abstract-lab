//! Stable routing, confirmed family absence and checked reactivation.
use super::{Kernel, Meter, Run, Scope, Selection, SlotMeta, reserve};
use crate::{Answer, Availability, Change, KernelError};

/// Stable current-generation selector owned outside the replaceable generation.
///
/// Read this entry before reading local guarantees. Historical scopes are never
/// joined into it. Clones share the authoritative selector in the same graph;
/// a detached fork has its own staged selection behind that same checked handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationEntry {
    slot: SlotMeta<Scope>,
}

impl<J> Kernel<J> {
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
        self.check_usable_scope(owner)?;
        self.check_usable_scope(initial)?;
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
        self.facts[slot.id.index].selected = Some(Selection::Active(initial));
        self.scopes[initial.index].entry = Some(slot.id);
        self.scopes[owner.index].entry_owner = true;
        self.touch();
        Ok(GenerationEntry { slot })
    }

    fn selection<S>(&self, entry: &GenerationEntry) -> Result<Selection, KernelError<S>> {
        self.check_scope(entry.slot.scope)?;
        self.check_fact(entry.slot.id)?;
        let selection = self.facts[entry.slot.id.index]
            .selected
            .ok_or(KernelError::InvalidHandle)?;
        let scope = selection.last();
        let generation = self
            .scopes
            .get(scope.index)
            .filter(|g| g.scope == scope)
            .ok_or(KernelError::InvalidHandle)?;
        match selection {
            Selection::Active(_)
                if generation.active && generation.entry == Some(entry.slot.id) =>
            {
                Ok(selection)
            }
            Selection::Absent { .. } if !generation.active && generation.entry.is_none() => {
                Ok(selection)
            }
            _ => Err(KernelError::InvalidHandle),
        }
    }
    /// Metered selected-input validity. Confirmed absence is resolved and does
    /// not require reseeding. An active stale selection remains replaceable.
    pub fn generation_stale<M: Meter>(
        &self,
        entry: &GenerationEntry,
        meter: &mut M,
    ) -> Result<bool, KernelError<M::Stop>> {
        reserve(meter, 3)?;
        Ok(match self.selection(entry)? {
            Selection::Active(scope) => self.scopes[scope.index].stale,
            Selection::Absent { .. } => false,
        })
    }

    /// Checked metered status for callbacks retaining a scope rather than its
    /// entry. Known retired scopes are obsolete too; foreign/aborted identities
    /// are rejected. Budget and cancellation failures remain typed failures.
    pub fn scope_stale<M: Meter>(
        &self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<bool, KernelError<M::Stop>> {
        reserve(meter, 3)?;
        if scope.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        let generation = self
            .scopes
            .get(scope.index)
            .filter(|g| g.scope == scope)
            .ok_or(KernelError::InactiveScope)?;
        Ok(!generation.active || generation.stale)
    }
}

impl<J, M: Meter> Run<'_, J, M> {
    /// Mandatory routing demand. Absent resolves only this family's current
    /// contribution; it is neither whole-point unreachability nor a must proof.
    pub fn current_generation(
        &mut self,
        entry: &GenerationEntry,
    ) -> Result<Answer<Scope>, KernelError<M::Stop>> {
        self.live()?;
        let selection = self.kernel.selection(entry)?;
        self.kernel
            .observe_scoped(self.job, entry.slot.id, entry.slot.scope, self.meter)?;
        reserve(self.meter, 1)?;
        let state = match selection {
            Selection::Absent { .. } => Availability::Absent,
            Selection::Active(scope) if self.kernel.scopes[scope.index].stale => {
                Availability::Pending
            }
            Selection::Active(scope) => Availability::Value(scope),
        };
        Ok(Answer {
            state,
            version: self.kernel.facts[entry.slot.id.index].version,
            scope: entry.slot.scope,
        })
    }

    /// Publish an independently seeded compatible scope. Active routing refresh
    /// retires its old validity; Absent -> Active adds a possible contribution
    /// and invalidates adopters of the stable entry's empty-contribution premise.
    /// No old narrowed local value may be copied into the fresh input universe.
    pub fn replace_generation(
        &mut self,
        entry: &mut GenerationEntry,
        next: Scope,
    ) -> Result<Change, KernelError<M::Stop>> {
        self.live()?;
        let selection = self.kernel.selection(entry)?;
        let old = selection.last();
        let active = matches!(selection, Selection::Active(_));
        self.kernel.check_usable_scope(next)?;
        if active && old == next {
            return Ok(Change::Unchanged);
        }
        if old == entry.slot.scope
            || (active && old == self.scope())
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
        let changes = if active {
            let head = self.kernel.scopes[old.index].head;
            let work = self.kernel.retirement_work(old, self.meter)?;
            self.kernel.prepare_changes(
                &[entry.slot.id, head],
                &[head],
                None,
                self.meter,
                work.saturating_add(3),
            )?
        } else {
            self.kernel
                .prepare_changes(&[entry.slot.id], &[entry.slot.id], None, self.meter, 3)?
        };
        self.kernel.facts[entry.slot.id.index].selected = Some(Selection::Active(next));
        self.kernel.scopes[next.index].entry = Some(entry.slot.id);
        if active {
            self.kernel.scopes[old.index].entry = None;
            self.kernel.install_retirement(old);
        }
        self.kernel.install_changes(changes);
        if active {
            self.kernel.release_generation(old);
        }
        Ok(Change::Changed)
    }

    /// Confirm that a COMPLETE source evaluation proves no current contribution
    /// for this family. Never call for partial, stopped or cancelled evaluation;
    /// this is no assertion about whole-point reachability or other producers.
    /// Retires payloads atomically while retaining selector/lineage identity.
    /// Withdrawing one's own current scope completes that obsolete Run; no later
    /// publication is permitted, and finish/drop cannot leave a false obligation.
    pub fn withdraw_generation(
        &mut self,
        entry: &mut GenerationEntry,
    ) -> Result<Change, KernelError<M::Stop>> {
        reserve(self.meter, 1)?;
        self.live()?;
        let Selection::Active(old) = self.kernel.selection(entry)? else {
            return Ok(Change::Unchanged);
        };
        let head = self.kernel.scopes[old.index].head;
        let work = self.kernel.retirement_work(old, self.meter)?;
        let changes = self.kernel.prepare_changes(
            &[entry.slot.id, head],
            &[head],
            None,
            self.meter,
            work.saturating_add(3),
        )?;
        let retiring_self = self.scope() == old;
        self.kernel.facts[entry.slot.id.index].selected = Some(Selection::Absent { last: old });
        self.kernel.scopes[old.index].entry = None;
        self.kernel.install_retirement(old);
        self.kernel.install_changes(changes);
        self.kernel.release_generation(old);
        self.finished |= retiring_self;
        Ok(Change::Changed)
    }
}
