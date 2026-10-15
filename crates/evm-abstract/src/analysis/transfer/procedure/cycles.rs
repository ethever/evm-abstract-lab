//! Metered feedback detection for local loops and recursive procedure inputs.
//!
//! A private bytecode function can have many acyclic callers. Those joins must
//! retain finite return addresses; only proven feedback needs acceleration.

use super::{BTreeMap, BTreeSet, MachineError, Solver};

#[cfg(test)]
mod tests;

impl Solver<'_> {
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
            self.table.nodes[to].loop_head = true;
            let procedure = self.table.nodes[to].procedure;
            self.table.procedures[procedure].local_cycle = true;
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
                self.table.procedures[id].recursive = true;
            }
        }
        self.table.procedures[caller].dependencies.insert(callee);
        Ok(())
    }

    pub(super) fn node_feedback(&self, id: usize) -> bool {
        let node = &self.table.nodes[id];
        let procedure = &self.table.procedures[node.procedure];
        node.loop_head || (id == procedure.entry && procedure.recursive)
    }
}
