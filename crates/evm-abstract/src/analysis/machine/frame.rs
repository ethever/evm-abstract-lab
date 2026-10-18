//! Execution data shared by frames whose role is relative to one call stack.

use super::{Continuation, FrameCode, FrameKey, MachineError, join};
use crate::{
    bytecode::Program,
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, ExprLimits},
    },
    world::{ByteArray, Snapshot},
};
use serde::Serialize;

/// Execution data and rollback checkpoint shared by root and child frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FrameState {
    /// Structural identity used by the worklist.
    pub key: FrameKey,
    /// Resolved execution mode, without following another delegation pointer.
    pub code: FrameCode,
    /// Captured instruction stream; later calls resolve the current code overlay.
    pub program: Option<Program>,
    /// Abstract stack in bottom-to-top order.
    pub stack: Vec<AbstractValue>,
    /// This frame's private memory.
    pub memory: ByteArray,
    /// Bytes copied from caller memory at entry, or supplied for the root.
    pub calldata: ByteArray,
    /// This immutable array is the original environment calldata, independent of
    /// whether a callee is later detached as a relative summary root.
    pub environment_calldata: bool,
    /// Full data from the most recently completed child call.
    pub returndata: ByteArray,
    /// CALLVALUE for this context; DELEGATECALL preserves its parent value.
    pub call_value: AbstractValue,
    /// Runtime ADDRESS expression for an abstract owner outside the fixed input vocabulary.
    pub address_operand: Option<AbstractValue>,
    /// Runtime CALLER expression; independent callbacks do not alias the root caller.
    pub caller_operand: Option<AbstractValue>,
    /// State immediately before entry, including persistent and transient effects.
    /// Root failure also restores this checkpoint.
    pub saved_store: Snapshot,
}

impl FrameState {
    pub(crate) fn work_size(&self) -> usize {
        self.stack.iter().fold(
            self.memory
                .work_size()
                .saturating_add(self.calldata.work_size())
                .saturating_add(self.returndata.work_size())
                .saturating_add(self.call_value.work_size())
                .saturating_add(
                    self.address_operand
                        .as_ref()
                        .map_or(0, AbstractValue::work_size),
                )
                .saturating_add(
                    self.caller_operand
                        .as_ref()
                        .map_or(0, AbstractValue::work_size),
                )
                .saturating_add(self.saved_store.state().work_size())
                .saturating_add(self.key.jump_history.len())
                .saturating_add(self.program.as_ref().map_or(0, |program| {
                    program.byte_len().saturating_add(
                        program.blocks().iter().fold(0usize, |cost, block| {
                            cost.saturating_add(block.instructions.len())
                        }),
                    )
                }))
                .saturating_add(3),
            |cost, value| cost.saturating_add(value.work_size()),
        )
    }

    pub(super) fn join_guarded(
        &self,
        incoming: &Self,
        domain: Domain,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, MachineError> {
        if self.key != incoming.key
            || self.code != incoming.code
            || self.program != incoming.program
            || self.environment_calldata != incoming.environment_calldata
            || self.stack.len() != incoming.stack.len()
        {
            return Err(MachineError::Shape);
        }
        let mut out = self.clone();
        out.stack = self
            .stack
            .iter()
            .zip(&incoming.stack)
            .map(|(left, right)| join::join_value(left, right, domain, &selector, limits))
            .collect::<Result<_, _>>()?;
        out.call_value = join::join_value(
            &self.call_value,
            &incoming.call_value,
            domain,
            &selector,
            limits,
        )?;
        for (output, left, right, implicit) in [
            (
                &mut out.address_operand,
                &self.address_operand,
                &incoming.address_operand,
                self.key.address_value,
            ),
            (
                &mut out.caller_operand,
                &self.caller_operand,
                &incoming.caller_operand,
                self.key.caller,
            ),
        ] {
            *output = match (left, right) {
                (None, None) => None,
                _ => {
                    // None uses the key's implicit EVM value. Without the
                    // environment scope, symbolic keys cover every address;
                    // the state-owner namespace is never a concrete fallback.
                    let implicit = implicit.value();
                    Some(join::join_value(
                        left.as_ref().unwrap_or(&implicit),
                        right.as_ref().unwrap_or(&implicit),
                        domain,
                        &selector,
                        limits,
                    )?)
                }
            };
        }
        out.memory =
            self.memory
                .join_guarded(&incoming.memory, domain, selector.clone(), limits)?;
        out.calldata =
            self.calldata
                .join_guarded(&incoming.calldata, domain, selector.clone(), limits)?;
        out.returndata =
            self.returndata
                .join_guarded(&incoming.returndata, domain, selector.clone(), limits)?;
        out.saved_store = Snapshot::from_state(self.saved_store.state().join_guarded(
            incoming.saved_store.state(),
            domain,
            selector,
            limits,
        )?);
        Ok(out)
    }
}

/// The entry frame of one analysis call stack, with no parent continuation.
///
/// A certified callee graph has its own root; this role does not imply that the
/// frame is the entry of the enclosing transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RootFrame {
    /// Execution data retained when a child is detached for summary capture.
    pub state: FrameState,
}

impl RootFrame {
    /// Attach this relative root to a suspended parent using its continuation.
    pub fn into_child(self, continuation: Continuation) -> ChildFrame {
        ChildFrame {
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
pub struct ChildFrame {
    /// Execution data and mandatory rollback checkpoint.
    pub state: FrameState,
    /// Caller destination and output range used when this child completes.
    pub continuation: Continuation,
}

impl ChildFrame {
    /// Detach this child as a relative root while retaining its return contract.
    ///
    /// Summary replay must restore the continuation when reattaching the root.
    pub fn into_root(self) -> (RootFrame, Continuation) {
        (RootFrame { state: self.state }, self.continuation)
    }
}
