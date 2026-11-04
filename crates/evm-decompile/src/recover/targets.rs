//! Monotone discovery shared by sibling contexts of the same indirect jump.
//!
//! Only legal destinations contributed by finite facts or retained witnesses
//! enter a pool. Unresolved contexts subscribe and get woken on growth; they
//! propagate their own frames and keep their unknown exit. Finite contexts never
//! follow sibling targets. Site identity excludes changing numeric facts and
//! separates incompatible input heights and branch roles.

use crate::{EdgeKind, Progress, artifact::Builder};
use analysis_kernel::cost;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct JumpSite {
    pub(super) original: usize,
    pub(super) height: usize,
    pub(super) kind: EdgeKind,
}

#[derive(Default)]
struct Discovered {
    targets: BTreeSet<usize>,
    subscribers: BTreeSet<usize>,
}

#[derive(Default)]
pub(super) struct TargetPools {
    sites: BTreeMap<JumpSite, Discovered>,
}

impl TargetPools {
    /// Add discovered legal PCs, returning the unresolved contexts to requeue.
    pub(super) fn observe(
        &mut self,
        site: JumpSite,
        targets: &[usize],
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<Vec<usize>> {
        if targets.is_empty() {
            return Some(Vec::new());
        }
        if !owner.pay(cost::key_lookup_work(self.sites.len(), 3)) {
            return None;
        }
        let (known, subscribers) = self
            .sites
            .get(&site)
            .map_or((0, 0), |s| (s.targets.len(), s.subscribers.len()));
        // Reserve owned insertions and a possible subscriber copy before growth.
        // Each actual wake separately pays the queue's enqueue tariff.
        if !owner.pay(
            targets
                .len()
                .saturating_mul(cost::tree_work(known.saturating_add(targets.len())))
                .saturating_add(subscribers),
        ) {
            return None;
        }
        let discovered = self.sites.entry(site).or_default();
        discovered.targets.extend(targets.iter().copied());
        Some(if discovered.targets.len() != known {
            discovered.subscribers.iter().copied().collect()
        } else {
            Vec::new()
        })
    }

    /// Register an unresolved context and copy the currently discovered PCs.
    pub(super) fn subscribe(
        &mut self,
        site: JumpSite,
        context: usize,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<Vec<usize>> {
        if !owner.pay(cost::key_lookup_work(self.sites.len(), 3)) {
            return None;
        }
        let (known, subscribers) = self
            .sites
            .get(&site)
            .map_or((0, 0), |s| (s.targets.len(), s.subscribers.len()));
        if !owner.pay(cost::tree_work(subscribers).saturating_add(known)) {
            return None;
        }
        let discovered = self.sites.entry(site).or_default();
        discovered.subscribers.insert(context);
        Some(discovered.targets.iter().copied().collect())
    }
}
