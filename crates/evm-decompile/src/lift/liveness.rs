//! Joint logical-parameter and simultaneous-edge liveness.
use super::{EntityId, Kind, Projector};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

impl<P: FnMut(usize) -> bool> Projector<'_, P> {
    pub(super) fn prune(&mut self) -> Option<()> {
        let mut live = BTreeSet::new();
        let mut work = VecDeque::new();
        for b in &self.result.program.blocks {
            work.extend(b.condition);
            work.extend(b.target);
            work.extend(&b.exit_values);
            for statement in &b.statements {
                work.extend(&statement.inputs);
            }
        }
        for c in &self.result.program.controls {
            work.push_back(c.condition);
        }
        let params: BTreeMap<_, _> = self
            .ir
            .blocks
            .iter()
            .flat_map(|b| {
                b.parameters
                    .iter()
                    .enumerate()
                    .map(move |(i, id)| (*id, (b.id, i)))
            })
            .collect();
        let mut predecessors = BTreeMap::<EntityId, Vec<usize>>::new();
        for (i, edge) in self.ir.edges.iter().enumerate() {
            predecessors.entry(edge.to).or_default().push(i);
        }
        let mut live_params = BTreeSet::new();
        // Open boundaries retain all logical entry bindings, even if none of the
        // currently represented consumers observes them.
        for b in &self.ir.blocks {
            if b.terminator.is_partial() {
                for (i, param) in b.parameters.iter().enumerate() {
                    live_params.insert(*param);
                    for edge in predecessors.get(&b.id).into_iter().flatten() {
                        work.push_back(self.result.program.transfers[*edge].arguments[i]);
                    }
                }
            }
        }
        while let Some(id) = work.pop_front() {
            if !(self.pay)(1) {
                return None;
            }
            if !live.insert(id) {
                continue;
            }
            match self.kind(id) {
                Kind::Value(value) => {
                    if let Some((block, slot)) = params.get(value)
                        && live_params.insert(*value)
                    {
                        for edge in predecessors.get(block).into_iter().flatten() {
                            work.push_back(self.result.program.transfers[*edge].arguments[*slot]);
                        }
                    }
                }
                kind => work.extend(kind.operands()),
            }
        }
        let live_results: BTreeSet<_> = self
            .result
            .program
            .expressions
            .iter()
            .filter(|e| live.contains(&e.id))
            .filter_map(|e| match e.kind {
                Kind::Value(value) => Some(value),
                _ => None,
            })
            .collect();
        for b in &mut self.result.program.blocks {
            b.parameters.retain(|p| live_params.contains(p));
            for statement in &mut b.statements {
                statement.outputs.retain(|v| live_results.contains(v));
            }
        }
        let blocks: BTreeMap<_, _> = self.ir.blocks.iter().map(|b| (b.id, b)).collect();
        for (raw, high) in self.ir.edges.iter().zip(&mut self.result.program.transfers) {
            let target = blocks.get(&raw.to)?;
            let mut slot = 0;
            high.arguments.retain(|_| {
                let keep = live_params.contains(&target.parameters[slot]);
                slot += 1;
                keep
            });
        }
        self.result
            .program
            .expressions
            .retain(|e| live.contains(&e.id));
        Some(())
    }
}
