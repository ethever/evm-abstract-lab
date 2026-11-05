//! Project unused word parameters without changing recovered EVM execution.
//!
//! Primitive operands are roots even for pure/stack operations, whose bodies
//! remain executable. An edge argument is live only when its destination slot
//! is live, so a phi-only cycle can disappear. Open exits root their full entry
//! stacks because their missing successors may consume any of those operands.
//! Recovery, rather than compact tuple arity, owns physical stack-height faults.

use crate::{EntityId, Ir, Progress, Terminator, ValueKind, artifact::Builder};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

/// Mutate only the caller's unpublished candidate. Refusal leaves current intact.
pub(super) fn run(owner: &mut Builder<impl FnMut(Progress)>, ir: &mut Ir) -> Option<usize> {
    if !owner.pay(super::ir_size(ir).saturating_mul(8)) {
        return None;
    }
    let parameters: BTreeSet<_> = ir
        .blocks
        .iter()
        .flat_map(|block| block.parameters.iter().copied())
        .collect();
    let targets: BTreeMap<_, _> = ir
        .blocks
        .iter()
        .map(|block| (block.id, &block.parameters))
        .collect();
    let mut dependencies = BTreeMap::<EntityId, Vec<EntityId>>::new();
    for edge in &ir.edges {
        if !owner.pay(edge.arguments.len().saturating_add(1)) {
            return None;
        }
        // Current was validated: each known edge has the target's full arity.
        for (parameter, argument) in targets[&edge.to].iter().zip(&edge.arguments) {
            if parameters.contains(argument) {
                dependencies.entry(*parameter).or_default().push(*argument);
            }
        }
    }
    let mut live = BTreeSet::new();
    let mut todo = Vec::new();
    for block in &ir.blocks {
        if !owner.pay(block.parameters.len().saturating_add(1)) {
            return None;
        }
        if block.terminator.is_partial() {
            for parameter in &block.parameters {
                mark(*parameter, &parameters, &mut live, &mut todo);
            }
        }
        for operation in &block.operations {
            if !owner.pay(operation.inputs.len().saturating_add(1)) {
                return None;
            }
            for operand in &operation.inputs {
                mark(*operand, &parameters, &mut live, &mut todo);
            }
        }
        match &block.terminator {
            Terminator::Jump { target, .. } => {
                mark(*target, &parameters, &mut live, &mut todo);
            }
            Terminator::Branch {
                condition, target, ..
            } => {
                mark(*condition, &parameters, &mut live, &mut todo);
                mark(*target, &parameters, &mut live, &mut todo);
            }
            Terminator::Exit { values, .. } => {
                if !owner.pay(values.len()) {
                    return None;
                }
                for value in values {
                    mark(*value, &parameters, &mut live, &mut todo);
                }
            }
            _ => {}
        }
    }
    while let Some(parameter) = todo.pop() {
        if !owner.pay(1) {
            return None;
        }
        for argument in dependencies.get(&parameter).into_iter().flatten() {
            if !owner.pay(1) {
                return None;
            }
            mark(*argument, &parameters, &mut live, &mut todo);
        }
    }
    let removed = parameters.len() - live.len();
    if removed == 0 {
        return Some(0);
    }
    let projection: BTreeMap<_, Vec<_>> = ir
        .blocks
        .iter()
        .map(|block| {
            (
                block.id,
                block
                    .parameters
                    .iter()
                    .map(|id| live.contains(id))
                    .collect(),
            )
        })
        .collect();
    for edge in &mut ir.edges {
        if !owner.pay(edge.arguments.len().saturating_add(1)) {
            return None;
        }
        let mut slot = 0;
        edge.arguments.retain(|_| {
            let keep = projection[&edge.to][slot];
            slot += 1;
            keep
        });
    }
    let mut indices = BTreeMap::new();
    for block in &mut ir.blocks {
        if !owner.pay(block.parameters.len().saturating_add(1)) {
            return None;
        }
        block.parameters.retain(|id| live.contains(id));
        indices.extend(
            block
                .parameters
                .iter()
                .enumerate()
                .map(|(index, id)| (*id, index)),
        );
    }
    if !owner.pay(ir.values.len()) {
        return None;
    }
    ir.values.retain_mut(|value| {
        if let ValueKind::Parameter { index, .. } = &mut value.kind {
            let Some(compact) = indices.get(&value.id) else {
                return false;
            };
            *index = *compact;
        }
        true
    });
    Some(removed)
}

fn mark(
    id: EntityId,
    parameters: &BTreeSet<EntityId>,
    live: &mut BTreeSet<EntityId>,
    todo: &mut Vec<EntityId>,
) {
    if parameters.contains(&id) && live.insert(id) {
        todo.push(id);
    }
}
