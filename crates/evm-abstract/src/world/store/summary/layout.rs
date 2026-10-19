//! Freeze sparse loop coordinates; forgotten cells flow into their defaults.

use super::{
    AbstractValue, BTreeSet, Domain, InitialIndependence, OrderedMap, StoreSummary, SummaryPlane,
    lifecycle, representable_default, sparse,
};

impl SummaryPlane {
    fn project_to_layout(&self, layout: &Self, domain: Domain) -> Self {
        let owners: BTreeSet<_> = layout
            .defaults
            .keys()
            .copied()
            .chain(layout.slots.keys().map(|(owner, _)| *owner))
            .collect();
        let mut global = self.global_default.clone();
        for (owner, value) in &self.defaults {
            if !owners.contains(owner) {
                global = domain.join(&global, value);
            }
        }
        for ((owner, _), value) in &self.slots {
            if !owners.contains(owner) {
                global = domain.join(&global, value);
            }
        }
        let global_default = representable_default(&global, domain);
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default,
            // Root projection cannot invent independence from an initial fact.
            initial_independence: InitialIndependence::new(false),
        };
        for owner in owners {
            let mut default = self.default_at(owner).clone();
            for ((_, key), value) in self
                .slots
                .range((owner, alloy_primitives::U256::ZERO)..=(owner, alloy_primitives::U256::MAX))
            {
                if layout.slots.get(&(owner, *key)).is_none() {
                    default = domain.join(&default, value);
                }
            }
            let default = representable_default(&default, domain);
            if default != out.global_default {
                out.defaults.insert(owner, default.clone());
            }
            for ((_, key), _) in layout
                .slots
                .range((owner, alloy_primitives::U256::ZERO)..=(owner, alloy_primitives::U256::MAX))
            {
                let value = self.at(owner, *key);
                if *value != default {
                    out.slots.insert((owner, *key), value.clone());
                }
            }
        }
        out
    }
}

impl StoreSummary {
    /// A complete coarse envelope when retaining numerical cell expressions is
    /// refused. Control-relevant code/lifecycle variants remain represented.
    pub(crate) fn forget_loop_numeric(&self) -> Self {
        let mut out = self.clone();
        let plane = || SummaryPlane {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: AbstractValue::top(),
            initial_independence: InitialIndependence::new(false),
        };
        out.persistent = plane();
        out.transient = plane();
        out.balances.clear();
        out.balance_default = AbstractValue::top();
        out.nonces.clear();
        out.possible_logs.clear();
        out.logs_unknown = true;
        out
    }

    /// Project numerical coordinates onto one frozen loop layout. Code and
    /// creation/destruction variants remain in the scheduler's structural key.
    pub(crate) fn project_to_layout(&self, layout: &Self, domain: Domain) -> Self {
        let mut out = self.clone();
        out.persistent = self
            .persistent
            .project_to_layout(&layout.persistent, domain);
        out.transient = self.transient.project_to_layout(&layout.transient, domain);
        (out.balances, out.balance_default) = sparse::project(
            (&self.balances, &self.balance_default),
            &layout.balances,
            |left, right| domain.join(left, right),
        );
        // Unlisted nonces already have Top as their complete upper bound.
        out.nonces
            .retain(|owner, _| layout.nonces.get(owner).is_some());
        // Missing code/existence observations denote Unknown. Saved rollback
        // stores may have a different code variant from the current store.
        out.codes
            .retain(|owner, _| layout.codes.get(owner).is_some());
        out.existence
            .retain(|owner, _| layout.existence.get(owner).is_some());
        (out.created, out.created_default) = sparse::project(
            (&self.created, &self.created_default),
            &layout.created,
            lifecycle::join,
        );
        (out.pending_destruction, out.pending_destruction_default) = sparse::project(
            (&self.pending_destruction, &self.pending_destruction_default),
            &layout.pending_destruction,
            lifecycle::join,
        );
        (out.touched, out.touched_default) = sparse::project(
            (&self.touched, &self.touched_default),
            &layout.touched,
            lifecycle::join,
        );
        if self
            .possible_logs
            .keys()
            .any(|key| layout.possible_logs.get(key).is_none())
        {
            out.logs_unknown = true;
        }
        if out.logs_unknown {
            out.possible_logs.clear();
        } else {
            for key in out.possible_logs.keys().cloned().collect::<Vec<_>>() {
                let log = out
                    .possible_logs
                    .get_mut(&key)
                    .expect("retained source key");
                log.data = log
                    .data
                    .project_to_layout(&layout.possible_logs[&key].data, domain);
            }
        }
        out
    }
}
