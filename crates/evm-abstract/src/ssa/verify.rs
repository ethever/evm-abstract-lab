//! SSA 的可执行契约。φ 的 use 发生在前驱边上，普通 use 发生在本块内。
//! 这一区别决定循环 φ 为什么合法：回边的值不必支配循环头。

use super::{Ssa, SsaError, ValueId};
use crate::analysis::{Analysis, Status};
use petgraph::{algo::dominators::simple_fast, graph::DiGraph};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

fn invariant(message: impl Into<String>) -> SsaError {
    SsaError::Invariant(message.into())
}

pub(super) fn verify(ssa: &Ssa, analysis: &Analysis) -> Result<(), SsaError> {
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    if ssa.blocks.len() != analysis.states().len() {
        return Err(invariant("block count mismatch"));
    }
    if ssa.blocks.is_empty() {
        return if ssa.value_count == 0 {
            Ok(())
        } else {
            Err(invariant("values without blocks"))
        };
    }
    let mut graph = DiGraph::<(), ()>::new();
    let nodes: Vec<_> = ssa.blocks.iter().map(|_| graph.add_node(())).collect();
    let mut predecessors = vec![BTreeSet::new(); nodes.len()];
    for edge in analysis.edges() {
        graph.add_edge(nodes[edge.from], nodes[edge.to], ());
        predecessors[edge.to].insert(edge.from);
    }
    let doms = simple_fast(&graph, nodes[0]);
    // (定义块, 定义位置)。φ 的位置为 0，指令位置从 1 开始。
    let mut definitions = BTreeMap::new();
    for (index, block) in ssa.blocks.iter().enumerate() {
        if block.state != index {
            return Err(invariant("block identity mismatch"));
        }
        if doms.dominators(nodes[index]).is_none() {
            return Err(invariant("unreachable SSA block"));
        }
        if block.phis.len() != analysis.states()[index].key.stack_height {
            return Err(invariant("entry stack height mismatch"));
        }
        if block.exit_stack.len() != analysis.states()[index].exit_stack.len() {
            return Err(invariant("exit stack height mismatch"));
        }
        let actual_pcs: Vec<_> = block.instructions.iter().map(|i| i.pc).collect();
        if actual_pcs != analysis.states()[index].executed_pcs {
            return Err(invariant("executed pc mismatch"));
        }
        let original = &analysis.program().blocks()[analysis.states()[index].key.block];
        for (item, source) in block.instructions.iter().zip(&original.instructions) {
            if item.opcode != source.opcode || item.immediate != source.immediate {
                return Err(invariant("opcode or immediate differs from the bytecode"));
            }
        }
        for value in block.phis.iter().map(|p| p.result) {
            register(&mut definitions, value, index, 0)?;
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            for value in &instruction.results {
                register(&mut definitions, *value, index, order + 1)?;
            }
        }
    }
    if definitions.len() != ssa.value_count || definitions.keys().copied().ne(0..ssa.value_count) {
        return Err(invariant(
            "value IDs must be contiguous and each defined once",
        ));
    }
    let check_use = |value: ValueId, block: usize, order: usize| -> Result<(), SsaError> {
        let Some((source, definition_order)) = definitions.get(&value).copied() else {
            return Err(invariant(format!("undefined value %{value}")));
        };
        if source == block {
            if definition_order >= order {
                return Err(invariant(format!("use before definition %{value}")));
            }
        } else if !doms
            .dominators(nodes[block])
            .is_some_and(|mut it| it.any(|d| d == nodes[source]))
        {
            return Err(invariant(format!(
                "definition %{value} does not dominate S{block}"
            )));
        }
        Ok(())
    };
    for block in &ssa.blocks {
        for (slot, phi) in block.phis.iter().enumerate() {
            if phi.slot != slot {
                return Err(invariant("phi slot identity mismatch"));
            }
            let sources: BTreeSet<_> = phi.inputs.iter().map(|i| i.predecessor).collect();
            if sources != predecessors[block.state] || sources.len() != phi.inputs.len() {
                return Err(invariant(
                    "phi must have exactly one input per distinct predecessor",
                ));
            }
            for input in &phi.inputs {
                let source = &ssa.blocks[input.predecessor];
                if source.exit_stack.get(slot) != Some(&input.value) {
                    return Err(invariant(
                        "phi input must equal the predecessor's outgoing slot",
                    ));
                }
                check_use(
                    input.value,
                    input.predecessor,
                    source.instructions.len() + 1,
                )?;
            }
        }
        for (order, instruction) in block.instructions.iter().enumerate() {
            for value in &instruction.operands {
                check_use(*value, block.state, order + 1)?;
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
        return Err(invariant(format!("duplicate definition %{value}")));
    }
    Ok(())
}
