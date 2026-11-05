//! Iterative scoped pure evaluation; opaque observations are caller bindings.
use super::{LiftedExpressionKind, LiftedProgram, LiftedType, LiftedValue, MAX_LIFTED_RECORDS};
use crate::{EntityId, U256};
use std::collections::{BTreeMap, BTreeSet};

impl LiftedProgram {
    /// Evaluate one scoped pure expression without invoking opaque producers.
    /// Every shared dependency is evaluated once; traversal/stack are bounded by
    /// the flat arena, and lookup supplies already-bound primitive/entry words.
    pub fn evaluate_expression(
        &self,
        id: EntityId,
        mut lookup: impl FnMut(EntityId) -> Option<U256>,
    ) -> Option<LiftedValue> {
        if self.expressions.len() > MAX_LIFTED_RECORDS {
            return None;
        }
        let index: BTreeMap<_, _> = self.expressions.iter().map(|e| (e.id, e)).collect();
        let mut needed = BTreeSet::new();
        let mut pending = vec![id];
        while let Some(next) = pending.pop() {
            if needed.insert(next) {
                let e = index.get(&next)?;
                pending.extend(e.kind.operands());
            }
        }
        let mut result = BTreeMap::new();
        for e in &self.expressions {
            if !needed.contains(&e.id) {
                continue;
            }
            let word = |id: &EntityId| match result.get(id) {
                Some(LiftedValue::Word(v)) => Some(*v),
                _ => None,
            };
            let pred = |id: &EntityId| match result.get(id) {
                Some(LiftedValue::Predicate(v)) => Some(*v),
                _ => None,
            };
            let value = match &e.kind {
                LiftedExpressionKind::Word(v) => LiftedValue::Word(*v),
                LiftedExpressionKind::Predicate(v) => LiftedValue::Predicate(*v),
                LiftedExpressionKind::Value(v) => LiftedValue::Word(lookup(*v)?),
                LiftedExpressionKind::WordFromPredicate(v) => {
                    LiftedValue::Word(U256::from(u8::from(pred(v)?)))
                }
                LiftedExpressionKind::NonZero(v) => LiftedValue::Predicate(word(v)? != U256::ZERO),
                LiftedExpressionKind::Not(v) => LiftedValue::Predicate(!pred(v)?),
                LiftedExpressionKind::Compare {
                    relation,
                    left,
                    right,
                } => LiftedValue::Predicate(relation.evaluate(word(left)?, word(right)?)),
                LiftedExpressionKind::WordOp { opcode, arguments } => {
                    let args = arguments.iter().map(word).collect::<Option<Vec<_>>>()?;
                    LiftedValue::Word(evm_bytecode::word::evaluate(*opcode, &args).ok()?)
                }
            };
            if value_type(value) != e.ty {
                return None;
            }
            result.insert(e.id, value);
        }
        result.get(&id).copied()
    }
}
fn value_type(value: LiftedValue) -> LiftedType {
    match value {
        LiftedValue::Word(_) => LiftedType::Word256,
        LiftedValue::Predicate(_) => LiftedType::Predicate,
    }
}
