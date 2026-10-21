//! Definitions and dominance are checked in the full native graph. A root
//! projection can contain an incoming child return without owning that child.

use super::{Ssa, SsaError, ValueId};
use crate::ssa::{SsaInvariantKind as Kind, error::invariant};
use crate::{
    analysis::{EvmAnalysis, Status},
    domain::WordElement,
};
use petgraph::{algo::dominators::simple_fast, graph::DiGraph};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

pub(super) fn verify<N: WordElement>(ssa: &Ssa, analysis: &EvmAnalysis<N>) -> Result<(), SsaError> {
    super::checkpoint()?;
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    if ssa.blocks.len() != analysis.states().len() {
        return Err(invariant(Kind::BlockCount)
            .expected(analysis.states().len())
            .observed(ssa.blocks.len()));
    }
    let native = analysis.execution();
    let mut graph = DiGraph::<(), ()>::new();
    let nodes = native
        .states()
        .iter()
        .map(|_| graph.add_node(()))
        .collect::<Vec<_>>();
    for edge in native.edges() {
        if edge.from >= nodes.len() || edge.to >= nodes.len() {
            return Err(invariant(Kind::EdgeStateMissing));
        }
        graph.add_edge(nodes[edge.from], nodes[edge.to], ());
    }
    let doms = nodes.first().map(|root| simple_fast(&graph, *root));
    let mut projected = ssa.execution.clone();
    let mut local_ids = BTreeMap::new();
    for (id, block) in ssa.blocks.iter().enumerate() {
        super::checkpoint()?;
        let fail = |kind| invariant(kind).state(id);
        if block.state != id
            || analysis.execution_state_id(id) != Some(block.native_state)
            || local_ids.insert(block.native_state, id).is_some()
        {
            return Err(fail(Kind::BlockIdentity));
        }
        let state = &analysis.states()[id];
        if block.phis.len() != state.entry_stack.len() {
            return Err(fail(Kind::EntryStackHeight)
                .expected(state.entry_stack.len())
                .observed(block.phis.len()));
        }
        if block.exit_stack.len() != state.exit_stack.len() {
            return Err(fail(Kind::ExitStackHeight)
                .expected(state.exit_stack.len())
                .observed(block.exit_stack.len()));
        }
        if block
            .instructions
            .iter()
            .map(|item| item.pc)
            .ne(state.executed_pcs.iter().copied())
        {
            return Err(fail(Kind::ExecutedPcs));
        }
        let source = &analysis.program().blocks()[state.key.basic_block_index];
        for (item, original) in block.instructions.iter().zip(&source.instructions) {
            if item.opcode != original.opcode || item.immediate != original.immediate {
                return Err(fail(Kind::BytecodeIdentity).pc(item.pc));
            }
        }
        let target = projected
            .blocks
            .get_mut(block.native_state)
            .ok_or_else(|| fail(Kind::BlockIdentity))?;
        target.phis = block
            .phis
            .iter()
            .map(|phi| super::FramePhi {
                frame: 0,
                slot: phi.slot,
                result: phi.result,
                inputs: phi
                    .inputs
                    .iter()
                    .map(|input| (input.edge, input.value))
                    .collect(),
            })
            .collect();
        target.instructions = block.instructions.clone();
        target.exit_frames = vec![block.exit_stack.clone()];
    }
    if projected.blocks.len() != native.states().len() {
        return Err(invariant(Kind::BlockCount));
    }
    // (native definition block, instruction order). Transition results have a
    // separate edge identity; they are available on that edge, before its phis.
    let mut definitions = BTreeMap::new();
    let mut transition_definitions = BTreeMap::new();
    for block in &projected.blocks {
        let reported = local_ids.get(&block.state).copied().unwrap_or(block.state);
        for phi in &block.phis {
            register(&mut definitions, phi.result, block.state, reported, 0)?;
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            for value in &instruction.results {
                register(&mut definitions, *value, block.state, reported, order + 1)
                    .map_err(|error| error.pc(instruction.pc))?;
            }
        }
    }
    for transition in projected.transitions() {
        if let Some(value) = transition.result {
            let edge = native
                .edges()
                .get(transition.edge)
                .ok_or_else(|| invariant(Kind::EdgeStateMissing))?;
            register(
                &mut definitions,
                value,
                edge.from,
                local_ids.get(&edge.from).copied().unwrap_or(edge.from),
                usize::MAX,
            )?;
            transition_definitions.insert(value, transition.edge);
        }
    }
    if definitions.len() != ssa.value_count
        || definitions.keys().copied().ne(0..ssa.value_count)
        || ssa.value_count != projected.value_count()
    {
        return Err(invariant(Kind::ContiguousValues)
            .expected(ssa.value_count)
            .observed(definitions.len()));
    }
    let check_use = |value: ValueId,
                     state: usize,
                     reported: usize,
                     order: usize,
                     edge: Option<usize>|
     -> Result<(), SsaError> {
        let fail = |kind| invariant(kind).state(reported).value(value);
        let (source, definition_order) = definitions
            .get(&value)
            .copied()
            .ok_or_else(|| fail(Kind::UndefinedValue))?;
        if let Some(definition_edge) = transition_definitions.get(&value) {
            return if edge == Some(*definition_edge) {
                Ok(())
            } else {
                Err(fail(Kind::NonDominatingDefinition)
                    .source_state(source)
                    .target_state(state))
            };
        }
        if source == state {
            if definition_order >= order {
                return Err(fail(Kind::UseBeforeDefinition)
                    .expected(order)
                    .observed(definition_order));
            }
        } else if !doms
            .as_ref()
            .and_then(|doms| doms.dominators(nodes[state]))
            .is_some_and(|mut path| path.any(|node| node == nodes[source]))
        {
            return Err(fail(Kind::NonDominatingDefinition)
                .source_state(source)
                .target_state(state));
        }
        Ok(())
    };
    for block in &ssa.blocks {
        super::checkpoint()?;
        let state = block.native_state;
        if !doms
            .as_ref()
            .is_some_and(|doms| doms.dominators(nodes[state]).is_some())
        {
            return Err(invariant(Kind::UnreachableBlock).state(block.state));
        }
        let expected = native
            .edges()
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| (edge.to == state).then_some(index))
            .collect::<BTreeSet<_>>();
        for (slot, phi) in block.phis.iter().enumerate() {
            let fail = |kind| {
                invariant(kind)
                    .state(block.state)
                    .slot(slot)
                    .value(phi.result)
            };
            if phi.slot != slot {
                return Err(fail(Kind::PhiSlotIdentity)
                    .expected(slot)
                    .observed(phi.slot));
            }
            let incoming = phi
                .inputs
                .iter()
                .map(|input| input.edge)
                .collect::<BTreeSet<_>>();
            if incoming != expected || incoming.len() != phi.inputs.len() {
                return Err(fail(Kind::PhiPredecessorCoverage)
                    .expected(expected.len())
                    .observed(phi.inputs.len()));
            }
            for input in &phi.inputs {
                let edge = &native.edges()[input.edge];
                if input.predecessor != edge.from
                    || projected
                        .transitions()
                        .get(input.edge)
                        .and_then(|transition| transition.stacks.first())
                        .and_then(|stack| stack.get(slot))
                        != Some(&input.value)
                {
                    return Err(fail(Kind::PhiOutgoingSlot)
                        .source_state(input.predecessor)
                        .value(input.value));
                }
                check_use(
                    input.value,
                    edge.from,
                    block.state,
                    projected.blocks[edge.from].instructions.len() + 1,
                    Some(input.edge),
                )?;
            }
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            for value in &instruction.operands {
                check_use(*value, state, block.state, order + 1, None)
                    .map_err(|error| error.pc(instruction.pc))?;
            }
        }
        for value in &block.exit_stack {
            check_use(
                *value,
                state,
                block.state,
                block.instructions.len() + 1,
                None,
            )?;
        }
    }
    projected.verify(native)
}

fn register(
    definitions: &mut BTreeMap<ValueId, (usize, usize)>,
    value: ValueId,
    native_state: usize,
    reported_state: usize,
    order: usize,
) -> Result<(), SsaError> {
    if definitions.insert(value, (native_state, order)).is_some() {
        return Err(invariant(Kind::DuplicateDefinition)
            .state(reported_state)
            .value(value)
            .expected(1)
            .observed(2));
    }
    Ok(())
}
