//! Reproduce derived summary Eq without an unquoted native comparison.

use super::{
    AlgebraRef, CloneError, EvmNumericAlgebra, EvmStoreSummary, EvmValue, OrderedMap, SummaryPlane,
    WordElement, WorkBudget, checkpoint, code_comparison_work, pay, pay_structure, scalar,
};

fn answer(equal: bool, budget: &WorkBudget) -> Result<bool, CloneError> {
    checkpoint(budget)?;
    Ok(equal)
}

fn map_equal<K: Ord + Clone, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &OrderedMap<K, EvmValue<N>>,
    right: &OrderedMap<K, EvmValue<N>>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, CloneError> {
    pay_structure(left.len().saturating_add(right.len()), budget)?;
    if left.len() != right.len() {
        return answer(false, budget);
    }
    for ((left_key, left), (right_key, right)) in left.iter().zip(right.iter()) {
        if left_key != right_key || !scalar::equal(left, right, domain, budget)? {
            return answer(false, budget);
        }
    }
    answer(true, budget)
}

impl<N: WordElement> SummaryPlane<N> {
    fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        pay_structure(
            self.clone_entries().saturating_add(other.clone_entries()),
            budget,
        )?;
        let equal = map_equal(&self.slots, &other.slots, domain, budget)?
            && map_equal(&self.defaults, &other.defaults, domain, budget)?
            && scalar::equal(&self.global_default, &other.global_default, domain, budget)?
            && self.initial_independence == other.initial_independence;
        answer(equal, budget)
    }
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Exact existing representation equality, including native predicates,
    /// scalar metadata, owner coordinates, code, lifecycle and event payloads.
    pub(crate) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        pay_structure(
            self.clone_prefix_work()
                .saturating_add(other.clone_prefix_work()),
            budget,
        )?;
        if !self
            .persistent
            .eq_with_payment(&other.persistent, domain, budget)?
            || !self
                .transient
                .eq_with_payment(&other.transient, domain, budget)?
            || !map_equal(&self.balances, &other.balances, domain, budget)?
            || !scalar::equal(
                &self.balance_default,
                &other.balance_default,
                domain,
                budget,
            )?
        {
            return answer(false, budget);
        }
        if self.codes.len() != other.codes.len() {
            return answer(false, budget);
        }
        for ((left_owner, left), (right_owner, right)) in self.codes.iter().zip(other.codes.iter())
        {
            if left_owner != right_owner {
                return answer(false, budget);
            }
            pay(budget, code_comparison_work(left, right))?;
            if left != right {
                return answer(false, budget);
            }
        }
        if !map_equal(&self.nonces, &other.nonces, domain, budget)?
            || self.existence != other.existence
            || self.created != other.created
            || self.created_default != other.created_default
            || self.pending_destruction != other.pending_destruction
            || self.pending_destruction_default != other.pending_destruction_default
            || self.touched != other.touched
            || self.touched_default != other.touched_default
            || self.possible_logs.len() != other.possible_logs.len()
        {
            return answer(false, budget);
        }
        for ((left_key, left), (right_key, right)) in
            self.possible_logs.iter().zip(other.possible_logs.iter())
        {
            if left_key != right_key || left.topics.len() != right.topics.len() {
                return answer(false, budget);
            }
            pay_structure(left.topics.len().saturating_add(right.topics.len()), budget)?;
            for (left, right) in left.topics.iter().zip(&right.topics) {
                if !scalar::equal(left, right, domain, budget)? {
                    return answer(false, budget);
                }
            }
            if !left.data.eq_with_payment(&right.data, domain, budget)? {
                return answer(false, budget);
            }
        }
        answer(self.logs_unknown == other.logs_unknown, budget)
    }
}
