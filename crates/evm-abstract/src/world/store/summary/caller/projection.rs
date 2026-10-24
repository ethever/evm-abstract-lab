//! Project current slot roots through the shared paid native reader.

use super::super::metered::representable_default_with_payment;
use super::{
    AlgebraRef, BTreeMap, BTreeSet, CloneError, Code, EvmNumericAlgebra, EvmStore, EvmStoreSummary,
    EvmValue, Existence, ExprId, OrderedMap, Plane, SlotStateId, SlotStateKind, StoreError,
    SummaryError, SummaryLog, SummaryPlane, U256, WordElement, WorkBudget, checkpoint, numeric,
    pay, pay_structure, scalar, source, sparse,
};
use crate::world::bytes::conditional::Context;

fn byte_source(error: SummaryError<crate::world::RangeError>) -> SummaryError<StoreError> {
    error.map_source(StoreError::from)
}

fn candidate_count(keys: &crate::world::Candidates<'_>) -> usize {
    keys.finite
        .map_or_else(|| usize::from(keys.singleton.is_some()), BTreeSet::len)
}

fn candidates<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<Option<BTreeSet<U256>>, SummaryError<StoreError>> {
    // Native discovery owns its existing complete view/singleton fees. Only an
    // actual finite result allocates a copied coordinate tree; cached length is
    // available after discovery and before any key is visited or allocated.
    pay(budget, 16)?;
    let Some(keys) = numeric::candidates_with_payment(value, budget).map_err(byte_source)? else {
        return Ok(None);
    };
    pay_structure(candidate_count(&keys), budget)?;
    let out = keys.iter().copied().collect();
    checkpoint(budget)?;
    Ok(Some(out))
}

fn candidate_subset<N: WordElement>(
    value: &EvmValue<N>,
    keys: &BTreeSet<U256>,
    budget: &mut WorkBudget,
) -> Result<bool, SummaryError<StoreError>> {
    pay(budget, 16)?;
    let Some(written) = numeric::candidates_with_payment(value, budget).map_err(byte_source)?
    else {
        return Ok(false);
    };
    let count = candidate_count(&written);
    // Each borrowed U256 membership visits at most all retained keys, with
    // four-limb comparison and tree-edge overhead. Fund full iteration/setup
    // and cleanup without constructing an intermediate coordinate tree.
    pay(
        budget,
        count
            .saturating_mul(keys.len().saturating_add(1))
            .saturating_mul(32)
            .saturating_add(16),
    )?;
    let subset = written.is_subset(keys);
    checkpoint(budget)?;
    Ok(subset)
}

fn branch<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<Option<bool>, SummaryError<StoreError>> {
    if !numeric::nonzero_with_payment(value, budget).map_err(byte_source)? {
        Ok(Some(false))
    } else if !numeric::contains_with_payment(value, U256::ZERO, budget).map_err(byte_source)? {
        Ok(Some(true))
    } else {
        Ok(None)
    }
}

fn constant_keys<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    plane: &Plane<N>,
    root: &SlotStateId,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<BTreeSet<U256>, SummaryError<StoreError>> {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let mut keys = BTreeSet::new();
    while let Some(root) = pending.pop() {
        pay(
            budget,
            24usize
                .saturating_add((seen.len().saturating_add(1).ilog2() as usize).saturating_mul(8)),
        )?;
        if !seen.insert(root.id()) {
            continue;
        }
        match root.kind() {
            SlotStateKind::Zero => {}
            SlotStateKind::Input { bindings, .. } => {
                pay_structure(bindings.len().saturating_add(keys.len()), budget)?;
                keys.extend(bindings.iter().map(|binding| binding.index));
            }
            SlotStateKind::Store { previous, key, .. } => {
                let value = numeric::scalar_with_payment(key, &plane.operands, domain, budget)
                    .map_err(byte_source)?;
                if let Some(constants) = candidates(&value, budget)? {
                    keys.extend(constants);
                }
                pending.push(previous);
            }
            SlotStateKind::Ite {
                then_state,
                else_state,
                ..
            } => pending.extend([then_state, else_state]),
        }
    }
    checkpoint(budget)?;
    Ok(keys)
}

fn remaining_upper<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    plane: &Plane<N>,
    root: &SlotStateId,
    keys: &BTreeSet<U256>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
    memo: &mut BTreeMap<SlotStateId, EvmValue<N>>,
) -> Result<EvmValue<N>, SummaryError<StoreError>> {
    pay(
        budget,
        24usize.saturating_add((memo.len().saturating_add(1).ilog2() as usize).saturating_mul(8)),
    )?;
    if let Some(value) = memo.get(root) {
        return Ok(scalar::clone_value(value, budget)?);
    }
    let value = match root.kind() {
        SlotStateKind::Zero => {
            let zero = scalar::constant(U256::ZERO, domain, budget)?;
            scalar::scalar(&zero, domain, budget)?
        }
        SlotStateKind::Input { .. } => scalar::top(domain, budget)?,
        SlotStateKind::Store {
            previous,
            key,
            value,
        } => {
            let old = remaining_upper(plane, previous, keys, domain, budget, memo)?;
            let key = numeric::scalar_with_payment(key, &plane.operands, domain, budget)
                .map_err(byte_source)?;
            if candidate_subset(&key, keys, budget)? {
                old
            } else {
                let value = numeric::scalar_with_payment(value, &plane.operands, domain, budget)
                    .map_err(byte_source)?;
                let value = scalar::scalar(&value, domain, budget)?;
                scalar::join_scalar(&old, &value, domain, false, budget)?
            }
        }
        SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } => {
            let truth = numeric::scalar_with_payment(condition, &plane.operands, domain, budget)
                .map_err(byte_source)?;
            match branch(&truth, budget)? {
                Some(false) => remaining_upper(plane, else_state, keys, domain, budget, memo)?,
                Some(true) => remaining_upper(plane, then_state, keys, domain, budget, memo)?,
                None => {
                    let left = remaining_upper(plane, then_state, keys, domain, budget, memo)?;
                    let right = remaining_upper(plane, else_state, keys, domain, budget, memo)?;
                    scalar::join_scalar(&left, &right, domain, false, budget)?
                }
            }
        }
    };
    memo.insert(root.clone(), scalar::clone_value(&value, budget)?);
    checkpoint(budget)?;
    Ok(value)
}

impl<N: WordElement> Plane<N> {
    fn summary_projection_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<SummaryPlane<N>, SummaryError<StoreError>> {
        pay_structure(
            self.roots
                .len()
                .saturating_add(self.slots.len())
                .saturating_add(self.defaults.len())
                .saturating_add(self.initial_independence.slots.len())
                .saturating_add(self.initial_independence.defaults.len()),
            budget,
        )?;
        if let Some(error) = &self.failure {
            return Err(SummaryError::Source(error.clone()));
        }
        let upper = remaining_upper(
            self,
            &self.base,
            &BTreeSet::new(),
            domain,
            budget,
            &mut BTreeMap::new(),
        )?;
        let mut out = SummaryPlane {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default: representable_default_with_payment(&upper, domain, budget)?,
            initial_independence: self.initial_independence.clone(),
        };
        let addresses: BTreeSet<_> = self
            .roots
            .keys()
            .chain(self.defaults.keys())
            .copied()
            .chain(self.slots.keys().map(|(owner, _)| *owner))
            .collect();
        for owner in addresses {
            // Account-source reconstruction is purely symbolic, not N work.
            pay(
                budget,
                self.base.work_size().saturating_add(1).saturating_mul(8),
            )?;
            let root = self.root_at(owner).map_err(source)?;
            let mut keys = constant_keys(self, &root, domain, budget)?;
            keys.extend(
                self.slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), _)| *key),
            );
            let upper = remaining_upper(self, &root, &keys, domain, budget, &mut BTreeMap::new())?;
            let default = representable_default_with_payment(&upper, domain, budget)?;
            if !scalar::equal(&default, &out.global_default, domain, budget)? {
                out.defaults
                    .insert(owner, scalar::clone_value(&default, budget)?);
            }
            let context =
                Context::with_payment(&self.operands, Some(&self.conditional), domain, budget)
                    .map_err(byte_source)?;
            for key in keys {
                pay(
                    budget,
                    24usize.saturating_add(
                        (out.slots.len().saturating_add(1).ilog2() as usize).saturating_mul(8),
                    ),
                )?;
                let value = numeric::slot_with_payment(
                    &root,
                    &ExprId::constant(key),
                    &context,
                    domain,
                    budget,
                )
                .map_err(byte_source)?;
                let value = scalar::scalar(&value, domain, budget)?;
                if !scalar::equal(&value, &default, domain, budget)? {
                    out.slots.insert((owner, key), value);
                }
            }
        }
        checkpoint(budget)?;
        Ok(out)
    }
}

fn scalar_map<K: Ord + Clone, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    values: &OrderedMap<K, EvmValue<N>>,
    default: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<OrderedMap<K, EvmValue<N>>, CloneError> {
    pay_structure(values.len(), budget)?;
    let mut out = OrderedMap::new();
    for (key, value) in values {
        let value = scalar::scalar(value, domain, budget)?;
        if !scalar::equal(&value, default, domain, budget)? {
            out.insert(key.clone(), value);
        }
    }
    checkpoint(budget)?;
    Ok(out)
}

impl<N: WordElement> EvmStore<N> {
    pub(crate) fn summary_projection_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmStoreSummary<N>, SummaryError<StoreError>> {
        pay_structure(self.clone_prefix_work(), budget)?;
        let mut possible_logs = OrderedMap::new();
        for (key, log) in &self.possible_logs {
            pay_structure(log.topics.len(), budget)?;
            let mut topics = Vec::with_capacity(log.topics.len());
            for topic in &log.topics {
                topics.push(scalar::scalar(topic, domain, budget)?);
            }
            let data = log
                .data
                .summary_projection_with_payment(domain, budget)
                .map_err(byte_source)?;
            possible_logs.insert(key.clone(), SummaryLog { topics, data });
        }
        let balance_default = scalar::scalar(&self.balance_default, domain, budget)?;
        let persistent = self
            .persistent
            .summary_projection_with_payment(domain, budget)?;
        let transient = self
            .transient
            .summary_projection_with_payment(domain, budget)?;
        let balances = scalar_map(&self.balances, &balance_default, domain, budget)?;
        let nonce_default = scalar::top(domain, budget)?;
        let nonces = scalar_map(&self.nonces, &nonce_default, domain, budget)?;
        let out = EvmStoreSummary {
            persistent,
            transient,
            balances,
            balance_default,
            codes: self
                .codes
                .iter()
                .filter(|(_, code)| !matches!(code, Code::Unknown))
                .map(|(key, code)| (*key, code.clone()))
                .collect(),
            nonces,
            existence: sparse::map((&self.existence, &Existence::Unknown), |value| *value),
            created: sparse::map((&self.created, &self.created_default), |value| *value),
            created_default: self.created_default,
            pending_destruction: sparse::map(
                (&self.pending_destruction, &self.pending_destruction_default),
                |value| *value,
            ),
            pending_destruction_default: self.pending_destruction_default,
            touched: sparse::map((&self.touched, &self.touched_default), |value| *value),
            touched_default: self.touched_default,
            possible_logs: if self.logs_unknown {
                OrderedMap::new()
            } else {
                possible_logs
            },
            logs_unknown: self.logs_unknown,
        };
        checkpoint(budget)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
