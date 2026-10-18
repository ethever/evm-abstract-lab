//! Demand-local guard choices select only their corresponding operand facts.

use super::{ConditionalFacts, Facts};
use crate::domain::{
    AbstractValue, Domain,
    relational::intersect_numeric,
    symbolic::{ExprError, ExprId},
};
use crate::resource::WorkBudget;
use alloy_primitives::U256;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

mod memo;
pub(in crate::world::bytes) use memo::{CONTEXTS, Preparation, preparation};

/// Resolve the active metadata scopes once per guard environment. Scalar
/// lookups then inspect only their own key in those maps, never the fact DAG.
/// Clones share a complete-value memo; changed choices start a separate memo.
/// The shared demand owns no snapshots and dies before its borrowed facts.
#[derive(Clone)]
pub(in crate::world::bytes) struct Context<'a> {
    facts: &'a Facts,
    conditional: Option<&'a ConditionalFacts>,
    choices: Rc<BTreeMap<ExprId, bool>>,
    active: Rc<Vec<&'a Facts>>,
    memo: Rc<RefCell<memo::Memo<'a>>>,
    demand: Option<Rc<memo::Demand<'a>>>,
}

impl<'a> Context<'a> {
    pub(in crate::world::bytes) fn new(
        facts: &'a Facts,
        conditional: Option<&'a ConditionalFacts>,
    ) -> Self {
        let mut out = Self {
            facts,
            conditional,
            choices: Rc::new(BTreeMap::new()),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::default())),
            demand: None,
        };
        out.select();
        out
    }

    /// Metadata and numeric actions share the original ledger. Fixed setup is
    /// funded before constructing its first Rc; refusal creates no Context.
    /// Facts and budget share only this trial's lifetime.
    pub(in crate::world::bytes) fn budgeted(
        facts: &'a Facts,
        conditional: &'a ConditionalFacts,
        domain: Domain,
        preparation: &Preparation,
        budget: &'a mut WorkBudget,
    ) -> Result<Option<Self>, ExprError> {
        let demand = memo::Demand::new(domain, preparation, conditional.largest_expression, budget);
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
        facts: &'a Facts,
        conditional: &'a ConditionalFacts,
        domain: Domain,
        nodes: usize,
    ) -> Self {
        Self::with_demand(
            facts,
            conditional,
            Rc::new(memo::Demand::oracle(
                domain,
                nodes,
                conditional.largest_expression,
            )),
        )
    }

    fn with_demand(
        facts: &'a Facts,
        conditional: &'a ConditionalFacts,
        demand: Rc<memo::Demand<'a>>,
    ) -> Self {
        let mut out = Self {
            facts,
            conditional: Some(conditional),
            choices: Rc::new(BTreeMap::new()),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::default())),
            demand: Some(demand),
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
        domain: Domain,
        op: u8,
        args: &[AbstractValue],
    ) -> AbstractValue {
        if self.stopped()
            || self.demand.as_ref().is_some_and(|demand| {
                !demand.policy(domain) || !demand.pay_reader(|| domain.operation_work(args))
            })
        {
            return AbstractValue::top();
        }
        #[cfg(test)]
        if let Some(demand) = &self.demand {
            demand.record_reader_call(false);
        }
        domain.apply(op, args)
    }

    /// This join is separate from resolve's already-paid source metadata join.
    pub(in crate::world::bytes) fn join(
        &self,
        domain: Domain,
        left: &AbstractValue,
        right: &AbstractValue,
    ) -> AbstractValue {
        if self.stopped()
            || self.demand.as_ref().is_some_and(|demand| {
                !demand.policy(domain)
                    || !demand.pay_reader(|| memo::join_work(domain, left, right))
            })
        {
            return AbstractValue::top();
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
        domain: Domain,
        left: &AbstractValue,
        right: &AbstractValue,
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

    fn fact<'b>(&self, facts: &'b Facts, expression: &ExprId) -> Option<&'b AbstractValue> {
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
        fn selected<'a>(
            conditional: &'a ConditionalFacts,
            context: &Context<'_>,
            active: &mut Vec<&'a Facts>,
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
            facts: self.facts,
            conditional: self.conditional,
            choices: Rc::new(choices),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::default())),
            demand: self.demand.as_ref().map(Rc::clone),
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
        domain: Domain,
    ) -> Option<AbstractValue> {
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
            .get(expression, domain, demand, &|nodes| self.reserve(nodes))
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

    fn resolve(&self, expression: &ExprId, domain: Domain) -> Option<AbstractValue> {
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
                            .map_or(0, AbstractValue::work_size)
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
                        .map_or(0, AbstractValue::work_size)
                        .saturating_mul(2),
                ),
            ) {
                return None;
            }
            let truth = if nonzero {
                AbstractValue::unsigned_range(U256::ONE, U256::MAX).expect("nonempty range")
            } else {
                AbstractValue::constant(U256::ZERO)
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
        domain: Domain,
    ) -> Option<U256> {
        self.value(destination, domain)
            .map(|value| value.interval().unsigned_bounds().0)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod payer_tests;
