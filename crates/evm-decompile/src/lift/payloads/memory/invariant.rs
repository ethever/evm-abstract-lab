//! Anchored cell invariants across transparent cyclic memory equations.
use super::{Memory, Word};
use crate::{EntityId, FactScope, U256, ValueKind};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
impl<P: FnMut(usize) -> bool> Memory<'_, P> {
    /// Close a cyclic read equation only when every reachable memory node is
    /// transparent for this exact cell and reaches a common external anchor.
    /// Reverse reachability rejects unanchored recursive claims. This is a
    /// cell invariant, not a claim that a control-flow loop terminates.
    pub(super) fn invariant_load(
        &mut self,
        memory: EntityId,
        address: U256,
        depth: usize,
    ) -> Option<Word> {
        if self.ir.fact_scope != FactScope::CompleteLocalCfg {
            return None;
        }
        let mut pending = vec![memory];
        let mut seen = BTreeSet::new();
        let mut dependents = BTreeMap::<EntityId, Vec<EntityId>>::new();
        let mut anchored = BTreeSet::new();
        let mut ready = VecDeque::new();
        let mut common: Option<Word> = None;
        while let Some(version) = pending.pop() {
            if !self.charge(6) {
                return None;
            }
            if !seen.insert(version) {
                continue;
            }
            let mut anchor = None;
            let children = match self.values.get(&version)?.kind.clone() {
                ValueKind::Memory { operation: None } => {
                    anchor = Some(Word::constant(U256::ZERO));
                    Vec::new()
                }
                ValueKind::Memory {
                    operation: Some(id),
                } => {
                    let op = *self.operations.get(&id)?;
                    if op.memory_output != Some(version) {
                        return None;
                    }
                    if op.opcode == 0x52
                        && op.inputs.len() == 2
                        && self.eval(op.inputs[0], depth + 1)?.exact() == Some(address)
                    {
                        anchor = Some(self.eval(op.inputs[1], depth + 1)?);
                        Vec::new()
                    } else if self.writer_disjoint(op, Word::constant(address), 32) {
                        vec![op.memory_input?]
                    } else {
                        return None;
                    }
                }
                ValueKind::MemoryParameter { block } => {
                    if self.blocks.get(&block)?.memory_parameter != version {
                        return None;
                    }
                    if Some(block) == self.ir.entry {
                        return None;
                    }
                    let edges = self.incoming.get(&block)?;
                    if edges.is_empty() {
                        return None;
                    }
                    edges.iter().map(|e| e.memory).collect()
                }
                _ => return None,
            };
            if !self.charge(children.len().saturating_mul(3)) {
                return None;
            }
            if let Some(word) = anchor {
                // Opaque operation identities are iteration scoped. Only an
                // exact constant is a loop invariant without a separate
                // dominance/iteration proof for its definition.
                word.exact()?;
                if common.is_some_and(|previous| !previous.same(word)) {
                    return None;
                }
                common = Some(match common {
                    Some(previous) => Word {
                        low: previous.low.min(word.low),
                        high: previous.high.max(word.high),
                        ..previous
                    },
                    None => word,
                });
                anchored.insert(version);
                ready.push_back(version);
            }
            for child in children {
                dependents.entry(child).or_default().push(version);
                pending.push(child);
            }
        }
        while let Some(version) = ready.pop_front() {
            if let Some(next) = dependents.get(&version) {
                let next = next.clone();
                if !self.charge(next.len().saturating_add(1)) {
                    return None;
                }
                for node in next {
                    if anchored.insert(node) {
                        ready.push_back(node);
                    }
                }
            }
        }
        if anchored.len() != seen.len() {
            return None;
        }
        let word = common?;
        for version in seen {
            self.loads.insert((version, address), Some(word));
        }
        Some(word)
    }
}
