//! Native scalar work for the frozen-coordinate summary path.
//!
//! These operations retain the actual carrier and stage owned results. Bound
//! normalization constructs retained components directly from borrowed inputs;
//! copying a component before projecting or forgetting it supplies no invariant.

use super::super::{
    AlgebraRef, EvmContinuation, EvmFrame, EvmFrameSummary, EvmPayloadSummary, MachineError,
    WorkBudget, bounded::reserve, stack_alias::StackAliases,
};
use crate::domain::{EvmNumericAlgebra, EvmValue, WordElement, summary};

impl<N: WordElement> EvmPayloadSummary<N> {
    fn copy_bound_location_with_payment(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<crate::analysis::MachineKey, MachineError> {
        // measure_work covers executable frames, not this independent report
        // projection. Fund its cached-length census before PolicyKey walks the
        // frame headers, then pay the complete key copy (including histories).
        reserve(
            budget,
            self.location
                .frames
                .len()
                .saturating_add(1)
                .saturating_mul(8),
        )?;
        crate::analysis::engine::policy::clone_key(&self.location, budget)
    }

    /// Build the normalized bound directly. Every retained native copy,
    /// projection and constructor obtains its own quote; discarded numeric
    /// payloads need no staging copy. The immutable input survives any refusal.
    pub(in crate::analysis) fn project_for_bound_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        layout: &Self,
        coarse: bool,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        reserve(budget, self.work_size().saturating_add(layout.work_size()))?;
        self.check_shape()?;
        layout.check_shape()?;
        if self.frames.len() != layout.frames.len() {
            return Err(MachineError::Shape);
        }
        let location = self.copy_bound_location_with_payment(budget)?;
        let scalar = |value: &EvmValue<N>, budget: &mut WorkBudget| {
            if coarse {
                let (lower, upper) = value.interval().unsigned_bounds();
                summary::unsigned_range(lower, upper, domain, budget)
            } else {
                summary::clone_value(value, budget)
            }
        };
        let store = |value: &crate::world::EvmStoreSummary<N>,
                     shape: &crate::world::EvmStoreSummary<N>,
                     budget: &mut WorkBudget| {
            if coarse {
                value.forget_loop_numeric_in_layout_with_payment(shape, domain, budget)
            } else {
                value.project_to_layout_with_payment(shape, domain, budget)
            }
        };
        let bytes = |value: &crate::world::EvmByteArraySummary<N>,
                     shape: &crate::world::EvmByteArraySummary<N>,
                     budget: &mut WorkBudget| {
            if coarse {
                // Projection preserves length/memory interpretation; its cell
                // and word results would immediately be erased by forgetting.
                value.forget_loop_content_with_payment(domain, budget)
            } else {
                value.project_to_layout_with_payment(shape, domain, budget)
            }
        };
        let current_store = store(&self.store, &layout.store, budget)?;
        let mut frames = Vec::with_capacity(self.frames.len());
        for (frame, shape) in self.frames.iter().zip(&layout.frames) {
            let stack = frame
                .stack
                .iter()
                .map(|value| scalar(value, budget))
                .collect::<Result<Vec<_>, _>>()?;
            let call_value = scalar(&frame.call_value, budget)?;
            let address_operand = scalar(&frame.address_operand, budget)?;
            let caller_operand = scalar(&frame.caller_operand, budget)?;
            let memory = bytes(&frame.memory, &shape.memory, budget)?;
            let calldata = bytes(&frame.calldata, &shape.calldata, budget)?;
            let returndata = bytes(&frame.returndata, &shape.returndata, budget)?;
            let saved_store = store(&frame.saved_store, &shape.saved_store, budget)?;
            let stack_aliases = StackAliases::form(&stack, false)?;
            frames.push(EvmFrameSummary {
                semantics: frame.semantics,
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
                stack_aliases,
            });
        }
        let mut continuations = Vec::with_capacity(self.continuations.len());
        for continuation in &self.continuations {
            continuations.push(EvmContinuation {
                return_block: continuation.return_block,
                output_offset: scalar(&continuation.output_offset, budget)?,
                output_size: scalar(&continuation.output_size, budget)?,
                creation: continuation.creation,
            });
        }
        let mut result = Self {
            frames,
            continuations,
            store: current_store,
            storage_observations: Default::default(),
            work: 0,
            location,
        };
        result.work = result.measure_work();
        reserve(budget, result.work_size())?;
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
                .continuations
                .iter()
                .zip(&incoming.continuations)
                .any(|(left, right)| {
                    left.return_block != right.return_block || left.creation != right.creation
                })
        {
            return Err(MachineError::Shape);
        }

        for (left, right) in self.frames.iter().zip(&incoming.frames) {
            if !left.same_shape_with_payment(right, budget)?
                || left.semantics != right.semantics
                || left.environment_calldata != right.environment_calldata
            {
                return Err(MachineError::Shape);
            }
        }
        let location = self.copy_bound_location_with_payment(budget)?;
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
                semantics: left.semantics,
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
            location,
        };
        result.work = result.measure_work();
        reserve(budget, result.work)?;
        Ok(result)
    }
}
