//! Complete mutable state traversal. Operations choose how each component is
//! transformed; this walk owns frame coverage and rebuilding rollback snapshots.
//! Scalar updates use the world's scoped-value APIs and cannot rename roots.

use crate::{
    analysis::{CallStack, FrameState, MachineError, MachinePayload},
    domain::AbstractValue,
    world::{ByteArray, Snapshot, Store},
};
use std::borrow::Cow;

pub(in crate::analysis) enum StatePart<'a, 'store> {
    Value(&'a mut AbstractValue),
    Bytes(&'a mut ByteArray),
    /// A frame's rollback version; current transaction state stays with callers.
    Store(&'a mut Cow<'store, Store>),
}

impl StatePart<'_, '_> {
    pub(in crate::analysis) fn update_values(self, update: &mut impl FnMut(&mut AbstractValue)) {
        match self {
            Self::Value(value) => update(value),
            Self::Bytes(bytes) => bytes.update_values(update),
            Self::Store(store) => store.to_mut().update_values(update),
        }
    }
}

impl FrameState {
    pub(crate) fn values(&self) -> impl Iterator<Item = &AbstractValue> {
        self.stack
            .iter()
            .chain([&self.call_value])
            .chain(self.address_operand.iter())
            .chain(self.caller_operand.iter())
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut AbstractValue> {
        self.stack
            .iter_mut()
            .chain([&mut self.call_value])
            .chain(self.address_operand.iter_mut())
            .chain(self.caller_operand.iter_mut())
    }

    pub(crate) fn byte_arrays(&self) -> [&ByteArray; 3] {
        [&self.memory, &self.calldata, &self.returndata]
    }

    pub(crate) fn byte_arrays_mut(&mut self) -> [&mut ByteArray; 3] {
        [&mut self.memory, &mut self.calldata, &mut self.returndata]
    }
}

impl CallStack {
    pub(in crate::analysis) fn transform(
        &mut self,
        transform: &mut impl FnMut(StatePart<'_, '_>) -> Result<(), MachineError>,
    ) -> Result<(), MachineError> {
        for frame in self.iter_mut() {
            MachineError::check_cancelled()?;
            for value in frame.values_mut() {
                transform(StatePart::Value(value))?;
            }
            for bytes in frame.byte_arrays_mut() {
                transform(StatePart::Bytes(bytes))?;
            }
            // Replacement transforms can build directly from the borrowed
            // checkpoint; in-place transforms copy it only on first mutation.
            let mut saved = Cow::Borrowed(frame.saved_store.state());
            transform(StatePart::Store(&mut saved))?;
            frame.saved_store = Snapshot::from_state(saved.into_owned());
        }
        for child in self.children_mut() {
            MachineError::check_cancelled()?;
            transform(StatePart::Value(&mut child.continuation.output_offset))?;
            transform(StatePart::Value(&mut child.continuation.output_size))?;
        }
        Ok(())
    }
}

impl MachinePayload {
    /// Preserve frame/continuation/current-Store order for comparison forms.
    /// Relations and immutable roots are outside unconditional scalar updates.
    pub(in crate::analysis) fn update_values(
        &mut self,
        update: &mut impl FnMut(&mut AbstractValue),
    ) -> Result<(), MachineError> {
        self.call_stack.transform(&mut |part| {
            part.update_values(update);
            Ok(())
        })?;
        self.store.update_values(update);
        Ok(())
    }
}
