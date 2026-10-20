//! Cycles widen retired engine-generated choices into a bounded set of paired
//! whole payloads. Source/GAS/manual identities are never enumerated. This
//! explicit widening forgets old path guarantees and preserves old snapshots.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{MachineError, checkpoint};
use crate::{
    analysis::{machine::EvmMachinePayload, state::StatePart},
    domain::{
        AlgebraRef, EvmValue,
        symbolic::{ExprId, ExprLimits},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn reserve(budget: &mut WorkBudget, cost: usize) -> Result<(), MachineError> {
    checkpoint()?;
    if cost == usize::MAX || !budget.charge(cost) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

fn scalar<N: WordElement>(value: &mut EvmValue<N>) {
    value.forget_identity();
    if let Some(constant) = value.expression().and_then(ExprId::as_constant) {
        *value = value.clone().with_numeric(N::constant(constant));
    }
}

fn form<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    payload: &EvmMachinePayload<N>,
    domain: AlgebraRef<'_, D>,
    limits: ExprLimits,
) -> Result<EvmMachinePayload<N>, MachineError> {
    checkpoint()?;
    let mut result = payload.clone();
    result.relations.forget();
    result.store = result.store.widening_form_with(domain, limits)?;
    result.call_stack.transform(&mut |part| {
        match part {
            StatePart::Value(value) => scalar(value),
            StatePart::Bytes(bytes) => *bytes = bytes.widening_form_with(domain, limits)?,
            StatePart::Store(store) => {
                *store = Cow::Owned(store.widening_form_with(domain, limits)?)
            }
        }
        Ok(())
    })?;
    result.content_form()
}

pub(super) fn prepare<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    previous: &[Arc<EvmMachinePayload<N>>],
    initial: Option<&EvmMachinePayload<N>>,
    incoming: &EvmMachinePayload<N>,
    selectors: &BTreeSet<ExprId>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<Vec<Arc<EvmMachinePayload<N>>>, MachineError> {
    let policy = domain.spec().relations();
    let limit = policy.max_constraints;
    let limits = policy.expressions();
    // Stored canonical cases contain no engine selectors. Reusing their Arc
    // keeps their original inputs and avoids redoing a completed projection.
    reserve(budget, previous.len())?;
    let mut complete = previous.to_vec();
    for input in initial.into_iter().chain(std::iter::once(incoming)) {
        reserve(budget, input.work_size().saturating_mul(4))?;
        let mut initial = input.clone();
        initial.relations.forget();
        let mut pending = vec![initial];
        while let Some(payload) = pending.pop() {
            reserve(budget, payload.work_size().saturating_mul(8))?;
            // Remove conservative cache/must metadata before looking for active
            // choices; retired guards in operand caches cannot make a branch
            // tree grow after the immutable current roots no longer use them.
            let payload = form(&payload, domain, limits)?;
            let selector = payload
                .fresh_leaves()
                .into_iter()
                .find(|leaf| selectors.contains(leaf));
            if let Some(selector) = selector {
                // Pending complete candidates consume the retention policy too.
                if complete
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(2)
                    > limit
                {
                    return Err(MachineError::Alternatives { limit });
                }
                for value in [U256::ZERO, U256::from(1)] {
                    let mut branch = payload.clone();
                    branch.rename_fresh_with(
                        &BTreeMap::from([(selector.clone(), ExprId::constant(value))]),
                        limits,
                        domain,
                    )?;
                    pending.push(branch);
                }
            } else {
                let canonical = payload;
                let mut represented = false;
                for old in &complete {
                    reserve(
                        budget,
                        old.work_size().saturating_add(canonical.work_size()),
                    )?;
                    if old.as_ref() == &canonical {
                        represented = true;
                        break;
                    }
                }
                if !represented {
                    if complete.len() >= limit {
                        return Err(MachineError::Alternatives { limit });
                    }
                    reserve(budget, canonical.work_size())?;
                    complete.push(Arc::new(canonical));
                }
            }
        }
    }
    checkpoint()?;
    Ok(complete)
}
