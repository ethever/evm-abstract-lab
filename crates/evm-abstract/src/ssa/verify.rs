//! SSA 的可执行契约。φ 的 use 发生在前驱边上，普通 use 发生在本块内。
//! 这一区别决定循环 φ 为什么合法：回边的值不必支配循环头。

use super::{Ssa, SsaError, ValueId};
use crate::analysis::{Analysis, Status};
use crate::ssa::{SsaInvariantKind as Kind, error::invariant};
use petgraph::{algo::dominators::simple_fast, graph::DiGraph};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

pub(super) fn verify(ssa: &Ssa, analysis: &Analysis) -> Result<(), SsaError> {
    crate::ssa::checkpoint()?;
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    if ssa.blocks.len() != analysis.states().len() {
        return Err(invariant(Kind::BlockCount)
            .expected(analysis.states().len())
            .observed(ssa.blocks.len()));
    }
    if ssa.blocks.is_empty() {
        return if ssa.value_count == 0 {
            Ok(())
        } else {
            Err(invariant(Kind::ValuesWithoutBlocks)
                .expected(0)
                .observed(ssa.value_count))
        };
    }
    let mut graph = DiGraph::<(), ()>::new();
    let nodes: Vec<_> = ssa.blocks.iter().map(|_| graph.add_node(())).collect();
    let mut predecessors = vec![BTreeSet::new(); nodes.len()];
    for edge in analysis.edges() {
        crate::ssa::checkpoint()?;
        graph.add_edge(nodes[edge.from], nodes[edge.to], ());
        predecessors[edge.to].insert(edge.from);
    }
    let doms = simple_fast(&graph, nodes[0]);
    // (定义块, 定义位置)。φ 的位置为 0，指令位置从 1 开始。
    let mut definitions = BTreeMap::new();
    for (index, block) in ssa.blocks.iter().enumerate() {
        crate::ssa::checkpoint()?;
        let invariant = |kind| invariant(kind).state(index);
        if block.state != index {
            return Err(invariant(Kind::BlockIdentity)
                .expected(index)
                .observed(block.state));
        }
        if doms.dominators(nodes[index]).is_none() {
            return Err(invariant(Kind::UnreachableBlock));
        }
        if block.phis.len() != analysis.states()[index].key.stack_height {
            return Err(invariant(Kind::EntryStackHeight)
                .expected(analysis.states()[index].key.stack_height)
                .observed(block.phis.len()));
        }
        if block.exit_stack.len() != analysis.states()[index].exit_stack.len() {
            return Err(invariant(Kind::ExitStackHeight)
                .expected(analysis.states()[index].exit_stack.len())
                .observed(block.exit_stack.len()));
        }
        let actual_pcs: Vec<_> = block.instructions.iter().map(|i| i.pc).collect();
        if actual_pcs != analysis.states()[index].executed_pcs {
            return Err(invariant(Kind::ExecutedPcs));
        }
        let original = &analysis.program().blocks()[analysis.states()[index].key.basic_block_index];
        for (item, source) in block.instructions.iter().zip(&original.instructions) {
            if item.opcode != source.opcode || item.immediate != source.immediate {
                return Err(invariant(Kind::BytecodeIdentity).pc(source.pc));
            }
        }
        for value in block.phis.iter().map(|p| p.result) {
            register(&mut definitions, value, index, 0)?;
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            crate::ssa::checkpoint()?;
            for value in &instruction.results {
                register(&mut definitions, *value, index, order + 1)
                    .map_err(|error| error.pc(instruction.pc))?;
            }
        }
    }
    if definitions.len() != ssa.value_count || definitions.keys().copied().ne(0..ssa.value_count) {
        return Err(invariant(Kind::ContiguousValues)
            .expected(ssa.value_count)
            .observed(definitions.len()));
    }
    let check_use = |value: ValueId, block: usize, order: usize| -> Result<(), SsaError> {
        let invariant = |kind| invariant(kind).state(block).value(value);
        let Some((source, definition_order)) = definitions.get(&value).copied() else {
            return Err(invariant(Kind::UndefinedValue));
        };
        if source == block {
            if definition_order >= order {
                return Err(invariant(Kind::UseBeforeDefinition)
                    .expected(order)
                    .observed(definition_order));
            }
        } else if !doms
            .dominators(nodes[block])
            .is_some_and(|mut it| it.any(|d| d == nodes[source]))
        {
            return Err(invariant(Kind::NonDominatingDefinition)
                .source_state(source)
                .target_state(block));
        }
        Ok(())
    };
    for block in &ssa.blocks {
        crate::ssa::checkpoint()?;
        let invariant = |kind| invariant(kind).state(block.state);
        for (slot, phi) in block.phis.iter().enumerate() {
            let invariant = |kind| invariant(kind).slot(slot).value(phi.result);
            if phi.slot != slot {
                return Err(invariant(Kind::PhiSlotIdentity)
                    .expected(slot)
                    .observed(phi.slot));
            }
            let sources: BTreeSet<_> = phi.inputs.iter().map(|i| i.predecessor).collect();
            if sources != predecessors[block.state] || sources.len() != phi.inputs.len() {
                return Err(invariant(Kind::PhiPredecessorCoverage)
                    .expected(predecessors[block.state].len())
                    .observed(phi.inputs.len()));
            }
            for input in &phi.inputs {
                let source = &ssa.blocks[input.predecessor];
                if source.exit_stack.get(slot) != Some(&input.value) {
                    return Err(invariant(Kind::PhiOutgoingSlot)
                        .source_state(input.predecessor)
                        .value(input.value));
                }
                check_use(
                    input.value,
                    input.predecessor,
                    source.instructions.len() + 1,
                )?;
            }
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            crate::ssa::checkpoint()?;
            for value in &instruction.operands {
                check_use(*value, block.state, order + 1)
                    .map_err(|error| error.pc(instruction.pc))?;
            }
        }
        for value in &block.exit_stack {
            check_use(*value, block.state, block.instructions.len() + 1)?;
        }
    }
    Ok(())
}

fn register(
    definitions: &mut BTreeMap<ValueId, (usize, usize)>,
    value: ValueId,
    block: usize,
    order: usize,
) -> Result<(), SsaError> {
    if definitions.insert(value, (block, order)).is_some() {
        return Err(invariant(Kind::DuplicateDefinition)
            .state(block)
            .value(value)
            .expected(1)
            .observed(2));
    }
    Ok(())
}
