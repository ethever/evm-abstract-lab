//! Join exact source contents or retain their common interval at a fixed cell.
//! Content atoms denote one acyclic memory phi or an external loop entry, never
//! a newly merged value across iterations. Missing incoming cells stay unknown.
use super::{Atom, Block, EntityId, Expr, Flow, Range, Source, State};
use std::collections::BTreeMap;
#[cfg(test)]
mod tests;
impl<P: FnMut(usize) -> bool> Source<'_, P> {
    pub(super) fn join(
        &mut self,
        block: &Block,
        flows: &BTreeMap<EntityId, Flow>,
        iteration: Option<EntityId>,
    ) -> Flow {
        if !self.charge(
            self.incoming
                .get(&block.id)
                .map_or(0, Vec::len)
                .saturating_add(1),
        ) {
            return Flow::Unknown;
        }
        let Some(incoming) = self.incoming.get(&block.id).cloned() else {
            return Flow::Unknown;
        };
        let mut joined = None;
        for edge in incoming {
            if !self.charge(1) {
                return Flow::Unknown;
            }
            match flows.get(&edge.id) {
                Some(Flow::Dead) => {}
                Some(Flow::Known(state)) => {
                    if !self.work(state, 1) {
                        return Flow::Unknown;
                    }
                    joined = Some(match joined {
                        None => state.clone(),
                        Some(previous) => {
                            if !self.work(&previous, 1) {
                                return Flow::Unknown;
                            }
                            self.merge(previous, state, iteration)
                        }
                    });
                }
                _ => return Flow::Unknown,
            }
        }
        joined.map_or(Flow::Dead, Flow::Known)
    }
    pub(super) fn merge(&self, mut a: State, b: &State, iteration: Option<EntityId>) -> State {
        let extent_anchors = if a.extent != b.extent {
            let anchors = super::extent::join_anchors(&a, b);
            a.extent = None;
            anchors
        } else {
            Vec::new()
        };
        // These are fresh contents of one reaching memory phi. An iteration's
        // internal join cannot create an invariant identity: retain the prior
        // conservative drop there and let source induction prove recurrences.
        let mut contents = Vec::new();
        if iteration.is_none()
            && a.memory == b.memory
            && let Some(memory) = a.memory
        {
            for (address, value) in &a.cells {
                let Some(other) = b.cells.get(address) else {
                    continue;
                };
                if value == other {
                    continue;
                }
                let Some(address_word) = address.exact() else {
                    continue;
                };
                let range = a.range(value).hull(b.range(other));
                if range != Range::FULL {
                    contents.push((address.clone(), Atom::Content(memory, address_word), range));
                }
            }
        }
        a.values.retain(|id, value| b.values.get(id) == Some(value));
        a.cells
            .retain(|address, value| b.cells.get(address) == Some(value));
        a.predicates.retain(|id, value| {
            b.predicates
                .get(id)
                .is_some_and(|other| other.opcode == value.opcode && other.inputs == value.inputs)
        });
        a.atoms.retain(|id, range| {
            *range = range.hull(b.atoms.get(id).copied().unwrap_or(Range::FULL));
            *range != Range::FULL
        });
        a.real.retain(|expr, range| {
            if let Some(other) = b.ordinary(expr) {
                *range = range.hull(other);
                true
            } else {
                false
            }
        });
        a.bounds.retain(|expr, range| {
            *range = range.hull(b.range(expr));
            *range != Range::FULL
        });
        for (address, atom, range) in contents {
            let value = if range.low == range.high {
                Expr::constant(range.low)
            } else {
                a.atoms.insert(atom, range);
                Expr::atom(atom)
            };
            a.cells.insert(address, value);
        }
        if a.extent.is_none() {
            a.extent = extent_anchors
                .iter()
                .find_map(|address| a.cells.get(address).cloned())
                .map(Box::new);
        }
        a
    }
}
