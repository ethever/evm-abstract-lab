//! Demand-local guard choices select only their corresponding operand facts.

use super::{ConditionalFacts, Facts};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    relational::intersect_numeric,
    symbolic::{ExprError, ExprId},
};
use crate::resource::WorkBudget;
use alloy_primitives::U256;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

mod bindings;
mod memo;
pub(in crate::world::bytes) use bindings::BindingBudget;
pub(in crate::world::bytes) use memo::{CONTEXTS, Preparation, preparation};

/// Resolve the active metadata scopes once per guard environment. Scalar
/// lookups then inspect only their own key in those maps, never the fact DAG.
/// Clones share a complete-value memo; changed choices start a separate memo.
/// The shared demand owns no snapshots and dies before its borrowed facts.
pub(in crate::world::bytes) struct Context<
    'a,
    N: WordElement = crate::domain::NumericValue,
    D: EvmNumericAlgebra<Element = N> = crate::domain::Domain,
> {
    domain: AlgebraRef<'a, D>,
    facts: &'a Facts<N>,
    conditional: Option<&'a ConditionalFacts<N>>,
    choices: Rc<BTreeMap<ExprId, bool>>,
    active: Rc<Vec<&'a Facts<N>>>,
    memo: Rc<RefCell<memo::Memo<'a, N, D>>>,
    demand: Option<Rc<memo::Demand<'a, D>>>,
    binding_budget: Option<&'a BindingBudget<'a>>,
}

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Context<'a, N, D> {
    pub(in crate::world::bytes) fn new(
        facts: &'a Facts<N>,
        conditional: Option<&'a ConditionalFacts<N>>,
        domain: AlgebraRef<'a, D>,
    ) -> Self {
        let mut out = Self {
            domain,
            facts,
            conditional,
            choices: Rc::new(BTreeMap::new()),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::<N, D>::default())),
            demand: None,
            binding_budget: None,
        };
        out.select();
        out
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn algebra(&self) -> AlgebraRef<'a, D> {
        self.domain
    }

    pub(in crate::world::bytes) fn prepaid(
        facts: &'a Facts<N>,
        conditional: Option<&'a ConditionalFacts<N>>,
        domain: AlgebraRef<'a, D>,
        binding_budget: &'a BindingBudget<'a>,
    ) -> Self {
        let mut out = Self::new(facts, conditional, domain);
        out.binding_budget = Some(binding_budget);
        out
    }

    /// Retention follows the expression association, never equality of exports.
    pub(in crate::world::bytes) fn has_binding_value(&self, expression: &ExprId) -> bool {
        self.fact(self.facts, expression).is_some()
            || self
                .active
                .iter()
                .any(|facts| self.fact(facts, expression).is_some())
    }

    pub(in crate::world::bytes) fn pay_binding(
        &self,
        numeric: &N,
        observation: &crate::domain::WordObservation,
    ) -> bool {
        if self.stopped() {
            return false;
        }
        let work = numeric.import_binding_work(observation);
        // Zero is the old raw NumericValue materialization already funded by
        // the caller. Do not add a payment/cancellation receipt for that case.
        if work == 0 {
            return true;
        }
        if let Some(demand) = &self.demand {
            demand.pay_reader(|| work)
        } else {
            self.binding_budget.is_none_or(|budget| budget.pay(work))
        }
    }

    /// Metadata and numeric actions share the original ledger. Fixed setup is
    /// funded before constructing its first Rc; refusal creates no Context.
    /// Facts and budget share only this trial's lifetime.
    pub(in crate::world::bytes) fn budgeted(
        facts: &'a Facts<N>,
        conditional: &'a ConditionalFacts<N>,
        domain: AlgebraRef<'a, D>,
        preparation: &Preparation,
        budget: &'a mut WorkBudget,
    ) -> Result<Option<Self>, ExprError> {
        let demand =
            memo::Demand::<D>::new(domain, preparation, conditional.largest_expression, budget);
        if !demand.pay_metadata(memo::DEMAND_LIFETIME_WORK + memo::CONTEXT_LIFETIME_WORK) {
            demand.check()?;
            return Ok(None);
        }
        Ok(Some(Self::with_demand(facts, conditional, Rc::new(demand))))
    }

    /// Compatibility oracle for existing private reference probes. Production
    /// prepared reads can only use budgeted, which installs the original payer.
    #[cfg(test)]
    pub(in crate::world::bytes) fn prepared(
        facts: &'a Facts<N>,
        conditional: &'a ConditionalFacts<N>,
        domain: AlgebraRef<'a, D>,
        nodes: usize,
    ) -> Self {
        Self::with_demand(
            facts,
            conditional,
            Rc::new(memo::Demand::<D>::oracle(
                domain,
                nodes,
                conditional.largest_expression,
            )),
        )
    }

    fn with_demand(
        facts: &'a Facts<N>,
        conditional: &'a ConditionalFacts<N>,
        demand: Rc<memo::Demand<'a, D>>,
    ) -> Self {
        let mut out = Self {
            domain: demand.domain,
            facts,
            conditional: Some(conditional),
            choices: Rc::new(BTreeMap::new()),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::<N, D>::default())),
            demand: Some(demand),
            binding_budget: None,
        };
        // Immutable source keys are sized once, before any prepared lookups.
        // All fixed scaffolding above was funded before its first allocation.
        let demand = out.demand.as_deref().expect("prepared context");
        if demand.reserve(facts.len().saturating_add(1)) {
            for key in facts.keys() {
                demand.source_key(key.work_size());
            }
            out.select();
        }
        out
    }

    pub(in crate::world::bytes) fn check(&self) -> Result<(), ExprError> {
        if let Some(budget) = self.binding_budget {
            budget.check()?;
        }
        if let Some(demand) = &self.demand {
            // Latch cancellation in the shared trial before its dynamic scope
            // can end; every existing Context/Reader clone then stays stopped.
            demand.check()
        } else if analysis_control::current_cancellation().is_cancelled() {
            Err(ExprError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// A private cache refusal is separate from source validation/cancellation.
    pub(in crate::world::bytes) fn cache_refused(&self) -> bool {
        self.demand.as_ref().is_some_and(|demand| demand.refused())
    }

    /// Work is mandatory and must never enter the optional cache fallback.
    pub(in crate::world::bytes) fn work_exhausted(&self) -> bool {
        self.demand
            .as_ref()
            .is_some_and(|demand| demand.work_exhausted())
            || self.binding_budget.is_some_and(BindingBudget::exhausted)
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn demand_usage(&self) -> Option<(usize, usize)> {
        self.demand
            .as_ref()
            .map(|demand| (demand.retained_contexts(), demand.used()))
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn demand_payments(&self) -> Vec<(bool, usize)> {
        self.demand.as_deref().expect("prepared context").payments()
    }

    pub(in crate::world::bytes) fn stopped(&self) -> bool {
        self.check().is_err() || self.cache_refused() || self.work_exhausted()
    }

    /// Pay each actual Reader transfer on the original demand ledger. The
    /// payment borrow ends before Domain runs; None/oracle keep their old path.
    /// A refused action yields only the checked demand's private stop sentinel.
    pub(in crate::world::bytes) fn apply(
        &self,
        domain: AlgebraRef<'_, D>,
        op: u8,
        args: &[EvmValue<N>],
    ) -> EvmValue<N> {
        if !self.pay_apply(domain, args) {
            return EvmValue::<N>::top();
        }
        domain.apply(op, args)
    }

    /// Byte observations keep the same complete policy guard, original quote
    /// and stop/call ordering as ordinary Reader transfers. No reduced fee or
    /// alternate payer is introduced when the domain omits optional reduction.
    pub(in crate::world::bytes) fn observe_byte(
        &self,
        domain: AlgebraRef<'_, D>,
        op: u8,
        args: &[EvmValue<N>],
    ) -> EvmValue<N> {
        if !self.pay_apply(domain, args) {
            return EvmValue::<N>::top();
        }
        domain.observe_byte(op, args)
    }

    fn pay_apply(&self, domain: AlgebraRef<'_, D>, args: &[EvmValue<N>]) -> bool {
        if self.stopped()
            || self.demand.as_ref().is_some_and(|demand| {
                !demand.policy(domain) || !demand.pay_reader(|| domain.operation_work(args))
            })
        {
            return false;
        }
        #[cfg(test)]
        if let Some(demand) = &self.demand {
            demand.record_reader_call(false);
        }
        true
    }

    /// Prepay the fixed byte-image mask proof and inline numeric replacement.
    /// The fixed 32*256 envelope covers masks/bounds, 32 stack-byte probes and
    /// construction/release of Top's six inline words; no AV payload is copied.
    /// The owned scalar moves its metadata; the ordinary apply pays its own
    /// transfer. None is a pure library path; prepaid analysis callers retain
    /// their old envelope, and the reference oracle owes no Reader fees.
    pub(in crate::world::bytes) fn pay_byte_image(&self, domain: AlgebraRef<'_, D>) -> bool {
        !self.stopped()
            && self
                .demand
                .as_ref()
                .is_none_or(|demand| demand.policy(domain) && demand.pay_reader(|| 32 * 256))
    }

    /// This join is separate from resolve's already-paid source metadata join.
    pub(in crate::world::bytes) fn join_with(
        &self,
        domain: AlgebraRef<'_, D>,
        left: &EvmValue<N>,
        right: &EvmValue<N>,
    ) -> EvmValue<N> {
        if self.stopped()
            || self.demand.as_ref().is_some_and(|demand| {
                !demand.policy(domain)
                    || !demand.pay_reader(|| memo::join_work(domain, left, right))
            })
        {
            return EvmValue::<N>::top();
        }
        #[cfg(test)]
        if let Some(demand) = &self.demand {
            demand.record_reader_call(true);
        }
        domain.join(left, right)
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn reader_calls(&self) -> (usize, usize) {
        self.demand
            .as_deref()
            .expect("prepared context")
            .reader_calls()
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn reader_join_work(
        domain: AlgebraRef<'_, D>,
        left: &EvmValue<N>,
        right: &EvmValue<N>,
    ) -> usize {
        memo::join_work(domain, left, right)
    }

    #[cfg(test)]
    pub(in crate::world::bytes) fn cancel_after_numeric_payments(
        &self,
        count: usize,
        token: analysis_control::Cancellation,
    ) {
        self.demand
            .as_deref()
            .expect("prepared context")
            .cancel_after_payments(count, token);
    }

    fn reserve(&self, nodes: usize) -> bool {
        !self.stopped()
            && self
                .demand
                .as_ref()
                .is_none_or(|demand| demand.reserve(nodes))
    }

    fn reserve_owned(&self, nodes: usize, release: usize) -> bool {
        !self.stopped()
            && self
                .demand
                .as_ref()
                .is_none_or(|demand| demand.reserve_owned(nodes, release, 0))
    }

    fn fact<'b>(&self, facts: &'b Facts<N>, expression: &ExprId) -> Option<&'b EvmValue<N>> {
        if self.demand.is_none() {
            return facts.get(expression);
        }
        memo::probe(
            facts.len(),
            expression,
            self.demand.as_deref().expect("prepared lookup"),
            &|nodes| self.reserve(nodes),
        )
        .then(|| facts.get(expression))
        .flatten()
    }

    fn choice(&self, expression: &ExprId) -> Option<bool> {
        if self.demand.is_none() {
            return self.choices.get(expression).copied();
        }
        memo::probe(
            self.choices.len(),
            expression,
            self.demand.as_deref().expect("prepared choice"),
            &|nodes| self.reserve(nodes),
        )
        .then(|| self.choices.get(expression).copied())
        .flatten()
    }

    fn select(&mut self) {
        fn selected<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
            conditional: &'a ConditionalFacts<N>,
            context: &Context<'_, N, D>,
            active: &mut Vec<&'a Facts<N>>,
        ) {
            for branch in &conditional.branches {
                if !context.reserve(1) {
                    return;
                }
                let truth = context.choice(&branch.condition).or_else(|| {
                    branch
                        .condition
                        .as_constant()
                        .map(|n| n != U256::ZERO)
                        .or_else(|| {
                            active
                                .iter()
                                .rev()
                                .filter_map(|facts| context.fact(facts, &branch.condition))
                                .chain(context.fact(context.facts, &branch.condition))
                                .find_map(|value| {
                                    if !context.reserve(value.numeric().work_size()) {
                                        return None;
                                    }
                                    if !value.may_be_zero() {
                                        Some(true)
                                    } else if !value.may_be_nonzero() {
                                        Some(false)
                                    } else {
                                        None
                                    }
                                })
                        })
                });
                let Some(nonzero) = truth else { continue };
                let scope = if nonzero {
                    &branch.then_facts
                } else {
                    &branch.else_facts
                };
                if !scope.facts.is_empty() {
                    let growth = if active.len() == active.capacity() {
                        active.len()
                    } else {
                        0
                    };
                    if !context.reserve_owned(growth.saturating_add(1), 1) {
                        return;
                    }
                    active.push(&scope.facts);
                }
                selected(&scope.conditional, context, active);
            }
        }
        let mut active = Vec::new();
        if let Some(conditional) = self.conditional {
            selected(conditional, self, &mut active);
        }
        if !self.stopped() {
            self.active = Rc::new(active);
        }
    }

    pub(in crate::world::bytes) fn branch(&self, condition: &ExprId, nonzero: bool) -> Self {
        if !self.reserve(1) || self.choice(condition) == Some(nonzero) {
            return self.clone();
        }
        let key = (condition.immutable_node_key(), nonzero);
        if self.demand.is_some() {
            if !self.reserve(super::CONTEXTS) {
                return self.clone();
            }
            if let Some((_, old)) = self.memo.borrow().branches.get(&key) {
                return old.clone();
            }
        }
        if !self.reserve(self.choices.len().saturating_add(1)) {
            return self.clone();
        }
        let mut nodes = condition.work_size();
        for key in self.choices.keys() {
            if !self.reserve(1) {
                return self.clone();
            }
            nodes = nodes.saturating_add(key.work_size());
        }
        if let Some(demand) = &self.demand
            && !demand.context(nodes, self.choices.len().saturating_add(1))
        {
            return self.clone();
        }
        if !self.reserve_owned(
            nodes.saturating_add(super::CONTEXTS),
            nodes
                .saturating_add(memo::CONTEXT_LIFETIME_WORK)
                .saturating_add(1),
        ) {
            return self.clone();
        }
        if let Some(demand) = &self.demand {
            demand.include_key(condition.work_size());
        }
        let mut choices = (*self.choices).clone();
        choices.insert(condition.clone(), nonzero);
        let mut out = Self {
            domain: self.domain,
            facts: self.facts,
            conditional: self.conditional,
            choices: Rc::new(choices),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::<N, D>::default())),
            demand: self.demand.as_ref().map(Rc::clone),
            binding_budget: self.binding_budget,
        };
        out.select();
        // The unchanged construction envelope above already funded this
        // complete child's shells, choices and release, even when not retained.
        // Cache capacity skips only retention; N/node/depth and Work still bound
        // every temporary child's construction and all of its later actions.
        if let Some(demand) = &self.demand
            && !self.stopped()
            && demand.can_retain_context()
            && self
                .memo
                .borrow_mut()
                .branches
                .insert(key, (condition.clone(), out.clone()))
                .is_none()
        {
            demand.record_retained_context();
        }
        out
    }

    pub(in crate::world::bytes) fn value(
        &self,
        expression: &ExprId,
        domain: AlgebraRef<'_, D>,
    ) -> Option<EvmValue<N>> {
        if !self.reserve(1)
            || self
                .demand
                .as_ref()
                .is_some_and(|demand| !demand.policy(domain))
        {
            return None;
        }
        let Some(demand) = &self.demand else {
            return self.resolve(expression, domain);
        };
        if let Some(value) = self
            .memo
            .borrow()
            .get_with(expression, domain, demand, &|nodes| self.reserve(nodes))
        {
            return value;
        }
        if self.stopped() {
            return None;
        }
        let out = self.resolve(expression, domain);
        // An interrupted callback must never install a completed negative entry.
        if !self.stopped() {
            self.memo
                .borrow_mut()
                .insert(expression, &out, domain, demand, &|nodes| {
                    self.reserve(nodes)
                });
        }
        (!self.stopped()).then_some(out).flatten()
    }

    fn resolve(&self, expression: &ExprId, domain: AlgebraRef<'_, D>) -> Option<EvmValue<N>> {
        let mut out = match self.fact(self.facts, expression) {
            Some(value) if self.reserve(value.work_size()) => Some(value.clone()),
            _ => None,
        };
        for facts in self.active.iter() {
            if !self.reserve(1) {
                return None;
            }
            let Some(value) = self.fact(facts, expression) else {
                continue;
            };
            if !self.reserve(
                value
                    .work_size()
                    .saturating_mul(2)
                    .saturating_add(
                        out.as_ref()
                            .map_or(0, EvmValue::<N>::work_size)
                            .saturating_mul(3),
                    )
                    .saturating_add(domain.capacity())
                    .saturating_add(32),
            ) {
                return None;
            }
            out = Some(match out {
                Some(old) => {
                    let numeric = if old.numeric() == value.numeric() || value.numeric().is_top() {
                        old.numeric().clone()
                    } else if old.numeric().is_top() {
                        value.numeric().clone()
                    } else {
                        if self
                            .demand
                            .as_ref()
                            .is_some_and(|demand| !demand.intersect())
                        {
                            return None;
                        }
                        // A contradictory branch is unreachable. Keeping its
                        // previous upper bound cannot remove a concrete state.
                        intersect_numeric(old.numeric(), value.numeric(), domain)
                            .unwrap_or_else(|| old.numeric().clone())
                    };
                    if self
                        .demand
                        .as_ref()
                        .is_some_and(|demand| !demand.pay(memo::join_work(domain, &old, value)))
                    {
                        return None;
                    }
                    domain.join(&old, value).with_numeric(numeric)
                }
                None => value.clone(),
            });
        }
        if let Some(nonzero) = self.choice(expression) {
            if !self.reserve(
                32usize.saturating_add(
                    out.as_ref()
                        .map_or(0, EvmValue::<N>::work_size)
                        .saturating_mul(2),
                ),
            ) {
                return None;
            }
            let truth = if nonzero {
                EvmValue::<N>::unsigned_range(U256::ONE, U256::MAX).expect("nonempty range")
            } else {
                EvmValue::<N>::constant(U256::ZERO)
            };
            out = Some(match out {
                Some(value) => {
                    if self
                        .demand
                        .as_ref()
                        .is_some_and(|demand| !demand.intersect())
                    {
                        return None;
                    }
                    let numeric = intersect_numeric(value.numeric(), truth.numeric(), domain)
                        .unwrap_or_else(|| value.numeric().clone());
                    value.with_numeric(numeric)
                }
                None => truth,
            });
        }
        out
    }

    pub(in crate::world::bytes) fn write_lower_bound(
        &self,
        destination: &ExprId,
        domain: AlgebraRef<'_, D>,
    ) -> Option<U256> {
        self.value(destination, domain)
            .map(|value| crate::world::unsigned_bounds(&value).0)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod payer_tests;

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Clone for Context<'a, N, D> {
    fn clone(&self) -> Self {
        Self {
            domain: self.domain,
            facts: self.facts,
            conditional: self.conditional,
            choices: Rc::clone(&self.choices),
            active: Rc::clone(&self.active),
            memo: Rc::clone(&self.memo),
            demand: self.demand.clone(),
            binding_budget: self.binding_budget,
        }
    }
}
