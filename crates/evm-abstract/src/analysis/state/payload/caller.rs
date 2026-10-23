//! Complete caller operations preserve observations, aliases and native values.

use super::{
    AlgebraRef, EvmByteArray, EvmCallStack, EvmChildFrame, EvmContinuation, EvmEntry,
    EvmFrameSummary, EvmMachinePayload, EvmPayloadSummary, EvmRootFrame, EvmSnapshot, EvmValue,
    FrameByteKind, FrameCode, FrameMapOrder, MachineError, RelationState, StackAliases, identify,
};
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::{
    domain::{SummaryConstruction, summary},
    resource::{
        SummaryError, WorkBudget,
        clone_payment::{checkpoint, finish_quote, pay, value_work},
    },
    world::AddressInput,
};

#[cfg(test)]
mod tests;

impl<E> From<SummaryError<E>> for MachineError
where
    MachineError: From<E>,
{
    fn from(error: SummaryError<E>) -> Self {
        match error {
            SummaryError::Budget(error) => error.into(),
            SummaryError::Source(error) => error.into(),
        }
    }
}

fn scalar<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, MachineError> {
    checkpoint(budget)?;
    pay(budget, value.numeric().clone_work().saturating_add(16))?;
    let out = EvmValue::from_numeric(value.numeric().clone());
    checkpoint(budget)?;
    Ok(out)
}

fn implicit<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &Option<EvmValue<N>>,
    address: AddressInput,
    entry: &EvmEntry<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, MachineError> {
    if let Some(value) = value {
        return scalar(value, budget);
    }
    let fee = match address {
        AddressInput::Concrete(address) => {
            domain.literal_quote_work(alloy_primitives::U256::from_be_slice(address.as_slice()))
        }
        AddressInput::Symbolic(_) => domain
            .numeric_algebra()
            .summary_construction_work(SummaryConstruction::UnknownAddress),
    };
    pay(budget, fee.saturating_add(32))?;
    let value = entry.environment.address_value(address);
    checkpoint(budget)?;
    scalar(&value, budget)
}

impl<N: WordElement> EvmPayloadSummary<N> {
    pub(in crate::analysis) fn join_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        let mut result = self.join_bound_with_payment(incoming, domain, budget, widen)?;
        result.storage_observations = self.storage_observations.join_with_payment(
            &incoming.storage_observations,
            domain,
            widen,
            budget,
        )?;
        for ((frame, left), right) in result
            .frames
            .iter_mut()
            .zip(&self.frames)
            .zip(&incoming.frames)
        {
            pay(
                budget,
                left.stack_aliases
                    .work_size()
                    .saturating_add(right.stack_aliases.work_size()),
            )?;
            frame.stack_aliases = left.stack_aliases.intersect(&right.stack_aliases)?;
        }
        result.work = result.measure_work();
        checkpoint(budget)?;
        Ok(result)
    }
    /// Invoice exactly one derived Clone, including observations and aliases.
    /// Census work is paid here; callers (including kernel read_slot) pay the
    /// returned fee before copying. It does not fund queries on the copy.
    pub(in crate::analysis) fn clone_work_with(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<usize, MachineError> {
        pay(budget, self.work_size())?;
        self.check_shape()?;
        let mut work = self
            .work_size()
            .saturating_add(self.store.clone_work_with(budget)?);
        for frame in &self.frames {
            for value in frame.values() {
                work = work.saturating_add(value_work(value, budget)?);
            }
            for bytes in frame.byte_arrays() {
                work = work.saturating_add(bytes.clone_work_with(budget)?);
            }
            work = work.saturating_add(frame.saved_store.clone_work_with(budget)?);
        }
        for continuation in &self.continuations {
            work = work
                .saturating_add(value_work(&continuation.output_offset, budget)?)
                .saturating_add(value_work(&continuation.output_size, budget)?);
        }
        work = work.saturating_add(self.storage_observations.clone_work_with(budget)?);
        Ok(finish_quote(work, budget)?)
    }

    pub(in crate::analysis) fn clone_with_payment(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        let work = self.clone_work_with(budget)?;
        pay(budget, work)?;
        let out = self.clone();
        checkpoint(budget)?;
        Ok(out)
    }

    pub(in crate::analysis) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, MachineError> {
        pay(budget, self.work_size().saturating_add(other.work_size()))?;
        if self.frames.len() != other.frames.len()
            || self.continuations.len() != other.continuations.len()
        {
            return Ok(false);
        }
        for (left, right) in self.frames.iter().zip(&other.frames) {
            if !left.same_shape(right)
                || left.environment_calldata != right.environment_calldata
                || left.stack_aliases != right.stack_aliases
            {
                return Ok(false);
            }
            for (left, right) in left.values().zip(right.values()) {
                if !summary::equal(left, right, domain, budget)? {
                    return Ok(false);
                }
            }
            for (left, right) in left.byte_arrays().into_iter().zip(right.byte_arrays()) {
                if !left.eq_with_payment(right, domain, budget)? {
                    return Ok(false);
                }
            }
            if !left
                .saved_store
                .eq_with_payment(&right.saved_store, domain, budget)?
            {
                return Ok(false);
            }
        }
        for (left, right) in self.continuations.iter().zip(&other.continuations) {
            if left.return_block != right.return_block
                || left.creation != right.creation
                || !summary::equal(&left.output_offset, &right.output_offset, domain, budget)?
                || !summary::equal(&left.output_size, &right.output_size, domain, budget)?
            {
                return Ok(false);
            }
        }
        if !self.store.eq_with_payment(&other.store, domain, budget)? {
            return Ok(false);
        }
        self.storage_observations
            .eq_with_payment(&other.storage_observations, domain, budget)
    }
}

/// Import guarantees about the exact retained sources before a numerical
/// template discards their relational history. The input, including rollback
/// snapshots, stays unchanged if either copying or optional import is refused.
pub(in crate::analysis) fn copy_with_scalar_guarantees<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    payload: &EvmMachinePayload<N>,
    entry: &EvmEntry<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmMachinePayload<N>, MachineError> {
    let mut refined = payload.copy_for_policy(
        &mut crate::analysis::engine::policy::PolicyContext::new(domain, entry, budget),
    )?;
    let limits = domain.spec().relations();
    let mut projection = super::super::projection::Projection::new(
        &payload.relations,
        domain,
        &limits,
        budget,
        super::super::projection::Policy::Summary,
    );
    projection.payload(&mut refined)?;
    projection.finish()?;
    Ok(refined)
}

pub(in crate::analysis) fn form_normalized_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    payload: &EvmMachinePayload<N>,
    entry: &EvmEntry<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmPayloadSummary<N>, MachineError> {
    pay(budget, payload.policy_work_size())?;
    let storage_observations =
        super::super::storage_observations::EvmStorageObservations::form_with_payment(
            payload, domain, budget,
        )?;
    let mut frames = Vec::with_capacity(payload.call_stack.depth());
    for frame in payload.call_stack.iter() {
        let unknown = frame.code == FrameCode::Unknown;
        let stack = if unknown {
            Vec::new()
        } else {
            frame
                .stack
                .iter()
                .map(|value| scalar(value, budget))
                .collect::<Result<Vec<_>, _>>()?
        };
        let stack_aliases = StackAliases::form_with_payment(
            if unknown { &[] } else { &frame.stack },
            domain.spec().relations().enabled,
            domain,
            budget,
        )?;
        // try_zip takes several closures borrowing one ledger; a RefCell only
        // shares this synchronous borrow, never a second budget or refund.
        let ledger = std::cell::RefCell::new(&mut *budget);
        let mut summary = frame.try_zip(
            frame,
            stack,
            FrameMapOrder::BytesFirst,
            |value, _| scalar(value, &mut ledger.borrow_mut()),
            |value, _, address| implicit(value, address, entry, domain, &mut ledger.borrow_mut()),
            |bytes, _, kind| {
                if unknown {
                    let mut budget = ledger.borrow_mut();
                    // Empty/memory constructs exactly a length and default literal.
                    pay(
                        &mut budget,
                        domain
                            .literal_quote_work(alloy_primitives::U256::ZERO)
                            .saturating_mul(2)
                            .saturating_add(32),
                    )?;
                    let empty = match kind {
                        FrameByteKind::Memory => EvmByteArray::memory(),
                        _ => EvmByteArray::empty(),
                    };
                    Ok(empty.summary_projection_with_payment(domain, &mut budget)?)
                } else {
                    Ok(bytes.summary_projection_with_payment(domain, &mut ledger.borrow_mut())?)
                }
            },
            |saved, _| {
                Ok(saved
                    .state()
                    .summary_projection_with_payment(domain, &mut ledger.borrow_mut())?)
            },
        )?;
        if unknown {
            summary.key.jump_history.clear();
            summary.key.basic_block_index = 0;
        }
        summary.key.stack_height = summary.stack.len();
        frames.push(EvmFrameSummary {
            frame: summary,
            stack_aliases,
        });
    }
    let continuations = payload
        .call_stack
        .children()
        .iter()
        .map(|child| {
            Ok(EvmContinuation {
                return_block: child.continuation.return_block,
                creation: child.continuation.creation,
                output_offset: scalar(&child.continuation.output_offset, budget)?,
                output_size: scalar(&child.continuation.output_size, budget)?,
            })
        })
        .collect::<Result<Vec<_>, MachineError>>()?;
    let mut result = EvmPayloadSummary {
        frames,
        continuations,
        store: payload
            .store
            .summary_projection_with_payment(domain, budget)?,
        storage_observations,
        work: 0,
        location: payload.key(),
    };
    result.work = result.measure_work();
    checkpoint(budget)?;
    Ok(result)
}

pub(in crate::analysis) fn instantiate_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    payload: &EvmPayloadSummary<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmMachinePayload<N>, MachineError> {
    pay(budget, payload.work_size())?;
    payload.check_shape()?;
    let mut frames = Vec::with_capacity(payload.frames.len());
    for frame in &payload.frames {
        let enabled = domain.spec().relations().enabled;
        let stack = frame
            .stack_aliases
            .instantiate_with_payment(&frame.stack, enabled, budget)?;
        let ledger = std::cell::RefCell::new(&mut *budget);
        frames.push(frame.try_zip(
            frame,
            stack,
            FrameMapOrder::BytesFirst,
            |value, _| {
                Ok(identify(
                    summary::clone_value(value, &mut ledger.borrow_mut())?,
                    enabled,
                ))
            },
            |value, _, _| {
                Ok(Some(identify(
                    summary::clone_value(value, &mut ledger.borrow_mut())?,
                    enabled,
                )))
            },
            |bytes, _, _| Ok(bytes.instantiate_with_payment(domain, &mut ledger.borrow_mut())?),
            |saved, _| {
                Ok(EvmSnapshot::from_state(saved.instantiate_with_payment(
                    domain,
                    &mut ledger.borrow_mut(),
                )?))
            },
        )?);
    }
    let mut frames = frames.into_iter();
    let root = EvmRootFrame {
        state: frames.next().expect("nonempty relative stack"),
    };
    let children = frames
        .zip(&payload.continuations)
        .map(|(state, continuation)| {
            Ok(EvmChildFrame {
                state,
                continuation: EvmContinuation {
                    return_block: continuation.return_block,
                    creation: continuation.creation,
                    output_offset: summary::clone_value(&continuation.output_offset, budget)?,
                    output_size: summary::clone_value(&continuation.output_size, budget)?,
                },
            })
        })
        .collect::<Result<Vec<_>, MachineError>>()?;
    let mut output = EvmMachinePayload {
        call_stack: EvmCallStack::from_parts(root, children),
        store: payload.store.instantiate_with_payment(domain, budget)?,
        relations: RelationState::default(),
    };
    payload.storage_observations.install_with_payment(
        &mut output,
        &payload.active().stack_aliases,
        domain,
        budget,
    )?;
    checkpoint(budget)?;
    Ok(output)
}
