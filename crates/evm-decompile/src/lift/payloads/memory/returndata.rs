//! Reaching returndata epochs from source effect equations. A join requires one
//! identical acyclic CALL-family occurrence on every closed incoming edge.
use super::{MAX_DEPTH, Memory};
use crate::{EntityId, FactScope, ValueKind};
use std::collections::BTreeSet;

enum EpochFrame {
    Enter(EntityId),
    Exit(EntityId, Vec<EntityId>),
}
enum EpochEquation {
    Call(EntityId),
    Dependencies(Vec<EntityId>),
    Unknown,
}

impl<P: FnMut(usize) -> bool> Memory<'_, P> {
    pub fn return_data_call(&mut self, effect: EntityId) -> Option<EntityId> {
        // A raw query may have warmed the producer's memo while certificate
        // replay skips that raw record. Depth must therefore never decide
        // validity: traverse the source DAG explicitly under the work payer.
        if !self.charge(8) {
            return None;
        }
        let mut pending = vec![EpochFrame::Enter(effect)];
        let mut active = BTreeSet::new();
        while let Some(frame) = pending.pop() {
            match frame {
                EpochFrame::Enter(current) => {
                    if !self.charge(
                        16usize.saturating_add(
                            analysis_kernel::cost::key_lookup_work(self.values.len(), 1)
                                .saturating_mul(4),
                        ),
                    ) {
                        return None;
                    }
                    if self.return_data_epochs.contains_key(&current) {
                        continue;
                    }
                    if !active.insert(current) {
                        self.return_data_epochs.insert(current, None);
                        continue;
                    }
                    match self.epoch_equation(current)? {
                        EpochEquation::Call(call) => {
                            self.return_data_epochs.insert(current, Some(call));
                            active.remove(&current);
                        }
                        EpochEquation::Unknown => {
                            self.return_data_epochs.insert(current, None);
                            active.remove(&current);
                        }
                        EpochEquation::Dependencies(children) => {
                            if !self.charge(children.len().saturating_mul(8).saturating_add(4)) {
                                return None;
                            }
                            pending.push(EpochFrame::Exit(current, children.clone()));
                            pending.extend(children.into_iter().rev().map(EpochFrame::Enter));
                        }
                    }
                }
                EpochFrame::Exit(current, children) => {
                    if !self.charge(
                        children.len().saturating_mul(
                            analysis_kernel::cost::tree_work(self.return_data_epochs.len())
                                .saturating_add(1),
                        ),
                    ) {
                        return None;
                    }
                    let mut result = None;
                    let mut known = true;
                    for child in children {
                        let Some(Some(call)) = self.return_data_epochs.get(&child).copied() else {
                            known = false;
                            break;
                        };
                        if result.is_some_and(|old| old != call) {
                            known = false;
                            break;
                        }
                        result = Some(call);
                    }
                    self.return_data_epochs
                        .insert(current, if known { result } else { None });
                    active.remove(&current);
                }
            }
        }
        self.return_data_epochs.get(&effect).copied().flatten()
    }

    fn epoch_equation(&mut self, effect: EntityId) -> Option<EpochEquation> {
        let Some(value) = self.values.get(&effect) else {
            return Some(EpochEquation::Unknown);
        };
        match value.kind.clone() {
            ValueKind::Effect {
                operation: Some(id),
            } => {
                let Some(op) = self.operations.get(&id).copied() else {
                    return Some(EpochEquation::Unknown);
                };
                if op.effect_output != Some(effect) {
                    return Some(EpochEquation::Unknown);
                }
                if matches!(op.opcode, 0xf0..=0xf2 | 0xf4 | 0xf5 | 0xfa) {
                    return Some(if self.acyclic_operation(op) {
                        EpochEquation::Call(id)
                    } else {
                        EpochEquation::Unknown
                    });
                }
                if op.effects.return_data_write {
                    return Some(EpochEquation::Unknown);
                }
                let Some(previous) = op.effect_input else {
                    return Some(EpochEquation::Unknown);
                };
                if !self.charge(4) {
                    return None;
                }
                Some(EpochEquation::Dependencies(vec![previous]))
            }
            ValueKind::EffectParameter { block }
                if self.ir.fact_scope == FactScope::CompleteLocalCfg
                    && Some(block) != self.ir.entry =>
            {
                if self.blocks.get(&block)?.effect_parameter != effect {
                    return Some(EpochEquation::Unknown);
                }
                let Some(incoming) = self.incoming.get(&block) else {
                    return Some(EpochEquation::Unknown);
                };
                let count = incoming.len();
                if count == 0 {
                    return Some(EpochEquation::Unknown);
                }
                if !self.charge(count.saturating_mul(4)) {
                    return None;
                }
                let children = self
                    .incoming
                    .get(&block)?
                    .iter()
                    .map(|edge| edge.effect)
                    .collect();
                Some(EpochEquation::Dependencies(children))
            }
            _ => Some(EpochEquation::Unknown),
        }
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
