//! Joint logical-parameter and simultaneous-edge liveness.
use super::{Kind, Projector};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

impl<P: FnMut(usize) -> bool> Projector<'_, P> {
    pub(super) fn prune(&mut self) -> Option<()> {
        if !(self.pay)(
            self.result
                .program
                .entity_ids()
                .count()
                .saturating_mul(4)
                .saturating_add(self.result.proof.parameters.len())
                .saturating_add(1),
        ) {
            return None;
        }
        let mut live = BTreeSet::new();
        let mut live_results = BTreeSet::new();
        let mut work = VecDeque::new();
        let mut materialized = BTreeMap::new();
        for (block, b) in self.result.program.blocks.iter().enumerate() {
            work.extend(b.condition);
            work.extend(b.target);
            work.extend(&b.exit_values);
            for (slot, statement) in b.statements.iter().enumerate() {
                if !(self.pay)(
                    statement
                        .inputs
                        .len()
                        .saturating_add(statement.outputs.len()),
                ) {
                    return None;
                }
                if self.optional_statements.contains(&statement.operation) {
                    for output in &statement.outputs {
                        materialized.insert(*output, (block, slot));
                    }
                } else {
                    work.extend(&statement.inputs);
                }
            }
        }
        for c in &self.result.program.controls {
            work.push_back(c.condition);
        }
        let mut live_params = BTreeSet::new();
        // Open boundaries retain all logical entry bindings, even if none of the
        // currently represented consumers observes them.
        for (block, b) in self.ir.blocks.iter().enumerate() {
            if b.terminator.is_partial() {
                for (i, param) in b.parameters.iter().enumerate() {
                    live_params.insert(*param);
                    for edge in &self.result.proof.incoming[block] {
                        if !(self.pay)(1) {
                            return None;
                        }
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
            match self.kind(id).clone() {
                Kind::Value(value) => {
                    if !live_results.insert(value) {
                        continue;
                    }
                    if let Some((block, slot)) = self.result.proof.parameters.get(&value)
                        && live_params.insert(value)
                    {
                        if !(self.pay)(self.result.proof.incoming[*block].len()) {
                            return None;
                        }
                        for edge in &self.result.proof.incoming[*block] {
                            work.push_back(self.result.program.transfers[*edge].arguments[*slot]);
                        }
                    }
                    if let Some((block, slot)) = materialized.get(&value) {
                        let inputs = &self.result.program.blocks[*block].statements[*slot].inputs;
                        if !(self.pay)(inputs.len()) {
                            return None;
                        }
                        work.extend(inputs);
                    }
                }
                kind => work.extend(kind.operands()),
            }
        }
        for b in &mut self.result.program.blocks {
            b.parameters.retain(|p| live_params.contains(p));
            for statement in &mut b.statements {
                statement.outputs.retain(|v| live_results.contains(v));
            }
            b.statements.retain(|s| {
                !self.optional_statements.contains(&s.operation) || !s.outputs.is_empty()
            });
        }
        for (raw, high) in self.ir.edges.iter().zip(&mut self.result.program.transfers) {
            if !(self.pay)(high.arguments.len().saturating_add(1)) {
                return None;
            }
            let target = &self.ir.blocks[self.result.proof.blocks[&raw.to]];
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
