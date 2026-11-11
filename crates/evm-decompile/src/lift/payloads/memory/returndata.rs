//! Reaching returndata epochs from source effect equations. A join requires one
//! identical acyclic CALL-family occurrence on every closed incoming edge.
use super::{MAX_DEPTH, Memory};
use crate::{EntityId, FactScope, ValueKind};
use std::collections::BTreeSet;

impl<P: FnMut(usize) -> bool> Memory<'_, P> {
    pub fn return_data_call(&mut self, effect: EntityId) -> Option<EntityId> {
        self.epoch(effect, 0, &mut BTreeSet::new())
    }

    fn epoch(
        &mut self,
        effect: EntityId,
        depth: usize,
        active: &mut BTreeSet<EntityId>,
    ) -> Option<EntityId> {
        if !self.charge(1) || depth > MAX_DEPTH || !active.insert(effect) {
            return None;
        }
        if let Some(result) = self.return_data_epochs.get(&effect) {
            active.remove(&effect);
            return *result;
        }
        let result = (|| match self.values.get(&effect)?.kind.clone() {
            ValueKind::Effect {
                operation: Some(id),
            } => {
                let op = *self.operations.get(&id)?;
                if op.effect_output != Some(effect) {
                    return None;
                }
                // CALL/CREATE semantics define an epoch even if a supplied
                // effect flag was falsely cleared. An unknown declared
                // writer cannot be traversed as an unchanged buffer.
                if matches!(op.opcode, 0xf0..=0xf2 | 0xf4 | 0xf5 | 0xfa) {
                    return self.acyclic_operation(op).then_some(id);
                }
                if op.effects.return_data_write {
                    return None;
                }
                self.epoch(op.effect_input?, depth + 1, active)
            }
            ValueKind::EffectParameter { block }
                if self.ir.fact_scope == FactScope::CompleteLocalCfg
                    && Some(block) != self.ir.entry =>
            {
                if self.blocks.get(&block)?.effect_parameter != effect {
                    return None;
                }
                let count = self.incoming.get(&block)?.len();
                if count == 0 || !self.charge(count) {
                    return None;
                }
                let incoming = self.incoming.get(&block)?.clone();
                let mut result = None;
                for edge in incoming {
                    let next = self.epoch(edge.effect, depth + 1, active)?;
                    if result.is_some_and(|known| known != next) {
                        return None;
                    }
                    result = Some(next);
                }
                result
            }
            _ => None,
        })();
        active.remove(&effect);
        self.return_data_epochs.insert(effect, result);
        result
    }

    pub fn return_data_size_call(&mut self, value: EntityId) -> Option<EntityId> {
        self.size_epoch(value, 0, &mut BTreeSet::new())
    }

    fn size_epoch(
        &mut self,
        value: EntityId,
        depth: usize,
        active: &mut BTreeSet<EntityId>,
    ) -> Option<EntityId> {
        if !self.charge(1) || depth > MAX_DEPTH || !active.insert(value) {
            return None;
        }
        let result = (|| match self.values.get(&value)?.kind.clone() {
            ValueKind::Operation {
                operation,
                index: 0,
            } => {
                let op = *self.operations.get(&operation)?;
                if op.opcode != 0x3d
                    || !op.inputs.is_empty()
                    || op.outputs.first() != Some(&value)
                    || !self.acyclic_operation(op)
                {
                    return None;
                }
                self.return_data_call(op.effect_input?)
            }
            ValueKind::Parameter { block, index }
                if self.ir.fact_scope == FactScope::CompleteLocalCfg
                    && Some(block) != self.ir.entry =>
            {
                if self.blocks.get(&block)?.parameters.get(index) != Some(&value) {
                    return None;
                }
                let count = self.incoming.get(&block)?.len();
                if count == 0 || !self.charge(count) {
                    return None;
                }
                let incoming = self.incoming.get(&block)?.clone();
                let mut result = None;
                for edge in incoming {
                    let next = self.size_epoch(*edge.arguments.get(index)?, depth + 1, active)?;
                    if result.is_some_and(|known| known != next) {
                        return None;
                    }
                    result = Some(next);
                }
                result
            }
            _ => None,
        })();
        active.remove(&value);
        result
    }
}
