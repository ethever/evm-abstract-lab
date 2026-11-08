//! Widen only at feedback points of the discovered contextual CFG.
//!
//! Each newly admitted source -> target edge checks whether target already
//! reaches source. The last discovered edge of every cycle therefore marks a
//! target on that cycle. Acyclic fan-in retains ordinary value joins, regardless
//! of the number or arrival order of its predecessors. Retaining old edges and
//! marks is conservative when later proofs remove a path.

use crate::{Progress, artifact::Builder};
use analysis_kernel::cost;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct FeedbackGraph {
    successors: BTreeMap<usize, BTreeSet<usize>>,
    points: BTreeSet<usize>,
}
impl FeedbackGraph {
    /// Observe a real context edge before joining its destination state. Edge
    /// roles do not distinguish topology; repeated observations never redo DFS.
    pub(super) fn observe(
        &mut self,
        source: usize,
        target: usize,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<()> {
        if !owner.pay(
            cost::key_lookup_work(self.successors.len(), 1)
                .saturating_add(cost::key_lookup_work(self.points.len(), 1)),
        ) {
            return None;
        }
        let marked = self.points.contains(&target);
        let outgoing = self.successors.get(&source);
        let degree = outgoing.map_or(0, BTreeSet::len);
        if !owner.pay(cost::key_lookup_work(degree, 1)) {
            return None;
        }
        if outgoing.is_some_and(|outgoing| outgoing.contains(&target)) {
            return Some(());
        }
        // An existing mark already lies on every cycle this edge could close,
        // so only an unmarked target needs a new reachability query.
        let feedback = marked || source == target || self.reaches(target, source, owner)?;
        let insertion = cost::key_insert_work(self.successors.len(), 1, self.successors.len())
            .saturating_add(cost::key_insert_work(degree, 1, degree))
            .saturating_add(if feedback && !marked {
                cost::key_insert_work(self.points.len(), 1, self.points.len())
            } else {
                0
            });
        if !owner.pay(insertion) {
            return None;
        }
        self.successors.entry(source).or_default().insert(target);
        if feedback && !marked {
            self.points.insert(target);
        }
        Some(())
    }

    pub(super) fn is_point(
        &self,
        target: usize,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<bool> {
        owner
            .pay(cost::key_lookup_work(self.points.len(), 1))
            .then(|| self.points.contains(&target))
    }

    /// Traversal is iterative and visits only reachable topology. Every queue,
    /// visited-set and adjacency action is reserved before it executes, keeping
    /// cancellation observable even for a large strongly connected component.
    fn reaches(
        &self,
        start: usize,
        goal: usize,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<bool> {
        if !owner.pay(cost::key_lookup_work(self.successors.len(), 1)) {
            return None;
        }
        if !self.successors.contains_key(&start) {
            return Some(false);
        }
        if !owner.pay(cost::key_insert_work(0, 1, 0).saturating_mul(2)) {
            return None;
        }
        let mut seen = BTreeSet::from([start]);
        let mut pending = BTreeSet::from([start]);
        while !pending.is_empty() {
            if !owner.pay(
                cost::tree_work(pending.len())
                    .saturating_mul(3)
                    .saturating_add(4),
            ) {
                return None;
            }
            let current = pending.pop_first().expect("nonempty traversal frontier");
            if current == goal {
                return Some(true);
            }
            if !owner.pay(cost::key_lookup_work(self.successors.len(), 1)) {
                return None;
            }
            if let Some(successors) = self.successors.get(&current) {
                for successor in successors {
                    if !owner.pay(cost::key_lookup_work(seen.len(), 1)) {
                        return None;
                    }
                    if seen.contains(successor) {
                        continue;
                    }
                    if !owner.pay(
                        cost::key_insert_work(seen.len(), 1, seen.len())
                            .saturating_add(cost::key_insert_work(pending.len(), 1, pending.len())),
                    ) {
                        return None;
                    }
                    seen.insert(*successor);
                    pending.insert(*successor);
                }
            }
        }
        Some(false)
    }
}
