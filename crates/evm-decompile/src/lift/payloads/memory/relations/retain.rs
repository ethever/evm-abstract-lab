//! Content identities describe a reaching memory value, not its entire history.
//! Prune dead content support after simultaneous edge binding, before cloning
//! it into retained flows. Existing scalar and iteration-only facts stay intact.
use super::{Atom, Expr, Source, State};
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

impl<P: FnMut(usize) -> bool> Source<'_, P> {
    pub(super) fn retain_contents(&mut self, state: &mut State) -> bool {
        let records = state
            .values
            .len()
            .saturating_add(state.cells.len().saturating_mul(2))
            .saturating_add(state.predicates.len())
            .saturating_add(state.real.len())
            .saturating_add(state.bounds.len())
            .saturating_add(state.atoms.len());
        let records = records.saturating_add(usize::from(state.extent.is_some()));
        if !self.charge(records.saturating_mul(4).saturating_add(1)) {
            return false;
        }
        let roots = state
            .values
            .values()
            .chain(state.cells.keys())
            .chain(state.cells.values())
            .chain(state.extent.as_deref());
        let root_slots = roots
            .clone()
            .fold(0_usize, |n, expr| n.saturating_add(expr.terms.len()));
        let predicate_slots = state.predicates.values().fold(0_usize, |n, predicate| {
            predicate
                .inputs
                .iter()
                .fold(n.saturating_add(1), |n, expr| {
                    n.saturating_add(expr.terms.len())
                })
        });
        let fact_slots = state
            .real
            .keys()
            .chain(state.bounds.keys())
            .fold(state.atoms.len(), |n, expr| {
                n.saturating_add(expr.terms.len()).saturating_add(1)
            });
        let slots = root_slots
            .saturating_add(predicate_slots)
            .saturating_add(fact_slots);
        // Quote the complete support graph and fact scan before allocating its
        // worklist/set. Each predicate's inputs are visited at most once.
        if !self.charge(slots.saturating_mul(4).saturating_add(1)) {
            return false;
        }
        let mut pending: Vec<_> = roots.flat_map(|expr| expr.terms.keys().copied()).collect();
        let mut live = BTreeSet::new();
        while let Some(atom) = pending.pop() {
            if !live.insert(atom) {
                continue;
            }
            if let Some(predicate) = state.predicates.get(&atom) {
                pending.extend(
                    predicate
                        .inputs
                        .iter()
                        .flat_map(|expr| expr.terms.keys().copied()),
                );
            }
        }
        let dead = |atom: &Atom| matches!(atom, Atom::Content(..)) && !live.contains(atom);
        let supported = |expr: &Expr| !expr.terms.keys().any(dead);
        state.atoms.retain(|atom, _| !dead(atom));
        // Shifted anchors with the same terms remain usable by ordinary().
        state.real.retain(|expr, _| supported(expr));
        state.bounds.retain(|expr, _| supported(expr));
        state
            .predicates
            .retain(|atom, predicate| !dead(atom) && predicate.inputs.iter().all(supported));
        true
    }
}
