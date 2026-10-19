//! Capture-avoiding traversal includes immutable byte/slot roots and source
//! tokens. Numeric visit_values is intentionally insufficient for this job.

use super::{FrameState, MachineError, MachinePayload, join::checkpoint};
use crate::{
    analysis::state::StatePart,
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprLimits},
    },
};
use std::collections::{BTreeMap, BTreeSet};

fn collect_value(value: &AbstractValue, leaves: &mut BTreeSet<ExprId>) {
    if let Some(expression) = value.expression() {
        leaves.extend(expression.fresh_leaves());
    }
}

impl FrameState {
    pub(crate) fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut leaves = self.saved_store.state().fresh_leaves();
        for bytes in self.byte_arrays() {
            leaves.extend(bytes.fresh_leaves());
        }
        for value in self.values() {
            collect_value(value, &mut leaves);
        }
        leaves
    }
}

impl MachinePayload {
    pub(crate) fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut leaves = self.relations.fresh_leaves();
        leaves.extend(self.store.fresh_leaves());
        for frame in self.call_stack.iter() {
            leaves.extend(frame.fresh_leaves());
        }
        for child in self.call_stack.children() {
            collect_value(&child.continuation.output_offset, &mut leaves);
            collect_value(&child.continuation.output_size, &mut leaves);
        }
        leaves
    }

    /// The same map renames every occurrence of one local source/selector. A
    /// failed traversal leaves the original payload and its history untouched.
    pub(crate) fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), MachineError> {
        checkpoint()?;
        let mut renamed = self.clone();
        renamed.call_stack.transform(&mut |part| {
            match part {
                StatePart::Value(value) => {
                    checkpoint()?;
                    value.forget_identity();
                    value.rename_fresh(map, limits)?;
                }
                StatePart::Bytes(bytes) => bytes.rename_fresh(map, limits)?,
                StatePart::Store(store) => store.to_mut().rename_fresh(map, limits)?,
            }
            Ok(())
        })?;
        renamed.store.rename_fresh(map, limits)?;
        renamed.relations = renamed.relations.rename_fresh(map, limits)?;
        checkpoint()?;
        *self = renamed;
        Ok(())
    }
}
