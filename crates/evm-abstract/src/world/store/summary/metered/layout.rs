//! Layout coordinates are borrowed; defaults always come from their own input.

use super::super::InitialIndependence;
use super::{
    AlgebraRef, BTreeSet, CloneError, EvmNumericAlgebra, EvmStoreSummary, EvmValue, NumericMap,
    OrderedMap, SummaryLog, SummaryPlane, U256, WordElement, WorkBudget, checkpoint, lifecycle,
    pay_structure, representable_default_with_payment, scalar, sparse,
};

fn project_numeric<K: Ord + Clone, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    (values, default): (&NumericMap<K, N>, &EvmValue<N>),
    layout: &NumericMap<K, N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<(NumericMap<K, N>, EvmValue<N>), CloneError> {
    pay_structure(values.len().saturating_add(layout.len()), budget)?;
    let mut upper = scalar::clone_value(default, budget)?;
    for (key, value) in values {
        if layout.get(key).is_none() {
            upper = scalar::join(&upper, value, domain, false, budget)?;
        }
    }
    let mut retained = OrderedMap::new();
    for key in layout.keys() {
        // Forgotten cells widen only the output default, never a missing
        // layout coordinate's original input value.
        let value = values.get(key).unwrap_or(default);
        if !scalar::equal(value, &upper, domain, budget)? {
            retained.insert(key.clone(), scalar::clone_value(value, budget)?);
        }
    }
    checkpoint(budget)?;
    Ok((retained, upper))
}

impl<N: WordElement> SummaryPlane<N> {
    fn project_to_layout_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        layout: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(
            self.clone_entries().saturating_add(layout.clone_entries()),
            budget,
        )?;
        let owners: BTreeSet<_> = layout
            .defaults
            .keys()
            .copied()
            .chain(layout.slots.keys().map(|(owner, _)| *owner))
            .collect();
        let mut global = scalar::clone_value(&self.global_default, budget)?;
        for (owner, value) in &self.defaults {
            if !owners.contains(owner) {
                global = scalar::join(&global, value, domain, false, budget)?;
            }
        }
        for ((owner, _), value) in &self.slots {
            if !owners.contains(owner) {
                global = scalar::join(&global, value, domain, false, budget)?;
            }
        }
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: representable_default_with_payment(&global, domain, budget)?,
            initial_independence: InitialIndependence::new(false),
        };
        for owner in owners {
            let mut default = scalar::clone_value(self.default_at(owner), budget)?;
            for ((_, key), value) in self.slots.range((owner, U256::ZERO)..=(owner, U256::MAX)) {
                if layout.slots.get(&(owner, *key)).is_none() {
                    default = scalar::join(&default, value, domain, false, budget)?;
                }
            }
            let default = representable_default_with_payment(&default, domain, budget)?;
            if !scalar::equal(&default, &out.global_default, domain, budget)? {
                out.defaults
                    .insert(owner, scalar::clone_value(&default, budget)?);
            }
            for ((_, key), _) in layout.slots.range((owner, U256::ZERO)..=(owner, U256::MAX)) {
                let value = self.at(owner, *key);
                if !scalar::equal(value, &default, domain, budget)? {
                    out.slots
                        .insert((owner, *key), scalar::clone_value(value, budget)?);
                }
            }
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Forget numeric coordinates without copying payloads that are discarded.
    /// Code, lifecycle, existence and transaction touch facts survive exactly.
    pub(crate) fn forget_loop_numeric_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(self.clone_prefix_work(), budget)?;
        let mut plane = || {
            Ok::<_, CloneError>(SummaryPlane {
                slots: OrderedMap::new(),
                defaults: OrderedMap::new(),
                global_default: scalar::top(domain, budget)?,
                initial_independence: InitialIndependence::new(false),
            })
        };
        let persistent = plane()?;
        let transient = plane()?;
        let out = Self {
            persistent,
            transient,
            balances: OrderedMap::new(),
            balance_default: scalar::top(domain, budget)?,
            codes: self.codes.clone(),
            nonces: OrderedMap::new(),
            existence: self.existence.clone(),
            created: self.created.clone(),
            created_default: self.created_default,
            pending_destruction: self.pending_destruction.clone(),
            pending_destruction_default: self.pending_destruction_default,
            touched: self.touched.clone(),
            touched_default: self.touched_default,
            possible_logs: OrderedMap::new(),
            logs_unknown: true,
        };
        checkpoint(budget)?;
        Ok(out)
    }

    /// Freeze sparse coordinates with separate paid steps on every actual
    /// intermediate. Layout scalars are never substituted for input defaults.
    pub(crate) fn project_to_layout_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        layout: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay_structure(
            self.clone_prefix_work()
                .saturating_add(layout.clone_prefix_work()),
            budget,
        )?;
        let persistent =
            self.persistent
                .project_to_layout_with_payment(&layout.persistent, domain, budget)?;
        let transient =
            self.transient
                .project_to_layout_with_payment(&layout.transient, domain, budget)?;
        let (balances, balance_default) = project_numeric(
            (&self.balances, &self.balance_default),
            &layout.balances,
            domain,
            budget,
        )?;
        let mut nonces = OrderedMap::new();
        for (owner, value) in &self.nonces {
            if layout.nonces.get(owner).is_some() {
                nonces.insert(*owner, scalar::clone_value(value, budget)?);
            }
        }
        let codes = self
            .codes
            .iter()
            .filter_map(|(owner, code)| layout.codes.get(owner).map(|_| (*owner, code.clone())))
            .collect();
        let existence = self
            .existence
            .iter()
            .filter_map(|(owner, value)| layout.existence.get(owner).map(|_| (*owner, *value)))
            .collect();
        let (created, created_default) = sparse::project(
            (&self.created, &self.created_default),
            &layout.created,
            lifecycle::join,
        );
        let (pending_destruction, pending_destruction_default) = sparse::project(
            (&self.pending_destruction, &self.pending_destruction_default),
            &layout.pending_destruction,
            lifecycle::join,
        );
        let (touched, touched_default) = sparse::project(
            (&self.touched, &self.touched_default),
            &layout.touched,
            lifecycle::join,
        );
        let logs_unknown = self.logs_unknown
            || self
                .possible_logs
                .keys()
                .any(|key| layout.possible_logs.get(key).is_none());
        let mut possible_logs = OrderedMap::new();
        if !logs_unknown {
            for (key, log) in &self.possible_logs {
                pay_structure(log.topics.len(), budget)?;
                let mut topics = Vec::with_capacity(log.topics.len());
                for topic in &log.topics {
                    topics.push(scalar::clone_value(topic, budget)?);
                }
                let data = log.data.project_to_layout_with_payment(
                    &layout.possible_logs[key].data,
                    domain,
                    budget,
                )?;
                possible_logs.insert(key.clone(), SummaryLog { topics, data });
            }
        }
        let out = Self {
            persistent,
            transient,
            balances,
            balance_default,
            codes,
            nonces,
            existence,
            created,
            created_default,
            pending_destruction,
            pending_destruction_default,
            touched,
            touched_default,
            possible_logs,
            logs_unknown,
        };
        checkpoint(budget)?;
        Ok(out)
    }
}
