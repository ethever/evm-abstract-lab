//! Capture-avoiding traversal includes immutable byte/slot roots and source
//! tokens. Numeric visit_values is intentionally insufficient for this job.

use crate::domain::{AlgebraRef, EvmNumericAlgebra, ValueAlgebra, WordElement};

use super::{EvmFrameState, EvmMachinePayload, MachineError, join::checkpoint};
use crate::{
    analysis::state::StatePart,
    domain::{
        EvmValue,
        symbolic::{ExprId, ExprLimits},
    },
};
use std::collections::{BTreeMap, BTreeSet};

fn collect_value<N: WordElement>(value: &EvmValue<N>, leaves: &mut BTreeSet<ExprId>) {
    if let Some(expression) = value.expression() {
        leaves.extend(expression.fresh_leaves());
    }
}

impl<N: WordElement> EvmFrameState<N> {
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

impl<N: WordElement> EvmMachinePayload<N> {
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
    pub(crate) fn rename_fresh_with<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
        domain: AlgebraRef<'_, D>,
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
                StatePart::Bytes(bytes) => bytes.rename_fresh_with(map, limits, domain)?,
                StatePart::Store(store) => store.to_mut().rename_fresh_with(map, limits, domain)?,
            }
            Ok(())
        })?;
        renamed.store.rename_fresh_with(map, limits, domain)?;
        renamed.relations = renamed.relations.rename_fresh(map, limits)?;
        checkpoint()?;
        *self = renamed;
        Ok(())
    }
}

// Existing default replay/SSA helpers retain their scalar-free call syntax.
// Generic executable paths must carry their run's algebra into `_with`.
impl super::MachinePayload {
    pub(crate) fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), MachineError> {
        let numeric = crate::domain::Domain::default();
        self.rename_fresh_with(map, limits, ValueAlgebra::new(numeric).borrowed())
    }
}
