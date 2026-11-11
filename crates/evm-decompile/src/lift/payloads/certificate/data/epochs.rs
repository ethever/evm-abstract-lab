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
}

enum Equation {
    Known(Option<EntityId>),
    Parents(Vec<EntityId>),
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
        })
    }

    pub(in super::super) fn call<P: FnMut(usize) -> bool>(
        &mut self,
        effect: EntityId,
        memory: &mut Memory<'_, P>,
    ) -> Option<EntityId> {
        if !memory.charge(1) {
            return None;
        }
        // Explicit postorder avoids both host recursion and query-order-dependent
        // depth refusals. Only actual work limits may stop a long acyclic chain.
        let mut pending = vec![(effect, false)];
        let mut active = BTreeSet::new();
        while let Some((value, finish)) = pending.pop() {
            if !memory.charge(1) {
                return None;
            }
            if finish {
                active.remove(&value);
            }
            if self.cache.contains_key(&value) {
                continue;
            }
            if !finish && active.contains(&value) {
                self.cache.insert(value, None);
                continue;
            }
            let equation = self.equation(value, memory);
            if memory.refused {
                return None;
            }
            match equation.unwrap_or(Equation::Known(None)) {
                Equation::Known(epoch) => {
                    self.cache.insert(value, epoch);
                }
                Equation::Parents(parents) if finish => {
                    let epoch = parents
                        .first()
                        .and_then(|id| self.cache.get(id))
                        .copied()
                        .flatten();
                    let same = epoch.is_some()
                        && parents.iter().all(|id| self.cache.get(id) == Some(&epoch));
                    self.cache.insert(value, if same { epoch } else { None });
                }
                Equation::Parents(parents) => {
                    if !memory.charge(parents.len().saturating_add(2)) {
                        return None;
                    }
                    active.insert(value);
                    pending.push((value, true));
                    pending.extend(parents.into_iter().rev().map(|id| (id, false)));
                }
            }
        }
        self.cache.get(&effect).copied().flatten()
    }

    fn equation<P: FnMut(usize) -> bool>(
        &self,
        effect: EntityId,
        memory: &mut Memory<'_, P>,
    ) -> Option<Equation> {
        match self.values.get(&effect)? {
            ValueKind::Effect {
                operation: Some(id),
            } => {
                let operation = memory.operations.get(id).copied()?;
                if operation.effect_output != Some(effect) {
                    return None;
                }
                if matches!(operation.opcode, 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa) {
                    return Some(Equation::Known(
                        memory.acyclic_operation(operation).then_some(operation.id),
                    ));
                }
                if operation.effects.return_data_write || !memory.charge(1) {
                    return None;
                }
                Some(Equation::Parents(vec![operation.effect_input?]))
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
                Some(Equation::Parents(
                    edges.iter().map(|edge| edge.effect).collect(),
                ))
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
