//! Execution data shared by frames whose role is relative to one call stack.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{EvmContinuation, FrameCode, FrameKey, MachineError, join};
use crate::{
    bytecode::Program,
    domain::{
        AlgebraRef, EvmValue,
        symbolic::{ExprId, ExprLimits},
    },
    world::{AddressInput, EvmByteArray, EvmSnapshot},
};
use serde::Serialize;

/// Common frame shape; byte, checkpoint and implicit-operand representations
/// distinguish executable state from numerical templates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename = "FrameState")]
pub struct EvmFrame<N: WordElement, Bytes, Checkpoint, Operand> {
    /// Structural identity used by the worklist.
    pub key: FrameKey,
    /// Resolved execution mode, without following another delegation pointer.
    pub code: FrameCode,
    /// Captured instruction stream; later calls resolve the current code overlay.
    pub program: Option<Program>,
    /// Abstract stack in bottom-to-top order.
    pub stack: Vec<EvmValue<N>>,
    /// This frame's private memory.
    pub memory: Bytes,
    /// Bytes copied from caller memory at entry, or supplied for the root.
    pub calldata: Bytes,
    /// This immutable array is the original environment calldata, independent of
    /// whether a callee is later detached as a relative summary root.
    pub environment_calldata: bool,
    /// Full data from the most recently completed child call.
    pub returndata: Bytes,
    /// CALLVALUE for this context; DELEGATECALL preserves its parent value.
    pub call_value: EvmValue<N>,
    /// Runtime ADDRESS expression for an abstract owner outside the fixed input vocabulary.
    pub address_operand: Operand,
    /// Runtime CALLER expression; independent callbacks do not alias the root caller.
    pub caller_operand: Operand,
    /// State immediately before entry, including persistent and transient effects.
    /// Root failure also restores this checkpoint.
    pub saved_store: Checkpoint,
}

/// Executable frame data with optional key-implied operands and rollback state.
pub type EvmFrameState<N> = EvmFrame<N, EvmByteArray<N>, EvmSnapshot<N>, Option<EvmValue<N>>>;

pub(in crate::analysis) enum FrameMapOrder {
    ScalarsFirst,
    BytesFirst,
}

pub(in crate::analysis) enum FrameByteKind {
    Memory,
    Calldata,
    Returndata,
}

// Only operand iteration differs. Keeping this small expansion over shared
// parts avoids a visitor trait for optional versus materialized scalars.
macro_rules! scalar_traversal {
    ($operand:ty, $operands:ident => $iterate:expr) => {
        impl<N: WordElement, B, C> EvmFrame<N, B, C, $operand> {
            pub(crate) fn values(&self) -> impl Iterator<Item = &EvmValue<N>> {
                let (values, $operands) = self.scalar_parts();
                values.chain($iterate)
            }
            pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut EvmValue<N>> {
                let (values, $operands) = self.scalar_parts_mut();
                values.chain($iterate)
            }
        }
    };
}
scalar_traversal!(Option<EvmValue<N>>, operands => operands.into_iter().flatten());
scalar_traversal!(EvmValue<N>, operands => operands);

impl<N: WordElement, Bytes, Checkpoint, Operand> EvmFrame<N, Bytes, Checkpoint, Operand> {
    fn scalar_parts(&self) -> (impl Iterator<Item = &EvmValue<N>>, [&Operand; 2]) {
        (
            self.stack.iter().chain([&self.call_value]),
            [&self.address_operand, &self.caller_operand],
        )
    }

    fn scalar_parts_mut(&mut self) -> (impl Iterator<Item = &mut EvmValue<N>>, [&mut Operand; 2]) {
        (
            self.stack.iter_mut().chain([&mut self.call_value]),
            [&mut self.address_operand, &mut self.caller_operand],
        )
    }

    pub(in crate::analysis) fn metadata_work(&self) -> usize {
        self.program
            .as_ref()
            .map_or(0, |program| {
                program
                    .blocks()
                    .iter()
                    .fold(program.byte_len(), |cost, block| {
                        cost.saturating_add(block.instructions.len())
                    })
            })
            .saturating_add(self.key.jump_history.len())
    }

    pub(crate) fn byte_arrays(&self) -> [&Bytes; 3] {
        [&self.memory, &self.calldata, &self.returndata]
    }

    pub(crate) fn byte_arrays_mut(&mut self) -> [&mut Bytes; 3] {
        [&mut self.memory, &mut self.calldata, &mut self.returndata]
    }

    pub(in crate::analysis) fn same_shape(&self, other: &Self) -> bool {
        self.key == other.key
            && self.code == other.code
            && self.program == other.program
            && self.stack.len() == other.stack.len()
    }

    /// One structural rebuilding path for both unary representation maps (zip
    /// with self) and binary joins. Callers prepare the stack/aliases first and
    /// retain their own shape and implicit-operand policy. Order preserves the
    /// original cancellation/error boundaries and replay's fresh-source order.
    #[expect(
        clippy::too_many_arguments,
        reason = "typed component mappings retain operation-specific order and policy"
    )]
    pub(in crate::analysis) fn try_zip<B, C, O>(
        &self,
        other: &Self,
        stack: Vec<EvmValue<N>>,
        order: FrameMapOrder,
        mut value: impl FnMut(&EvmValue<N>, &EvmValue<N>) -> Result<EvmValue<N>, MachineError>,
        mut operand: impl FnMut(&Operand, &Operand, AddressInput) -> Result<O, MachineError>,
        mut bytes: impl FnMut(&Bytes, &Bytes, FrameByteKind) -> Result<B, MachineError>,
        checkpoint: impl FnOnce(&Checkpoint, &Checkpoint) -> Result<C, MachineError>,
    ) -> Result<EvmFrame<N, B, C, O>, MachineError> {
        let mut values = || {
            Ok::<_, MachineError>((
                value(&self.call_value, &other.call_value)?,
                operand(
                    &self.address_operand,
                    &other.address_operand,
                    self.key.address_value,
                )?,
                operand(&self.caller_operand, &other.caller_operand, self.key.caller)?,
            ))
        };
        let mut arrays = || {
            Ok::<_, MachineError>([
                bytes(&self.memory, &other.memory, FrameByteKind::Memory)?,
                bytes(&self.calldata, &other.calldata, FrameByteKind::Calldata)?,
                bytes(
                    &self.returndata,
                    &other.returndata,
                    FrameByteKind::Returndata,
                )?,
            ])
        };
        let ((call_value, address_operand, caller_operand), [memory, calldata, returndata]) =
            match order {
                FrameMapOrder::ScalarsFirst => {
                    let values = values()?;
                    (values, arrays()?)
                }
                FrameMapOrder::BytesFirst => {
                    let arrays = arrays()?;
                    (values()?, arrays)
                }
            };
        Ok(EvmFrame {
            key: self.key.clone(),
            code: self.code,
            program: self.program.clone(),
            stack,
            memory,
            calldata,
            environment_calldata: self.environment_calldata,
            returndata,
            call_value,
            address_operand,
            caller_operand,
            saved_store: checkpoint(&self.saved_store, &other.saved_store)?,
        })
    }
}

impl<N: WordElement> EvmFrameState<N> {
    pub(crate) fn work_size(&self) -> usize {
        self.values()
            .map(EvmValue::work_size)
            .chain(self.byte_arrays().map(EvmByteArray::work_size))
            .fold(
                self.saved_store
                    .state()
                    .work_size()
                    .saturating_add(self.metadata_work())
                    .saturating_add(3),
                usize::saturating_add,
            )
    }

    pub(super) fn join_guarded<D: EvmNumericAlgebra<Element = N>>(
        &self,
        incoming: &Self,
        domain: AlgebraRef<'_, D>,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, MachineError> {
        if !self.same_shape(incoming) || self.environment_calldata != incoming.environment_calldata
        {
            return Err(MachineError::Shape);
        }
        let stack = self
            .stack
            .iter()
            .zip(&incoming.stack)
            .map(|(left, right)| join::join_value(left, right, domain, &selector, limits))
            .collect::<Result<_, _>>()?;
        self.try_zip(
            incoming,
            stack,
            FrameMapOrder::ScalarsFirst,
            |left, right| join::join_value(left, right, domain, &selector, limits),
            |left, right, implicit| match (left, right) {
                (None, None) => Ok(None),
                _ => {
                    // None denotes the key's EVM value, never its state-owner
                    // namespace. Environment materialization belongs to form.
                    let implicit = implicit.value_with::<N>();
                    Ok(Some(join::join_value(
                        left.as_ref().unwrap_or(&implicit),
                        right.as_ref().unwrap_or(&implicit),
                        domain,
                        &selector,
                        limits,
                    )?))
                }
            },
            |left, right, _| Ok(left.join_guarded_with(right, domain, selector.clone(), limits)?),
            |left, right| {
                Ok(EvmSnapshot::from_state(left.state().join_guarded_with(
                    right.state(),
                    domain,
                    selector.clone(),
                    limits,
                )?))
            },
        )
    }
}

/// The entry frame of one analysis call stack, with no parent continuation.
///
/// A certified callee graph has its own root; this role does not imply that the
/// frame is the entry of the enclosing transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRootFrame<N: WordElement> {
    /// Execution data retained when a child is detached for summary capture.
    pub state: EvmFrameState<N>,
}

impl<N: WordElement> EvmRootFrame<N> {
    /// Attach this relative root to a suspended parent using its continuation.
    pub fn into_child(self, continuation: EvmContinuation<N>) -> EvmChildFrame<N> {
        EvmChildFrame {
            state: self.state,
            continuation,
        }
    }
}

/// A nested frame that always carries the continuation of its suspended parent.
///
/// A child cannot be constructed without its return contract:
///
/// ```compile_fail
/// use evm_abstract::analysis::{ChildFrame, FrameState};
/// fn missing_continuation(state: FrameState) -> ChildFrame {
///     ChildFrame { state }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmChildFrame<N: WordElement> {
    /// Execution data and mandatory rollback checkpoint.
    pub state: EvmFrameState<N>,
    /// Caller destination and output range used when this child completes.
    pub continuation: EvmContinuation<N>,
}

impl<N: WordElement> EvmChildFrame<N> {
    /// Detach this child as a relative root while retaining its return contract.
    ///
    /// Summary replay must restore the continuation when reattaching the root.
    pub fn into_root(self) -> (EvmRootFrame<N>, EvmContinuation<N>) {
        (EvmRootFrame { state: self.state }, self.continuation)
    }
}

/// Default numeric assembly for [`EvmFrameState`].
pub type FrameState = EvmFrameState<crate::domain::NumericValue>;

/// Default numeric assembly for [`EvmRootFrame`].
pub type RootFrame = EvmRootFrame<crate::domain::NumericValue>;

/// Default numeric assembly for [`EvmChildFrame`].
pub type ChildFrame = EvmChildFrame<crate::domain::NumericValue>;
