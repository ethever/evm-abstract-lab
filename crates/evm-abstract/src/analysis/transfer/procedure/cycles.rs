//! Metered feedback detection for local loops and recursive procedure inputs.
//!
//! A private bytecode function can have many acyclic callers. Those joins must
//! retain finite return addresses; only proven feedback needs acceleration.

use super::Composition;
use crate::analysis::engine::{
    ContextPolicy, PartitionPolicy, PolicyContext, PolicySite, StatePolicy,
};
use crate::domain::EvmNumericAlgebra;

use super::{BTreeMap, BTreeSet, EvmSolver, MachineError, admission};

#[cfg(test)]
mod tests;

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    pub(super) fn node_edge(&mut self, from: usize, to: usize) -> Result<(), MachineError> {
        if self.table.nodes[from].procedure != self.table.nodes[to].procedure {
            return Err(MachineError::Shape);
        }
        if self.table.nodes[from].successors.contains(&to) {
            return Ok(());
        }
        self.reserve(1)?;
        let cyclic = self.joins_node_cycle(to, from)?;
        if cyclic {
            self.reserve(self.table.nodes[to].payload.work_size())?;
            let payload = self.table.nodes[to].payload.clone();
            self.admit_node(to, &payload, true, true, None)?;
            let procedure = self.table.nodes[to].procedure;
            self.refresh_procedure_feedback(procedure, false, true)?;
        }
        self.table.nodes[from].successors.insert(to);
        Ok(())
    }

    fn joins_node_cycle(&mut self, target: usize, from: usize) -> Result<bool, MachineError> {
        let mut pending = vec![target];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            MachineError::check_cancelled()?;
            if id == from {
                return Ok(true);
            }
            if !seen.insert(id) {
                continue;
            }
            self.reserve(self.table.nodes[id].successors.len().saturating_add(1))?;
            pending.extend(self.table.nodes[id].successors.iter().copied());
        }
        Ok(false)
    }

    pub(super) fn call_dependency(
        &mut self,
        from: usize,
        callee: usize,
    ) -> Result<(), MachineError> {
        let caller = self.table.nodes[from].procedure;
        if self.table.procedures[caller].dependencies.contains(&callee) {
            return Ok(());
        }
        self.reserve(1)?;
        // Search the existing graph before adding caller -> callee. The reverse
        // graph identifies every procedure on a closing path, not unrelated
        // descendants or all blocks in a recursive function.
        let mut pending = vec![callee];
        let mut seen = BTreeSet::new();
        let mut reverse: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        while let Some(id) = pending.pop() {
            MachineError::check_cancelled()?;
            if !seen.insert(id) {
                continue;
            }
            self.reserve(
                self.table.procedures[id]
                    .dependencies
                    .len()
                    .saturating_add(1)
                    .saturating_mul(3),
            )?;
            for dependency in &self.table.procedures[id].dependencies {
                reverse.entry(*dependency).or_default().push(id);
                pending.push(*dependency);
            }
        }
        if seen.contains(&caller) {
            let mut pending = vec![caller];
            let mut recursive = BTreeSet::new();
            while let Some(id) = pending.pop() {
                MachineError::check_cancelled()?;
                if !recursive.insert(id) {
                    continue;
                }
                let predecessors = reverse.get(&id).map_or(&[][..], Vec::as_slice);
                self.reserve(predecessors.len().saturating_add(1))?;
                pending.extend(predecessors.iter().copied());
            }
            for id in recursive {
                self.refresh_procedure_feedback(id, true, false)?;
            }
        }
        self.table.procedures[caller].dependencies.insert(callee);
        Ok(())
    }

    pub(super) fn node_feedback(&self, id: usize) -> bool {
        let node = &self.table.nodes[id];
        let procedure = &self.table.procedures[node.procedure];
        node.loop_head || (procedure.entries.contains(&id) && procedure.recursive)
    }

    /// Feedback is a semantic scheduling input. Already settled consumers must
    /// observe its publication even if self-admission leaves their values equal.
    /// All work uses existing Body/Resume/output slots in the shared kernel.
    fn refresh_procedure_feedback(
        &mut self,
        procedure: usize,
        recursive: bool,
        local_cycle: bool,
    ) -> Result<(), MachineError> {
        self.feedback_outputs(procedure, recursive, local_cycle)?;
        self.refresh_entry(procedure)?;
        self.reserve(self.table.procedures[procedure].entries.len())?;
        let entries = self.table.procedures[procedure]
            .entries
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for id in entries {
            let feedback = self.node_feedback(id);
            if feedback != self.table.nodes[id].feedback {
                self.reserve(self.table.nodes[id].payload.work_size())?;
                let payload = self.table.nodes[id].payload.clone();
                self.admit_node(id, &payload, feedback, false, None)?;
            }
        }
        self.reserve(self.table.subscribers.len())?;
        for id in 0..self.table.subscribers.len() {
            let subscriber = &self.table.subscribers[id];
            let feedback = self.node_feedback(subscriber.from)
                || self.table.procedures[subscriber.callee].recursive;
            if feedback != subscriber.feedback {
                self.reserve(subscriber.payload.work_size())?;
                let payload = self.table.subscribers[id].payload.clone();
                self.admit_subscriber(id, &payload)?;
            }
        }
        Ok(())
    }

    /// Stage every outcome separately, including empty relations. Generations
    /// advance on schedule changes so delivery receipts cannot hide a wakeup.
    /// A failed admission/publication leaves each retained output and its flags
    /// intact. Earlier successful slot publications remain sound on interruption.
    fn feedback_outputs(
        &mut self,
        procedure: usize,
        recursive: bool,
        local_cycle: bool,
    ) -> Result<(), MachineError> {
        let old = &self.table.procedures[procedure];
        let recursive = recursive || old.recursive;
        let local_cycle = local_cycle || old.local_cycle;
        if recursive == old.recursive && local_cycle == old.local_cycle {
            return Ok(());
        }
        let mut outputs = BTreeMap::new();
        let mut install_work = 1usize;
        for (kind, output) in &old.outputs {
            let acceleration =
                self.acceleration(PolicySite::Output, recursive || local_cycle, output.updates);
            let prepared = admission::prepare_output(
                &self.policies.state,
                Some(output),
                &output.summary,
                acceleration,
                true,
                &mut PolicyContext::new(self.domain, self.entry, self.budget),
            )?
            .expect("feedback invalidates even an unchanged output");
            install_work = install_work
                .saturating_add(prepared.work)
                .saturating_add(output.work);
            outputs.insert(*kind, prepared);
        }
        self.publish_outputs(procedure, outputs, install_work, recursive, local_cycle)
    }
}
