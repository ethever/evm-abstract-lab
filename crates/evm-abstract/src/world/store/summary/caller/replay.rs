//! Materialization retains native slot operands beside independent root facts.

use super::{
    Address, AlgebraRef, BTreeMap, BTreeSet, CloneError, EvmLog, EvmNumericAlgebra, EvmStore,
    EvmStoreSummary, EvmValue, ExprId, OrderedMap, Plane, SlotStateId, SourceId, StateBinding,
    StoreError, SummaryError, SummaryPlane, U256, WordElement, WorkBudget, checkpoint, clone_map,
    observation, pay, pay_structure, scalar, source,
};
use crate::{domain::symbolic::ExprLimits, world::bytes::summary::instantiate_scalar_with_payment};

fn graph(work: usize, budget: &mut WorkBudget) -> Result<(), CloneError> {
    pay(budget, work.saturating_add(1).saturating_mul(8))
}

impl<N: WordElement> SummaryPlane<N> {
    fn instantiate_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Plane<N>, SummaryError<StoreError>> {
        pay_structure(self.clone_entries(), budget)?;
        let limits = domain.spec().relations().expressions();
        let global_default = scalar::clone_value(&self.global_default, budget)?;
        // Plane::new performs an independent native query. Reproduce its
        // construction here after paying the query on the actual copied N.
        let zero = scalar::singleton(&global_default, budget)? == Some(U256::ZERO);
        graph(4, budget)?;
        let base = if zero {
            SlotStateId::zero()
        } else {
            SlotStateId::input(SourceId::fresh().map_err(source)?)
        };
        let mut out = Plane {
            roots: OrderedMap::new(),
            base,
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
            slots: clone_map(&self.slots, budget)?,
            defaults: clone_map(&self.defaults, budget)?,
            global_default,
            initial_independence: self.initial_independence.clone(),
        };
        let owners: BTreeSet<_> = self
            .defaults
            .keys()
            .copied()
            .chain(self.slots.keys().map(|(owner, _)| *owner))
            .collect();
        for owner in owners {
            graph(4, budget)?;
            let source_id = SourceId::fresh().map_err(source)?;
            let plain = SlotStateId::input(source_id.clone());
            let zero = scalar::singleton(self.default_at(owner), budget)? == Some(U256::ZERO);
            let root = if zero {
                graph(1, budget)?;
                let mut root = SlotStateId::zero();
                for ((_, key), value) in self.slots.range((owner, U256::ZERO)..=(owner, U256::MAX))
                {
                    let singleton = scalar::singleton(value, budget)?;
                    graph(plain.nodes().saturating_add(4), budget)?;
                    let expression = if let Some(constant) = singleton {
                        ExprId::constant(constant)
                    } else {
                        let numeric = observation(value, budget)?;
                        graph(
                            plain
                                .nodes()
                                .saturating_add(numeric.work_size())
                                .saturating_add(8),
                            budget,
                        )?;
                        let observed = SlotStateId::observed(
                            source_id.clone(),
                            vec![StateBinding {
                                index: *key,
                                value: ExprId::slot_read(
                                    plain.clone(),
                                    ExprId::constant(*key),
                                    limits,
                                )
                                .map_err(source)?,
                                numeric,
                            }],
                            limits.into(),
                        )
                        .map_err(source)?;
                        graph(observed.nodes().saturating_add(2), budget)?;
                        let expression =
                            ExprId::slot_read(observed, ExprId::constant(*key), limits)
                                .map_err(source)?;
                        let operand =
                            scalar::clone_value(value, budget)?.with_expression(expression.clone());
                        out.operands.insert(expression.clone(), operand);
                        expression
                    };
                    graph(
                        root.nodes()
                            .saturating_add(expression.nodes())
                            .saturating_add(2),
                        budget,
                    )?;
                    root = root
                        .store(ExprId::constant(*key), expression, limits.into())
                        .map_err(source)?;
                }
                root
            } else {
                let mut bindings = Vec::new();
                let mut binding_work = plain.nodes();
                for ((_, key), value) in self.slots.range((owner, U256::ZERO)..=(owner, U256::MAX))
                {
                    let singleton = scalar::singleton(value, budget)?;
                    graph(plain.nodes().saturating_add(4), budget)?;
                    let expression = if let Some(constant) = singleton {
                        ExprId::constant(constant)
                    } else {
                        ExprId::slot_read(plain.clone(), ExprId::constant(*key), limits)
                            .map_err(source)?
                    };
                    let numeric = observation(value, budget)?;
                    binding_work = binding_work
                        .saturating_add(expression.nodes())
                        .saturating_add(numeric.work_size());
                    bindings.push(StateBinding {
                        index: *key,
                        value: expression,
                        numeric,
                    });
                }
                for binding in &bindings {
                    if let Some(value) = self.slots.get(&(owner, binding.index)) {
                        let operand = scalar::clone_value(value, budget)?
                            .with_expression(binding.value.clone());
                        out.operands.insert(binding.value.clone(), operand);
                    }
                }
                graph(
                    binding_work.saturating_add(bindings.len().saturating_mul(4)),
                    budget,
                )?;
                SlotStateId::observed(source_id, bindings, limits.into()).map_err(source)?
            };
            out.roots.insert(owner, root);
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

fn instantiate_map<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    values: &OrderedMap<Address, EvmValue<N>>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<OrderedMap<Address, EvmValue<N>>, SummaryError<StoreError>> {
    pay_structure(values.len(), budget)?;
    let mut out = OrderedMap::new();
    for (owner, value) in values {
        let value = instantiate_scalar_with_payment(value, domain, budget)
            .map_err(|error| error.map_source(StoreError::from))?;
        out.insert(*owner, value);
    }
    checkpoint(budget)?;
    Ok(out)
}

fn validate<N: WordElement>(
    store: &EvmStore<N>,
    limits: ExprLimits,
    budget: &mut WorkBudget,
) -> Result<(), SummaryError<StoreError>> {
    // Store::check_existing visits synthetic Top wrappers around root terms.
    // Newly replayed planes have no conditional scopes/failure; validate the
    // same actual values and roots directly, without those native constructors.
    pay_structure(store.clone_prefix_work(), budget)?;
    for plane in [&store.persistent, &store.transient] {
        for root in std::iter::once(&plane.base).chain(plane.roots.values()) {
            graph(root.nodes(), budget)?;
            root.join_guarded(root, ExprId::constant(U256::ZERO), limits.into())
                .map_err(source)?;
        }
        for value in plane
            .slots
            .values()
            .chain(plane.defaults.values())
            .chain(plane.operands.values())
            .chain([&plane.global_default])
        {
            crate::world::bytes::check_value(value, limits).map_err(source)?;
        }
    }
    for value in store
        .balances
        .values()
        .chain(store.nonces.values())
        .chain([&store.balance_default])
    {
        crate::world::bytes::check_value(value, limits).map_err(source)?;
    }
    for log in store.possible_logs.values() {
        pay_structure(log.topics.len(), budget)?;
        for topic in &log.topics {
            crate::world::bytes::check_value(topic, limits).map_err(source)?;
        }
        // The paid byte instantiation validates its own fresh root/payload.
    }
    checkpoint(budget)?;
    Ok(())
}

impl<N: WordElement> EvmStoreSummary<N> {
    pub(crate) fn instantiate_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmStore<N>, SummaryError<StoreError>> {
        pay_structure(self.clone_prefix_work(), budget)?;
        let persistent = self.persistent.instantiate_with_payment(domain, budget)?;
        let transient = self.transient.instantiate_with_payment(domain, budget)?;
        let balances = instantiate_map(&self.balances, domain, budget)?;
        let balance_default = scalar::clone_value(&self.balance_default, budget)?;
        let nonces = instantiate_map(&self.nonces, domain, budget)?;
        let mut possible_logs = OrderedMap::new();
        for (key, log) in &self.possible_logs {
            pay_structure(log.topics.len(), budget)?;
            let mut topics = Vec::with_capacity(log.topics.len());
            for topic in &log.topics {
                topics.push(
                    instantiate_scalar_with_payment(topic, domain, budget)
                        .map_err(|error| error.map_source(StoreError::from))?,
                );
            }
            let data = log
                .data
                .instantiate_with_payment(domain, budget)
                .map_err(|error| error.map_source(StoreError::from))?;
            possible_logs.insert(key.clone(), EvmLog { topics, data });
        }
        let out = EvmStore {
            persistent,
            transient,
            balances,
            balance_default,
            codes: self.codes.clone(),
            nonces,
            existence: self.existence.clone(),
            created: self.created.clone(),
            created_default: self.created_default,
            pending_destruction: self.pending_destruction.clone(),
            pending_destruction_default: self.pending_destruction_default,
            touched: self.touched.clone(),
            touched_default: self.touched_default,
            possible_logs,
            logs_unknown: self.logs_unknown,
        };
        validate(&out, domain.spec().relations().expressions(), budget)?;
        checkpoint(budget)?;
        Ok(out)
    }
}
