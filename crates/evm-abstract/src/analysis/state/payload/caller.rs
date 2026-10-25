//! Complete caller operations preserve observations, aliases and native values.

use super::{
    AlgebraRef, EvmCallStack, EvmChildFrame, EvmContinuation, EvmEntry, EvmFrameSummary,
    EvmMachinePayload, EvmPayloadSummary, EvmRootFrame, EvmSnapshot, EvmValue, FrameByteKind,
    FrameCode, FrameMapOrder, MachineError, RelationState, StackAliases, identify,
};
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::world::EvmByteArray;
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
    // This census owns only outer Vec/frame/continuation coordinates. Every
    // scalar, byte array, Store/checkpoint and observation retains its original
    // full clone census and fee below; no opaque payload is scanned here.
    fn clone_structure_work_with(&self, budget: &mut WorkBudget) -> Result<usize, MachineError> {
        let entries = self
            .frames
            .len()
            .saturating_add(self.location.frames.len())
            .saturating_add(self.continuations.len())
            .saturating_add(1);
        // Fund this once-only borrowed metadata census before its loops.
        pay(budget, entries.saturating_mul(64).saturating_add(24))?;
        let key_work = |key: &crate::analysis::FrameKey| {
            24usize
                .saturating_add(
                    size_of::<crate::analysis::FrameKey>()
                        .div_ceil(size_of::<usize>())
                        .saturating_mul(4),
                )
                .saturating_add(key.jump_history.len().saturating_mul(8))
                .saturating_add(if key.process.is_some() {
                    size_of::<crate::analysis::ProcessFrameKey>()
                        .div_ceil(size_of::<usize>())
                        .saturating_mul(4)
                        .saturating_add(8)
                } else {
                    0
                })
        };
        let mut work = 64usize.saturating_add(entries.saturating_mul(8));
        for frame in &self.frames {
            // Later Native copy quotes visit every stack/call/implicit scalar.
            // This fixed per-value iterator credit is separate from those full
            // Native quotations and their actual Clone payment.
            pay(
                budget,
                frame.stack.len().saturating_add(3).saturating_mul(8),
            )?;
            work = work
                .saturating_add(key_work(&frame.key))
                .saturating_add(frame.stack_aliases.clone_work())
                .saturating_add(frame.stack.len().saturating_add(3).saturating_mul(4))
                .saturating_add(32);
            // Preserve the complete cold legacy Program allowance using cached
            // byte length. Process capture handles retain their fixed bound.
            work = work.saturating_add(if frame.key.process.is_some() {
                16
            } else {
                frame
                    .program
                    .as_ref()
                    .map_or(0, |program| program.byte_len().saturating_mul(2))
            });
        }
        for key in &self.location.frames {
            work = work.saturating_add(key_work(key));
        }
        work = work.saturating_add(
            self.continuations.len().saturating_mul(
                size_of::<crate::analysis::CreationContinuation>()
                    .div_ceil(size_of::<usize>())
                    .saturating_mul(4)
                    .saturating_add(32),
            ),
        );
        // check_shape reads only the two cached cardinalities, already funded
        // by the census header. The returned fee is paid separately before the
        // actual derived Clone, allocation and retained/released metadata.
        Ok(work)
    }

    /// Invoice exactly one derived Clone, including observations and aliases.
    /// Census work is paid here; callers (including kernel read_slot) pay the
    /// returned fee before copying. It does not fund queries on the copy.
    pub(in crate::analysis) fn clone_work_with(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<usize, MachineError> {
        let structure = self.clone_structure_work_with(budget)?;
        self.check_shape()?;
        let mut work = structure.saturating_add(self.store.clone_work_with(budget)?);
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

    /// Copy the exact numerical template for another destination of the same
    /// native jump producer. Callers must establish that only this coordinate
    /// differs; arbitrary summaries or target-specific predicates cannot use it.
    pub(in crate::analysis) fn clone_at_block_with_payment(
        &self,
        block: usize,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        let mut output = self.clone_with_payment(budget)?;
        pay(budget, 2)?;
        output
            .frames
            .last_mut()
            .ok_or(MachineError::Shape)?
            .frame
            .key
            .basic_block_index = block;
        output
            .location
            .frames
            .last_mut()
            .ok_or(MachineError::Shape)?
            .basic_block_index = block;
        checkpoint(budget)?;
        Ok(output)
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
            if left.semantics != right.semantics {
                return Ok(false);
            }
            if !left.same_shape_with_payment(right, budget)?
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
    form_with_runtime(payload, entry, domain, budget, false)
}

/// Process frames retain executable source and invocation/checkpoint values;
/// their private byte/stack data is interpreted only by the process adapter.
pub(in crate::analysis) fn form_with_runtime<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    payload: &EvmMachinePayload<N>,
    entry: &EvmEntry<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
    runtime_process: bool,
) -> Result<EvmPayloadSummary<N>, MachineError> {
    pay(budget, payload.policy_work_size())?;
    let mut storage_observations =
        super::super::storage_observations::EvmStorageObservations::form_with_payment(
            payload, domain, budget,
        )?;
    let mut frames = Vec::with_capacity(payload.call_stack.depth());
    for frame in payload.call_stack.iter() {
        let unknown = frame.code == FrameCode::Unknown;
        let known_process = runtime_process && frame.code == FrameCode::Runtime;
        let process = unknown || runtime_process;
        let semantics = if unknown {
            crate::analysis::TransferSemantics::Environment
        } else if known_process {
            crate::analysis::TransferSemantics::KnownProcess
        } else if runtime_process && frame.code == FrameCode::InitCode {
            crate::analysis::TransferSemantics::ConstructorProcess
        } else if runtime_process {
            crate::analysis::TransferSemantics::BoundaryProcess
        } else {
            crate::analysis::TransferSemantics::Native
        };
        if process {
            storage_observations = Default::default();
        }
        let stack = if process {
            Vec::new()
        } else {
            frame
                .stack
                .iter()
                .map(|value| scalar(value, budget))
                .collect::<Result<Vec<_>, _>>()?
        };
        let stack_aliases = StackAliases::form_with_payment(
            if process { &[] } else { &frame.stack },
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
                if process {
                    Ok(crate::world::EvmByteArraySummary::empty_with_payment(
                        matches!(kind, FrameByteKind::Memory),
                        domain,
                        &mut ledger.borrow_mut(),
                    )?)
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
        if process {
            summary.key.jump_history.clear();
            // Process admission moves this cursor to a typed MAY coordinate
            // before canonicalizing the Native adapter anchor.
            // Its private calldata has been quotiented out. Retaining a root
            // origin flag would give equal process keys incompatible shapes.
            summary.environment_calldata = false;
        }
        summary.key.stack_height = summary.stack.len();
        frames.push(EvmFrameSummary {
            semantics,
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
    // Identity and prepared representation must describe the same process
    // cursor, while code/lifecycle identity still comes from the actual Store.
    if runtime_process {
        result.location.frames = result
            .frames
            .iter()
            .map(|frame| frame.key.clone())
            .collect();
    }
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
            |bytes, _, kind| {
                if frame.key.process.is_some() {
                    // These private arrays carry no Process facts. Replaying
                    // their numerical templates would allocate fresh Native
                    // sources that the next form operation immediately erases.
                    let mut budget = ledger.borrow_mut();
                    Ok(match kind {
                        FrameByteKind::Memory => {
                            EvmByteArray::memory_with_payment(domain, &mut budget)?
                        }
                        _ => EvmByteArray::empty_with_payment(domain, &mut budget)?,
                    })
                } else {
                    Ok(bytes.instantiate_with_payment(domain, &mut ledger.borrow_mut())?)
                }
            },
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
