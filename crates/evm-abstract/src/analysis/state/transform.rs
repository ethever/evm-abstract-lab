//! Complete mutable state traversal. Operations choose how each component is
//! transformed; this walk owns frame coverage and rebuilding rollback snapshots.
//! Scalar updates use the world's scoped-value APIs and cannot rename roots.

use crate::domain::WordElement;

use crate::{
    analysis::machine::{EvmCallStack, EvmMachinePayload, MachineError},
    domain::EvmValue,
    world::{EvmByteArray, EvmSnapshot, EvmStore},
};
use std::borrow::Cow;

pub(in crate::analysis) enum StatePart<'a, 'store, N: WordElement> {
    Value(&'a mut EvmValue<N>),
    Bytes(&'a mut EvmByteArray<N>),
    /// A frame's rollback version; current transaction state stays with callers.
    Store(&'a mut Cow<'store, EvmStore<N>>),
}

impl<N: WordElement> StatePart<'_, '_, N> {
    pub(in crate::analysis) fn update_values(self, update: &mut impl FnMut(&mut EvmValue<N>)) {
        match self {
            Self::Value(value) => update(value),
            Self::Bytes(bytes) => bytes.update_values(update),
            Self::Store(store) => store.to_mut().update_values(update),
        }
    }
}

impl<N: WordElement> EvmCallStack<N> {
    pub(in crate::analysis) fn transform(
        &mut self,
        transform: &mut impl FnMut(StatePart<'_, '_, N>) -> Result<(), MachineError>,
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
            frame.saved_store = EvmSnapshot::from_state(saved.into_owned());
        }
        for child in self.children_mut() {
            MachineError::check_cancelled()?;
            transform(StatePart::Value(&mut child.continuation.output_offset))?;
            transform(StatePart::Value(&mut child.continuation.output_size))?;
        }
        Ok(())
    }
}

impl<N: WordElement> EvmMachinePayload<N> {
    /// Preserve frame/continuation/current-Store order for comparison forms.
    /// Relations and immutable roots are outside unconditional scalar updates.
    pub(in crate::analysis) fn update_values(
        &mut self,
        update: &mut impl FnMut(&mut EvmValue<N>),
    ) -> Result<(), MachineError> {
        self.call_stack.transform(&mut |part| {
            part.update_values(update);
            Ok(())
        })?;
        self.store.update_values(update);
        Ok(())
    }
}
