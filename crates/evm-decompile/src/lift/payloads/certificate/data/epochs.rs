//! Independent closed-effect ancestry for immutable returndata occurrences.
use super::Memory;
use crate::{EntityId, FactScope, Ir, ValueKind};
use std::collections::{BTreeMap, BTreeSet};

pub(in super::super) struct Epochs<'a> {
    ir: &'a Ir,
    values: BTreeMap<EntityId, &'a ValueKind>,
    blocks: BTreeMap<EntityId, &'a crate::Block>,
    incoming: BTreeMap<EntityId, Vec<&'a crate::Edge>>,
    cache: BTreeMap<EntityId, Option<EntityId>>,
    active: BTreeSet<EntityId>,
}

impl<'a> Epochs<'a> {
    pub(in super::super) fn new(ir: &'a Ir, pay: &mut impl FnMut(usize) -> bool) -> Option<Self> {
        if !pay(ir
            .values
            .len()
            .saturating_add(ir.blocks.len())
            .saturating_add(ir.edges.len())
            .saturating_mul(2))
        {
            return None;
        }
        let mut incoming = BTreeMap::<_, Vec<_>>::new();
        for edge in &ir.edges {
            incoming.entry(edge.to).or_default().push(edge);
        }
        Some(Self {
            ir,
            values: ir
                .values
                .iter()
                .map(|value| (value.id, &value.kind))
                .collect(),
            blocks: ir.blocks.iter().map(|block| (block.id, block)).collect(),
            incoming,
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
        })
    }

    pub(super) fn call<P: FnMut(usize) -> bool>(
        &mut self,
        effect: EntityId,
        memory: &mut Memory<'_, P>,
    ) -> Option<EntityId> {
        if !memory.charge(1) {
            return None;
        }
        if let Some(result) = self.cache.get(&effect) {
            return *result;
        }
        if self.active.len() > 256 || !self.active.insert(effect) {
            return None;
        }
        let result = self.follow(effect, memory);
        self.active.remove(&effect);
        self.cache.insert(effect, result);
        result
    }

    fn follow<P: FnMut(usize) -> bool>(
        &mut self,
        effect: EntityId,
        memory: &mut Memory<'_, P>,
    ) -> Option<EntityId> {
        match self.values.get(&effect)? {
            ValueKind::Effect {
                operation: Some(id),
            } => {
                let operation = memory.operations.get(id).copied()?;
                if operation.effect_output != Some(effect) {
                    return None;
                }
                if matches!(operation.opcode, 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa) {
                    return memory.acyclic_operation(operation).then_some(operation.id);
                }
                if operation.effects.return_data_write {
                    return None;
                }
                self.call(operation.effect_input?, memory)
            }
            ValueKind::EffectParameter { block } => {
                if self.ir.fact_scope != FactScope::CompleteLocalCfg
                    || self.ir.entry == Some(*block)
                    || self.blocks.get(block)?.effect_parameter != effect
                {
                    return None;
                }
                let edges = self.incoming.get(block)?;
                if edges.is_empty() || !memory.charge(edges.len()) {
                    return None;
                }
                let edges = edges.clone();
                let mut epoch = None;
                for edge in edges {
                    let current = self.call(edge.effect, memory)?;
                    if epoch.is_some_and(|old| old != current) {
                        return None;
                    }
                    epoch = Some(current);
                }
                epoch
            }
            _ => None,
        }
    }

    pub(super) fn size_call<P: FnMut(usize) -> bool>(
        &mut self,
        value: EntityId,
        memory: &mut Memory<'_, P>,
    ) -> Option<EntityId> {
        self.size_equation(value, memory, &mut BTreeSet::new())
    }

    fn size_equation<P: FnMut(usize) -> bool>(
        &mut self,
        value: EntityId,
        memory: &mut Memory<'_, P>,
        active: &mut BTreeSet<EntityId>,
    ) -> Option<EntityId> {
        if !memory.charge(1) || active.len() > 256 || !active.insert(value) {
            return None;
        }
        let result = (|| match self.values.get(&value)? {
            ValueKind::Operation {
                operation,
                index: 0,
            } => {
                let operation = memory.operations.get(operation).copied()?;
                if operation.opcode != 0x3d
                    || !operation.inputs.is_empty()
                    || operation.outputs.as_slice() != [value]
                    || !memory.acyclic_operation(operation)
                {
                    return None;
                }
                self.call(operation.effect_input?, memory)
            }
            ValueKind::Parameter { block, index } => {
                if self.ir.fact_scope != FactScope::CompleteLocalCfg
                    || self.ir.entry == Some(*block)
                    || self.blocks.get(block)?.parameters.get(*index) != Some(&value)
                {
                    return None;
                }
                let index = *index;
                let edges = self.incoming.get(block)?;
                if edges.is_empty() || !memory.charge(edges.len()) {
                    return None;
                }
                let edges = edges.clone();
                let mut epoch = None;
                for edge in edges {
                    let current =
                        self.size_equation(*edge.arguments.get(index)?, memory, active)?;
                    if epoch.is_some_and(|old| old != current) {
                        return None;
                    }
                    epoch = Some(current);
                }
                epoch
            }
            _ => None,
        })();
        active.remove(&value);
        result
    }
}
