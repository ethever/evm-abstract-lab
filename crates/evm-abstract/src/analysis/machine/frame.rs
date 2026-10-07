//! Execution data shared by frames whose role is relative to one call stack.

use super::{Continuation, FrameCode, FrameKey};
use crate::{
    bytecode::Program,
    domain::{Domain, Value},
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
    pub stack: Vec<Value>,
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
    pub call_value: Value,
    /// State immediately before entry, including persistent and transient effects.
    /// Root failure also restores this checkpoint.
    pub saved_store: Snapshot,
}

impl FrameState {
    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(&Value)) {
        for value in &self.stack {
            visit(value);
        }
        self.memory.visit_values(visit);
        self.calldata.visit_values(visit);
        self.returndata.visit_values(visit);
        visit(&self.call_value);
        self.saved_store.state().visit_values(visit);
    }
    pub(crate) fn update_values(&mut self, update: &mut impl FnMut(&mut Value)) {
        for value in &mut self.stack {
            update(value);
        }
        self.memory.update_values(update);
        self.calldata.update_values(update);
        self.returndata.update_values(update);
        update(&mut self.call_value);
        let mut saved = self.saved_store.state().clone();
        saved.update_values(update);
        self.saved_store = Snapshot::from_state(saved);
    }
    pub(super) fn widen(&mut self, old: &Self, domain: Domain) {
        for (value, old) in self.stack.iter_mut().zip(&old.stack) {
            *value = domain.widen(old, value);
        }
        self.memory.widen(&old.memory, domain);
        self.calldata.widen(&old.calldata, domain);
        self.returndata.widen(&old.returndata, domain);
        self.call_value = domain.widen(&old.call_value, &self.call_value);
        let mut saved = self.saved_store.state().clone();
        saved.widen(old.saved_store.state(), domain);
        self.saved_store = Snapshot::from_state(saved);
    }
    pub(super) fn join(&mut self, incoming: &Self, domain: Domain) {
        for (slot, value) in self.stack.iter_mut().zip(&incoming.stack) {
            *slot = domain.join(slot, value);
        }
        self.memory = self.memory.join(&incoming.memory, domain);
        self.calldata = self.calldata.join(&incoming.calldata, domain);
        self.returndata = self.returndata.join(&incoming.returndata, domain);
        self.call_value = domain.join(&self.call_value, &incoming.call_value);
        self.saved_store = Snapshot::from_state(
            self.saved_store
                .state()
                .join(incoming.saved_store.state(), domain),
        );
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
