//! Scoped expression traversal for call certificates and state transformations.

use super::MachinePayload;
use crate::domain::{
    Value,
    symbolic::{ExprError, ExprId, ExprLimits},
};
use std::collections::{BTreeMap, BTreeSet};

impl MachinePayload {
    pub(crate) fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut leaves = self.relations.fresh_leaves();
        let mut visit = |value: &Value| {
            if let Some(expression) = value.expression() {
                leaves.extend(expression.fresh_leaves());
            }
        };
        self.call_stack.visit_values(&mut visit);
        self.store.visit_values(&mut visit);
        leaves
    }

    pub(crate) fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), ExprError> {
        let mut error = None;
        let mut update = |value: &mut Value| {
            value.forget_identity();
            if let Some(expression) = value.expression() {
                match expression.rename_fresh(map, limits) {
                    Ok(expression) => *value = value.clone().with_expression(expression),
                    Err(source) => error = Some(source),
                }
            }
        };
        self.call_stack.update_values(&mut update);
        self.store.update_values(&mut update);
        if let Some(error) = error {
            return Err(error);
        }
        self.relations = self.relations.rename_fresh(map, limits)?;
        Ok(())
    }
}
