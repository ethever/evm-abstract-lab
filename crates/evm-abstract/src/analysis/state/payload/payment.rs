//! Native scalar work for the frozen-coordinate summary path.
//!
//! These operations retain the actual carrier and stage owned results. The
//! bounded representation erases storage observations before any join, so its
//! staging copy need not copy observations that would immediately be dropped.

use super::super::{
    AlgebraRef, EvmContinuation, EvmFrame, EvmFrameSummary, EvmPayloadSummary, MachineError,
    WorkBudget, bounded::reserve, stack_alias::StackAliases,
};
use crate::domain::{EvmNumericAlgebra, WordElement, summary};

impl<N: WordElement> EvmPayloadSummary<N> {
    /// Copy the complete bounded payload while separately paying every scalar
    /// and nested summary copy. Optional observations are not part of a bound.
    pub(in crate::analysis) fn copy_for_bound_with_payment(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        reserve(budget, self.work_size())?;
        self.check_shape()?;
        let store = self.store.clone_with_payment(budget)?;
        let mut frames = Vec::with_capacity(self.frames.len());
        for frame in &self.frames {
            let stack = frame
                .stack
                .iter()
                .map(|value| summary::clone_value(value, budget))
                .collect::<Result<Vec<_>, _>>()?;
            let call_value = summary::clone_value(&frame.call_value, budget)?;
            let address_operand = summary::clone_value(&frame.address_operand, budget)?;
            let caller_operand = summary::clone_value(&frame.caller_operand, budget)?;
            let memory = frame.memory.clone_with_payment(budget)?;
            let calldata = frame.calldata.clone_with_payment(budget)?;
            let returndata = frame.returndata.clone_with_payment(budget)?;
            let saved_store = frame.saved_store.clone_with_payment(budget)?;
            frames.push(EvmFrameSummary {
                frame: EvmFrame {
                    key: frame.key.clone(),
                    code: frame.code,
                    program: frame.program.clone(),
                    stack,
                    memory,
                    calldata,
                    environment_calldata: frame.environment_calldata,
                    returndata,
                    call_value,
                    address_operand,
                    caller_operand,
                    saved_store,
                },
                stack_aliases: frame.stack_aliases.clone(),
            });
        }
        let mut continuations = Vec::with_capacity(self.continuations.len());
        for continuation in &self.continuations {
            continuations.push(EvmContinuation {
                return_block: continuation.return_block,
                output_offset: summary::clone_value(&continuation.output_offset, budget)?,
                output_size: summary::clone_value(&continuation.output_size, budget)?,
                creation: continuation.creation,
            });
        }
        let mut result = Self {
            frames,
            continuations,
            store,
            storage_observations: Default::default(),
            work: 0,
            location: self.location.clone(),
        };
        result.work = result.measure_work();
        reserve(budget, 0)?;
        Ok(result)
    }

    /// Join a normalized bound without reconstructing native values from their
    /// public observations. The caller has erased optional storage guarantees;
    /// stack aliases are likewise restored only by the selected regional policy.
    pub(in crate::analysis) fn join_bound_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<Self, MachineError> {
        reserve(
            budget,
            self.work_size().saturating_add(incoming.work_size()),
        )?;
        self.check_shape()?;
        incoming.check_shape()?;
        if self.frames.len() != incoming.frames.len()
            || self.continuations.len() != incoming.continuations.len()
            || self
                .frames
                .iter()
                .zip(&incoming.frames)
                .any(|(left, right)| {
                    !left.same_shape(right)
                        || left.environment_calldata != right.environment_calldata
                })
            || self
                .continuations
                .iter()
                .zip(&incoming.continuations)
                .any(|(left, right)| {
                    left.return_block != right.return_block || left.creation != right.creation
                })
        {
            return Err(MachineError::Shape);
        }

        let store = self
            .store
            .join_with_payment(&incoming.store, domain, widen, budget)?;
        let mut frames = Vec::with_capacity(self.frames.len());
        for (left, right) in self.frames.iter().zip(&incoming.frames) {
            let stack = left
                .stack
                .iter()
                .zip(&right.stack)
                .map(|(left, right)| summary::join_plain(left, right, domain, widen, budget))
                .collect::<Result<Vec<_>, _>>()?;
            let call_value =
                summary::join_plain(&left.call_value, &right.call_value, domain, widen, budget)?;
            let address_operand = summary::join_plain(
                &left.address_operand,
                &right.address_operand,
                domain,
                widen,
                budget,
            )?;
            let caller_operand = summary::join_plain(
                &left.caller_operand,
                &right.caller_operand,
                domain,
                widen,
                budget,
            )?;
            let memory = left
                .memory
                .join_with_payment(&right.memory, domain, widen, budget)?;
            let calldata =
                left.calldata
                    .join_with_payment(&right.calldata, domain, widen, budget)?;
            let returndata =
                left.returndata
                    .join_with_payment(&right.returndata, domain, widen, budget)?;
            let saved_store =
                left.saved_store
                    .join_with_payment(&right.saved_store, domain, widen, budget)?;
            let stack_aliases = StackAliases::form(&stack, false)?;
            frames.push(EvmFrameSummary {
                frame: EvmFrame {
                    key: left.key.clone(),
                    code: left.code,
                    program: left.program.clone(),
                    stack,
                    memory,
                    calldata,
                    environment_calldata: left.environment_calldata,
                    returndata,
                    call_value,
                    address_operand,
                    caller_operand,
                    saved_store,
                },
                stack_aliases,
            });
        }
        let mut continuations = Vec::with_capacity(self.continuations.len());
        for (left, right) in self.continuations.iter().zip(&incoming.continuations) {
            continuations.push(EvmContinuation {
                return_block: left.return_block,
                output_offset: summary::join_plain(
                    &left.output_offset,
                    &right.output_offset,
                    domain,
                    widen,
                    budget,
                )?,
                output_size: summary::join_plain(
                    &left.output_size,
                    &right.output_size,
                    domain,
                    widen,
                    budget,
                )?,
                creation: left.creation,
            });
        }
        let mut result = Self {
            frames,
            continuations,
            store,
            storage_observations: Default::default(),
            work: 0,
            location: self.location.clone(),
        };
        result.work = result.measure_work();
        reserve(budget, result.work)?;
        Ok(result)
    }
}
