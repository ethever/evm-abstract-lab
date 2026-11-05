//! Flat typed DAG admission, independent of raw-witness certification.
use super::{
    LiftedExpression, LiftedExpressionKind, LiftedModelError, LiftedProgram, LiftedType,
    MAX_LIFTED_RECORDS,
};
use crate::EntityId;
use std::collections::{BTreeMap, BTreeSet};

impl LiftedProgram {
    /// Bounded acyclic/type admission. Witness correspondence is checked by core.
    pub fn validate(&self) -> Result<(), LiftedModelError> {
        if self.entity_ids().count() > MAX_LIFTED_RECORDS {
            return Err(LiftedModelError::RecordLimit);
        }
        let mut ids = BTreeSet::new();
        for id in self.entity_ids() {
            if id.0 == 0 || !ids.insert(id) {
                return Err(LiftedModelError::DuplicateIdentity(id));
            }
        }
        let mut previous = BTreeMap::<EntityId, &LiftedExpression>::new();
        for e in &self.expressions {
            if e.ty != e.kind.ty() {
                return Err(LiftedModelError::Type(e.id));
            }
            for operand in e.kind.operands() {
                if !previous
                    .get(&operand)
                    .is_some_and(|child| child.block == e.block)
                {
                    return Err(LiftedModelError::Operand(e.id));
                }
            }
            let sort = |id: &EntityId, ty| previous.get(id).is_some_and(|child| child.ty == ty);
            let correct = match &e.kind {
                LiftedExpressionKind::WordFromPredicate(id) | LiftedExpressionKind::Not(id) => {
                    sort(id, LiftedType::Predicate)
                }
                LiftedExpressionKind::NonZero(id) => sort(id, LiftedType::Word256),
                LiftedExpressionKind::Compare { left, right, .. } => {
                    sort(left, LiftedType::Word256) && sort(right, LiftedType::Word256)
                }
                LiftedExpressionKind::WordOp { opcode, arguments } => {
                    evm_bytecode::word::arity(*opcode) == Some(arguments.len())
                        && arguments.iter().all(|id| sort(id, LiftedType::Word256))
                }
                _ => true,
            };
            if !correct {
                return Err(LiftedModelError::Type(e.id));
            }
            previous.insert(e.id, e);
        }
        let word = |id: &EntityId| {
            previous
                .get(id)
                .is_some_and(|e| e.ty == LiftedType::Word256)
        };
        let predicate = |id: &EntityId| {
            previous
                .get(id)
                .is_some_and(|e| e.ty == LiftedType::Predicate)
        };
        for b in &self.blocks {
            let word_in_scope = |id: &EntityId| {
                previous
                    .get(id)
                    .is_some_and(|e| e.ty == LiftedType::Word256 && e.block == b.block)
            };
            let predicate_in_scope = |id: &EntityId| {
                previous
                    .get(id)
                    .is_some_and(|e| e.ty == LiftedType::Predicate && e.block == b.block)
            };
            if b.condition
                .as_ref()
                .is_some_and(|id| !predicate_in_scope(id))
                || b.target.as_ref().is_some_and(|id| !word_in_scope(id))
                || !b.exit_values.iter().all(word_in_scope)
            {
                return Err(LiftedModelError::Type(b.id));
            }
            for s in &b.statements {
                if !s.inputs.iter().all(word_in_scope) {
                    return Err(LiftedModelError::Type(s.id));
                }
            }
        }
        for t in &self.transfers {
            if !t.arguments.iter().all(word) {
                return Err(LiftedModelError::Type(t.id));
            }
        }
        for c in &self.controls {
            if !predicate(&c.condition) {
                return Err(LiftedModelError::Type(c.id));
            }
        }
        Ok(())
    }
}
