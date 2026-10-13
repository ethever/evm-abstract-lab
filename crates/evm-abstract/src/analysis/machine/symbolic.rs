//! Capture-avoiding traversal includes immutable byte/slot roots and source
//! tokens. Numeric visit_values is intentionally insufficient for this job.

use super::{FrameState, MachineError, MachinePayload, join::checkpoint};
use crate::{
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprLimits},
    },
    world::Snapshot,
};
use std::collections::{BTreeMap, BTreeSet};

fn collect_value(value: &AbstractValue, leaves: &mut BTreeSet<ExprId>) {
    if let Some(expression) = value.expression() {
        leaves.extend(expression.fresh_leaves());
    }
}

fn rename_value(
    value: &mut AbstractValue,
    map: &BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
) -> Result<(), MachineError> {
    checkpoint()?;
    value.forget_identity();
    if let Some(expression) = value.expression() {
        *value = value
            .clone()
            .with_expression(expression.rename_fresh(map, limits)?);
    }
    Ok(())
}

impl FrameState {
    pub(crate) fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut leaves = self.saved_store.state().fresh_leaves();
        leaves.extend(self.memory.fresh_leaves());
        leaves.extend(self.calldata.fresh_leaves());
        leaves.extend(self.returndata.fresh_leaves());
        collect_value(&self.call_value, &mut leaves);
        for value in [&self.address_operand, &self.caller_operand]
            .into_iter()
            .flatten()
        {
            collect_value(value, &mut leaves);
        }
        for value in &self.stack {
            collect_value(value, &mut leaves);
        }
        leaves
    }

    fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), MachineError> {
        checkpoint()?;
        for value in &mut self.stack {
            rename_value(value, map, limits)?;
        }
        rename_value(&mut self.call_value, map, limits)?;
        for value in [&mut self.address_operand, &mut self.caller_operand]
            .into_iter()
            .flatten()
        {
            rename_value(value, map, limits)?;
        }
        self.memory.rename_fresh(map, limits)?;
        self.calldata.rename_fresh(map, limits)?;
        self.returndata.rename_fresh(map, limits)?;
        let mut saved = self.saved_store.state().clone();
        saved.rename_fresh(map, limits)?;
        self.saved_store = Snapshot::from_state(saved);
        Ok(())
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
        for frame in renamed.call_stack.iter_mut() {
            frame.rename_fresh(map, limits)?;
        }
        for child in renamed.call_stack.children_mut() {
            rename_value(&mut child.continuation.output_offset, map, limits)?;
            rename_value(&mut child.continuation.output_size, map, limits)?;
        }
        renamed.store.rename_fresh(map, limits)?;
        renamed.relations = renamed.relations.rename_fresh(map, limits)?;
        checkpoint()?;
        *self = renamed;
        Ok(())
    }
}
