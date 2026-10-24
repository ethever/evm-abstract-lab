//! One fallible native payer shared by the existing read evaluators.
//!
//! Pure entry points select `unpaid`; paid entry points never manufacture a
//! carrier sentinel on refusal. Each later hook quotes its actual input/result.

use super::{Context, EvmNumericAlgebra, EvmValue, WordElement};
use crate::{
    domain::{AlgebraRef, SummaryConstruction, WordObservation, summary},
    resource::{
        CloneError, SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
    world::{Candidates, RangeError},
};
use alloy_primitives::U256;

pub(in crate::world) type ReadResult<T> = Result<T, SummaryError<RangeError>>;

pub(in crate::world) struct ReadPayment<'b> {
    budget: Option<&'b mut WorkBudget>,
}

impl<'b> ReadPayment<'b> {
    pub(in crate::world) fn expression_lookup<'a>(
        &mut self,
        keys: impl Iterator<Item = &'a crate::domain::symbolic::ExprId>,
        key: &crate::domain::symbolic::ExprId,
    ) -> ReadResult<()> {
        if !self.is_paid() {
            return Ok(());
        }
        self.reserve(1)?;
        for stored in keys {
            self.reserve(stored.comparison_work(key).saturating_add(1))?;
        }
        Ok(())
    }
    pub(in crate::world) fn expression_equal(
        &mut self,
        left: &crate::domain::symbolic::ExprId,
        right: &crate::domain::symbolic::ExprId,
    ) -> ReadResult<bool> {
        self.reserve(left.comparison_work(right))?;
        Ok(left == right)
    }
    pub(in crate::world) fn equal<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        left: &EvmValue<N>,
        right: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<bool> {
        match &mut self.budget {
            Some(budget) => Ok(summary::equal(left, right, domain, budget)?),
            None => Ok(left == right),
        }
    }
    pub(super) fn try_equal<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        left: &EvmValue<N>,
        right: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<Option<bool>> {
        self.check()?;
        if !self.is_paid() {
            return Ok(Some(left == right));
        }
        let work = summary::equality_work(left, right, domain);
        self.check()?;
        if work == usize::MAX
            || self
                .budget
                .as_ref()
                .is_some_and(|budget| !budget.can_afford(work))
        {
            return Ok(None);
        }
        self.reserve(work)?;
        let equal = left == right;
        self.check()?;
        Ok(Some(equal))
    }
    pub(in crate::world) fn unpaid() -> Self {
        Self { budget: None }
    }
    pub(in crate::world) fn new(budget: &'b mut WorkBudget) -> Self {
        Self {
            budget: Some(budget),
        }
    }
    pub(in crate::world) fn is_paid(&self) -> bool {
        self.budget.is_some()
    }
    pub(in crate::world) fn check(&self) -> ReadResult<()> {
        if let Some(budget) = &self.budget {
            checkpoint(budget)?;
        }
        Ok(())
    }
    pub(in crate::world) fn context<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &self,
        context: &Context<'_, N, D>,
    ) -> ReadResult<()> {
        self.check()?;
        context
            .check()
            .map_err(|e| SummaryError::Source(e.into()))?;
        if context.stopped() {
            return Err(CloneError::Work.into());
        }
        Ok(())
    }
    pub(in crate::world) fn reserve(&mut self, work: usize) -> ReadResult<()> {
        if let Some(budget) = &mut self.budget {
            pay(budget, work)?;
        }
        Ok(())
    }
    pub(in crate::world) fn copy<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
    ) -> ReadResult<EvmValue<N>> {
        match &mut self.budget {
            Some(budget) => Ok(summary::clone_value(value, budget)?),
            None => Ok(value.clone()),
        }
    }
    /// An optional cache cannot exhaust the ledger merely to retain an already
    /// funded result. Inspect the actual copy quote before choosing retention.
    pub(super) fn try_cache_copy<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        value: &EvmValue<N>,
        structure: usize,
        context: &Context<'_, N, D>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        self.context(context)?;
        let copy = value.clone_work();
        self.context(context)?;
        let Some(work) = structure
            .checked_add(copy)
            .filter(|work| *work != usize::MAX)
        else {
            return Ok(None);
        };
        if self
            .budget
            .as_ref()
            .is_some_and(|budget| !budget.can_afford(work))
        {
            return Ok(None);
        }
        self.reserve(work)?;
        let value = value.clone();
        self.context(context)?;
        Ok(Some(value))
    }
    pub(in crate::world) fn copy_numeric<N: WordElement>(&mut self, value: &N) -> ReadResult<N> {
        self.check()?;
        if self.is_paid() {
            self.reserve(value.clone_work())?;
        }
        let out = value.clone();
        self.check()?;
        Ok(out)
    }
    pub(in crate::world) fn singleton<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
    ) -> ReadResult<Option<U256>> {
        match &mut self.budget {
            Some(budget) => Ok(summary::singleton(value, budget)?),
            None => Ok(value.singleton()),
        }
    }
    pub(in crate::world) fn bounds<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
    ) -> ReadResult<(U256, U256)> {
        match &mut self.budget {
            Some(budget) => Ok(summary::unsigned_bounds(value, budget)?),
            None => Ok(crate::world::unsigned_bounds(value)),
        }
    }
    pub(in crate::world) fn candidates<'a, N: WordElement>(
        &mut self,
        value: &'a EvmValue<N>,
    ) -> ReadResult<Option<Candidates<'a>>> {
        if !self.is_paid() {
            return Ok(crate::world::candidates(value));
        }
        self.check()?;
        self.reserve(value.numeric().work_size())?;
        let finite = value.constants();
        self.check()?;
        let singleton = if finite.is_none() {
            self.singleton(value)?
        } else {
            None
        };
        Ok((finite.is_some() || singleton.is_some()).then_some(Candidates { finite, singleton }))
    }
    pub(in crate::world) fn contains<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
        word: U256,
    ) -> ReadResult<bool> {
        // WordElement::work_size explicitly bounds one membership query.
        if self.is_paid() {
            self.reserve(value.numeric().work_size())?;
        }
        let answer = value.contains(word);
        self.check()?;
        Ok(answer)
    }
    pub(in crate::world) fn may_zero<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
    ) -> ReadResult<bool> {
        if self.is_paid() {
            return self.contains(value, U256::ZERO);
        }
        let answer = value.may_be_zero();
        self.check()?;
        Ok(answer)
    }
    pub(in crate::world) fn may_nonzero<N: WordElement>(
        &mut self,
        value: &EvmValue<N>,
    ) -> ReadResult<bool> {
        if self.is_paid() {
            return Ok(self.singleton(value)? != Some(U256::ZERO));
        }
        let answer = value.may_be_nonzero();
        self.check()?;
        Ok(answer)
    }
    pub(in crate::world) fn literal<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        word: U256,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        let numeric = self.literal_numeric(word, domain)?;
        let mut out = EvmValue::from_numeric(numeric);
        out.set_origin(crate::domain::provenance::Origin::Constant);
        Ok(out)
    }
    pub(in crate::world) fn literal_numeric<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        word: U256,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<N> {
        self.check()?;
        if self.is_paid() {
            self.reserve(domain.literal_quote_work(word).saturating_add(16))?;
        }
        let out = N::constant(word);
        self.check()?;
        Ok(out)
    }
    pub(in crate::world) fn top<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        match &mut self.budget {
            Some(budget) => Ok(summary::top(domain, budget)?),
            None => Ok(EvmValue::top()),
        }
    }
    pub(in crate::world) fn unknown_byte<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        match &mut self.budget {
            Some(budget) => Ok(summary::unknown_byte(domain, budget)?),
            None => Ok(EvmValue::unknown_byte()),
        }
    }
    pub(in crate::world) fn range<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        lower: U256,
        upper: U256,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        match &mut self.budget {
            Some(budget) => Ok(summary::unsigned_range(lower, upper, domain, budget)?),
            None => Ok(EvmValue::unsigned_range(lower, upper).expect("ordered read range")),
        }
    }
    pub(in crate::world) fn is_top<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<bool> {
        match &mut self.budget {
            Some(budget) => Ok(summary::is_top(value, domain, budget)?),
            None => Ok(value.numeric().is_top()),
        }
    }
    pub(in crate::world) fn equal_numeric<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        left: &N,
        right: &N,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<bool> {
        if self.is_paid() {
            self.reserve(domain.numeric_algebra().summary_equal_work(left, right))?;
        }
        let equal = left == right;
        self.check()?;
        Ok(equal)
    }
    pub(in crate::world) fn join<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        left: &EvmValue<N>,
        right: &EvmValue<N>,
        context: &Context<'_, N, D>,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        self.context(context)?;
        let out = match &mut self.budget {
            Some(budget) => summary::join(left, right, domain, false, budget)?,
            None => context.join(domain, left, right),
        };
        self.context(context)?;
        Ok(out)
    }
    pub(in crate::world) fn apply<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        op: u8,
        args: &[EvmValue<N>],
        context: &Context<'_, N, D>,
        domain: AlgebraRef<'_, D>,
        observe: bool,
    ) -> ReadResult<EvmValue<N>> {
        self.context(context)?;
        let out = if self.is_paid() {
            self.reserve(domain.operation_work(op, args))?;
            if observe {
                domain.observe_byte(op, args)
            } else {
                domain.apply(op, args)
            }
        } else if observe {
            context.observe_byte(domain, op, args)
        } else {
            context.apply(domain, op, args)
        };
        self.context(context)?;
        Ok(out)
    }
    pub(in crate::world) fn intersect<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        left: &N,
        right: &N,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<Option<N>> {
        let out = if self.is_paid() {
            domain.intersect_numeric_with_payment(left, right, 0, |work| self.reserve(work))?
        } else {
            crate::domain::relational::intersect_numeric(left, right, domain)
        };
        self.check()?;
        Ok(out)
    }
    pub(in crate::world) fn bind<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        value: EvmValue<N>,
        retained: bool,
        observation: &WordObservation,
        context: &Context<'_, N, D>,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<EvmValue<N>> {
        self.context(context)?;
        let numeric = if retained {
            if self.is_paid() {
                self.reserve(value.numeric().import_binding_work(observation))?;
            } else if !context.pay_binding(value.numeric(), observation) {
                return Err(CloneError::Work.into());
            }
            value.numeric().import_binding(observation)
        } else {
            if self.is_paid() {
                self.reserve(
                    domain
                        .numeric_algebra()
                        .summary_observation_work(observation),
                )?;
            }
            N::from_observation(observation)
        };
        self.context(context)?;
        Ok(value.with_numeric(numeric))
    }
    pub(in crate::world) fn top_numeric<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
    ) -> ReadResult<N> {
        if self.is_paid() {
            self.reserve(
                domain
                    .numeric_algebra()
                    .summary_construction_work(SummaryConstruction::Top),
            )?;
        }
        let out = N::top();
        self.check()?;
        Ok(out)
    }
}
