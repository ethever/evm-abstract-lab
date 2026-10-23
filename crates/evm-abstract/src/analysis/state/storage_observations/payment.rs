//! Paid native observations on one immutable replay snapshot.

use super::*;
use crate::{
    domain::summary,
    resource::{
        WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};

impl<N: WordElement> EvmStorageObservations<N> {
    pub(in crate::analysis) fn join_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        next: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        pay(
            budget,
            self.0.len().saturating_add(next.0.len()).saturating_add(1),
        )?;
        let mut observations = Vec::new();
        let mut right = next.0.iter().peekable();
        for left in &self.0 {
            let key = (left.owner, left.stack_position);
            while right
                .peek()
                .is_some_and(|r| (r.owner, r.stack_position) < key)
            {
                right.next();
            }
            let Some(other) = right.peek().filter(|r| (r.owner, r.stack_position) == key) else {
                continue;
            };
            pay(
                budget,
                left.numeric
                    .clone_work()
                    .saturating_add(other.numeric.clone_work())
                    .saturating_add(32),
            )?;
            let old = EvmValue::from_numeric(left.numeric.clone());
            let new = EvmValue::from_numeric(other.numeric.clone());
            let joined = summary::join(&old, &new, domain, widen, budget)?;
            if !summary::is_top(&joined, domain, budget)? {
                pay(budget, joined.numeric().clone_work())?;
                observations.push(Observation {
                    owner: left.owner,
                    stack_position: left.stack_position,
                    numeric: joined.numeric().clone(),
                });
                checkpoint(budget)?;
            }
        }
        Ok(Self(observations))
    }
    pub(in crate::analysis) fn clone_work_with(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<usize, MachineError> {
        pay(budget, self.0.len().saturating_add(1))?;
        let mut work = self.0.len().saturating_mul(8).saturating_add(1);
        for observation in &self.0 {
            checkpoint(budget)?;
            work = work.saturating_add(observation.numeric.clone_work());
        }
        Ok(crate::resource::clone_payment::finish_quote(work, budget)?)
    }
    pub(in crate::analysis) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, MachineError> {
        pay(
            budget,
            self.0.len().saturating_add(other.0.len()).saturating_add(1),
        )?;
        if self.0.len() != other.0.len() {
            return Ok(false);
        }
        for (left, right) in self.0.iter().zip(&other.0) {
            if left.owner != right.owner || left.stack_position != right.stack_position {
                return Ok(false);
            }
            pay(
                budget,
                domain
                    .numeric_algebra()
                    .summary_equal_work(&left.numeric, &right.numeric),
            )?;
            let equal = left.numeric == right.numeric;
            checkpoint(budget)?;
            if !equal {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn needs_install_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        index: usize,
        aliases: &StackAliases,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, MachineError> {
        let observation = &self.0[index];
        pay(budget, index.saturating_add(1))?;
        for earlier in &self.0[..index] {
            if earlier.owner != observation.owner
                || !aliases.same_class(earlier.stack_position, observation.stack_position)
            {
                continue;
            }
            pay(
                budget,
                domain
                    .numeric_algebra()
                    .summary_equal_work(&earlier.numeric, &observation.numeric),
            )?;
            let equal = earlier.numeric == observation.numeric;
            checkpoint(budget)?;
            if equal {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(in crate::analysis) fn install_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        payload: &mut EvmMachinePayload<N>,
        aliases: &StackAliases,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<(), MachineError> {
        pay(budget, self.0.len().saturating_add(1))?;
        for (index, observation) in self.0.iter().enumerate() {
            if !self.needs_install_with_payment(index, aliases, domain, budget)? {
                continue;
            }
            let frame = payload.active();
            if frame.key.address != observation.owner {
                return Err(MachineError::Shape);
            }
            let key = frame
                .stack
                .get(observation.stack_position)
                .ok_or(MachineError::Shape)?;
            let Some(key) = key_expression(key) else {
                continue;
            };
            pay(budget, read_work(MAX_ROOT_WORK, key.nodes()))?;
            let root = match payload.store.slot_root(observation.owner, false) {
                Ok(root) if root.nodes() <= MAX_ROOT_WORK => root,
                Ok(_) => continue,
                Err(error) => {
                    drop_read_error(error)?;
                    continue;
                }
            };
            let expression =
                match ExprId::slot_read(root, key, domain.spec().relations().expressions()) {
                    Ok(expression) => expression,
                    Err(error) => {
                        drop_read_error(error.into())?;
                        continue;
                    }
                };
            // WordElement's structural bound explicitly funds one observation
            // export. The subsequent fixed WordObservation import is separate;
            // it cannot discover an opaque native singleton or clone N.
            pay(budget, observation.numeric.work_size())?;
            let observed = observation.numeric.observation();
            checkpoint(budget)?;
            let work = payload
                .relations
                .work_size()
                .saturating_add(expression.nodes())
                .saturating_add(observed.work_size())
                .saturating_add(1)
                .saturating_mul(1024);
            pay(budget, work)?;
            let mut relations = payload.relations.clone();
            match relations.observe_observation(&expression, &observed, &domain.spec().relations())
            {
                Ok(()) => payload.relations = relations,
                Err(QueryReason::Cancelled) => {
                    return Err(MachineError::Relations(QueryReason::Cancelled));
                }
                Err(_) => {}
            }
            checkpoint(budget)?;
        }
        Ok(())
    }

    pub(in crate::analysis) fn form_with_payment<D: EvmNumericAlgebra<Element = N>>(
        payload: &EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        pay(budget, form_work(payload, domain))?;
        if !in_scope(payload, domain) {
            return Ok(Self::default());
        }
        let frame = payload.active();
        let root = match payload.store.slot_root(frame.key.address, false) {
            Ok(root) if root.nodes() <= MAX_ROOT_WORK => root,
            Ok(_) => return Ok(Self::default()),
            Err(error) => {
                drop_read_error(error)?;
                return Ok(Self::default());
            }
        };
        let use_facts = payload.relations.work_size() <= MAX_FACT_WORK;
        let mut observations = Vec::new();
        for (stack_position, key) in frame
            .stack
            .iter()
            .enumerate()
            .skip(frame.stack.len().saturating_sub(MAX_POSITIONS))
        {
            checkpoint(budget)?;
            if key_work(key).is_none() {
                continue;
            }
            let fact = latest_literal(&root, key)
                .map(ReadFact::Literal)
                .or_else(|| use_facts.then(|| read_fact(payload, &root, key)).flatten());
            let Some(fact) = fact else { continue };
            let numeric = match fact {
                ReadFact::Literal(value) => {
                    pay(budget, domain.literal_quote_work(value))?;
                    let numeric = N::constant(value);
                    checkpoint(budget)?;
                    numeric
                }
                ReadFact::Members(values) => {
                    let mut members = values.iter().copied();
                    let mut value = summary::constant(
                        members.next().expect("nonempty finite read fact"),
                        domain,
                        budget,
                    )?;
                    for member in members {
                        let next = summary::constant(member, domain, budget)?;
                        value = summary::join(&value, &next, domain, false, budget)?;
                    }
                    pay(budget, value.numeric().clone_work())?;
                    let numeric = value.numeric().clone();
                    checkpoint(budget)?;
                    numeric
                }
            };
            pay(
                budget,
                domain.numeric_algebra().summary_is_top_work(&numeric),
            )?;
            let top = numeric.is_top();
            checkpoint(budget)?;
            if !top {
                observations.push(Observation {
                    owner: frame.key.address,
                    stack_position,
                    numeric,
                });
            }
        }
        Ok(Self(observations))
    }
}
