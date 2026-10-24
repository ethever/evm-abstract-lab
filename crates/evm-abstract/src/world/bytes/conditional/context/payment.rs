//! Fallible read-side resolution over the same immutable facts and guards.

use super::{Context, Facts, memo};
use crate::{
    domain::{
        AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement,
        symbolic::{ExprId, ExprKind},
    },
    resource::WorkBudget,
    world::bytes::{
        conditional::ConditionalFacts,
        numeric::{
            payment::{ReadPayment, ReadResult},
            scalar_in_paid,
        },
    },
};
use alloy_primitives::U256;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Context<'a, N, D> {
    fn fact_paid<'b>(
        &self,
        facts: &'b Facts<N>,
        expression: &ExprId,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<&'b EvmValue<N>>> {
        payer.expression_lookup(facts.keys(), expression)?;
        Ok(facts.get(expression))
    }
    pub(in crate::world::bytes) fn has_binding_value_paid(
        &self,
        expression: &ExprId,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<bool> {
        if !payer.is_paid() {
            return Ok(self.has_binding_value(expression));
        }
        if self.fact_paid(self.facts, expression, payer)?.is_some() {
            return Ok(true);
        }
        for facts in self.active.iter() {
            if self.fact_paid(facts, expression, payer)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(in crate::world::bytes) fn write_lower_bound_paid(
        &self,
        destination: &ExprId,
        domain: AlgebraRef<'_, D>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<U256>> {
        if !payer.is_paid() {
            let value = self.write_lower_bound(destination, domain);
            payer.context(self)?;
            return Ok(value);
        }
        match self.value_paid(destination, domain, payer)? {
            Some(value) => Ok(Some(payer.bounds(&value)?.0)),
            None => Ok(None),
        }
    }
    pub(in crate::world) fn with_payment(
        facts: &'a Facts<N>,
        conditional: Option<&'a ConditionalFacts<N>>,
        domain: AlgebraRef<'a, D>,
        budget: &mut WorkBudget,
    ) -> ReadResult<Self> {
        Self::new_with_payment(facts, conditional, domain, budget)
    }
    pub(in crate::world) fn new_with_payment(
        facts: &'a Facts<N>,
        conditional: Option<&'a ConditionalFacts<N>>,
        domain: AlgebraRef<'a, D>,
        budget: &mut WorkBudget,
    ) -> ReadResult<Self> {
        let mut payer = ReadPayment::new(budget);
        payer.reserve(facts.len().saturating_add(32))?;
        let mut out = Self {
            domain,
            facts,
            conditional,
            choices: Rc::new(BTreeMap::new()),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::default())),
            demand: None,
            binding_budget: None,
            retained_words: conditional.is_some_and(|facts| facts.retained_words),
        };
        out.retained_words |= facts.iter().any(|(key, value)| {
            matches!(key.kind(), ExprKind::WordRead { .. })
                && value.numeric().requires_retained_binding()
        });
        out.select_paid(&mut payer)?;
        Ok(out)
    }

    fn select_paid(&mut self, payer: &mut ReadPayment<'_>) -> ReadResult<()> {
        fn selected<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
            conditional: &'a ConditionalFacts<N>,
            context: &Context<'a, N, D>,
            active: &mut Vec<&'a Facts<N>>,
            payer: &mut ReadPayment<'_>,
        ) -> ReadResult<()> {
            for branch in &conditional.branches {
                payer.reserve(branch.condition.work_size().saturating_add(1))?;
                payer.expression_lookup(context.choices.keys(), &branch.condition)?;
                let mut truth = context
                    .choices
                    .get(&branch.condition)
                    .copied()
                    .or_else(|| branch.condition.as_constant().map(|n| n != U256::ZERO));
                if truth.is_none() {
                    for facts in active
                        .iter()
                        .rev()
                        .copied()
                        .chain(std::iter::once(context.facts))
                    {
                        let Some(value) = context.fact_paid(facts, &branch.condition, payer)?
                        else {
                            continue;
                        };
                        if !payer.may_zero(value)? {
                            truth = Some(true);
                            break;
                        }
                        if !payer.may_nonzero(value)? {
                            truth = Some(false);
                            break;
                        }
                    }
                }
                let Some(nonzero) = truth else { continue };
                let scope = if nonzero {
                    &branch.then_facts
                } else {
                    &branch.else_facts
                };
                if !scope.facts.is_empty() {
                    payer.reserve(active.len().saturating_add(1))?;
                    active.push(&scope.facts);
                }
                selected(&scope.conditional, context, active, payer)?;
            }
            Ok(())
        }
        let mut active = Vec::new();
        if let Some(conditional) = self.conditional {
            selected(conditional, self, &mut active, payer)?;
        }
        payer.check()?;
        self.active = Rc::new(active);
        Ok(())
    }

    pub(in crate::world) fn branch_paid(
        &self,
        condition: &ExprId,
        nonzero: bool,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Self> {
        if !payer.is_paid() {
            return Ok(self.branch(condition, nonzero));
        }
        payer.context(self)?;
        payer.reserve(
            self.choices
                .len()
                .saturating_add(condition.work_size())
                .saturating_add(32),
        )?;
        payer.expression_lookup(self.choices.keys(), condition)?;
        if self.choices.get(condition) == Some(&nonzero) {
            return Ok(self.clone());
        }
        for key in self.choices.keys() {
            payer.reserve(key.work_size())?;
        }
        let mut choices = (*self.choices).clone();
        choices.insert(condition.clone(), nonzero);
        let mut out = Self {
            domain: self.domain,
            facts: self.facts,
            conditional: self.conditional,
            choices: Rc::new(choices),
            active: Rc::new(Vec::new()),
            memo: Rc::new(RefCell::new(memo::Memo::default())),
            demand: None,
            binding_budget: None,
            retained_words: self.retained_words,
        };
        out.select_paid(payer)?;
        Ok(out)
    }

    pub(in crate::world) fn branch_when_scoped_paid(
        &self,
        condition: &ExprId,
        nonzero: bool,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Self> {
        if self.conditional.is_none_or(|facts| facts.has_no_scopes()) {
            payer.context(self)?;
            Ok(self.clone())
        } else {
            self.branch_paid(condition, nonzero, payer)
        }
    }

    pub(in crate::world::bytes) fn value_paid(
        &self,
        expression: &ExprId,
        domain: AlgebraRef<'_, D>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        if !payer.is_paid() {
            let out = self.value(expression, domain);
            payer.context(self)?;
            return Ok(out);
        }
        payer.context(self)?;
        payer.reserve(
            expression
                .work_size()
                .saturating_add(self.facts.len())
                .saturating_add(1),
        )?;
        let mut out = self
            .fact_paid(self.facts, expression, payer)?
            .map(|value| payer.copy(value))
            .transpose()?;
        for facts in self.active.iter() {
            payer.reserve(
                expression
                    .work_size()
                    .saturating_add(facts.len())
                    .saturating_add(1),
            )?;
            let Some(value) = self.fact_paid(facts, expression, payer)? else {
                continue;
            };
            out = Some(match out {
                None => payer.copy(value)?,
                Some(old) => {
                    let numeric = if payer.equal_numeric(old.numeric(), value.numeric(), domain)?
                        || payer.is_top(value, domain)?
                    {
                        payer.copy_numeric(old.numeric())?
                    } else if payer.is_top(&old, domain)? {
                        payer.copy_numeric(value.numeric())?
                    } else {
                        match payer.intersect(old.numeric(), value.numeric(), domain)? {
                            Some(value) => value,
                            None => payer.copy_numeric(old.numeric())?,
                        }
                    };
                    payer.join(&old, value, self, domain)?.with_numeric(numeric)
                }
            });
        }
        payer.expression_lookup(self.choices.keys(), expression)?;
        if let Some(nonzero) = self.choices.get(expression).copied() {
            let truth = if nonzero {
                payer.range(U256::ONE, U256::MAX, domain)?
            } else {
                payer.literal(U256::ZERO, domain)?
            };
            out = Some(match out {
                None => truth,
                Some(value) => {
                    let numeric = match payer.intersect(value.numeric(), truth.numeric(), domain)? {
                        Some(n) => n,
                        None => payer.copy_numeric(value.numeric())?,
                    };
                    value.with_numeric(numeric)
                }
            });
        }
        payer.context(self)?;
        Ok(out)
    }

    pub(in crate::world::bytes) fn retained_word_paid(
        &self,
        expression: &ExprId,
        domain: AlgebraRef<'_, D>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        if !payer.is_paid() {
            let value = self.retained_word(expression, domain);
            payer.context(self)?;
            return Ok(value);
        }
        if !self.retained_words {
            return Ok(None);
        }
        if let ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } = expression.kind()
        {
            let selected = scalar_in_paid(condition, self, domain, payer)?;
            if !payer.may_zero(&selected)? {
                return self
                    .branch_paid(condition, true, payer)?
                    .retained_word_paid(then_value, domain, payer);
            }
            if !payer.may_nonzero(&selected)? {
                return self
                    .branch_paid(condition, false, payer)?
                    .retained_word_paid(else_value, domain, payer);
            }
            let left = self
                .branch_paid(condition, true, payer)?
                .retained_word_paid(then_value, domain, payer)?;
            let Some(left) = left else { return Ok(None) };
            let right = self
                .branch_paid(condition, false, payer)?
                .retained_word_paid(else_value, domain, payer)?;
            let Some(right) = right else { return Ok(None) };
            return Ok(Some(payer.join(&left, &right, self, domain)?));
        }
        if expression.as_constant().is_some() {
            return Ok(Some(scalar_in_paid(expression, self, domain, payer)?));
        }
        if !matches!(expression.kind(), ExprKind::WordRead { .. }) {
            return Ok(None);
        }
        payer.reserve(
            self.active
                .len()
                .saturating_add(1)
                .saturating_mul(expression.work_size().saturating_add(1)),
        )?;
        let mut retained = false;
        for facts in std::iter::once(self.facts).chain(self.active.iter().copied()) {
            if let Some(value) = self.fact_paid(facts, expression, payer)? {
                retained |= value.numeric().requires_retained_binding();
            }
        }
        if retained {
            self.value_paid(expression, domain, payer)
        } else {
            Ok(None)
        }
    }
}
