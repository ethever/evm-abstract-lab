//! 两阶段构建：先给所有入栈槽位命名，再执行块并连接 φ。

use super::{
    Block, Instruction, Phi, PhiInput, Ssa, SsaError, SsaInvariantKind as Kind, ValueId,
    error::invariant,
};
use crate::analysis::{Analysis, Status};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

pub(super) fn build(analysis: &Analysis) -> Result<Ssa, SsaError> {
    crate::ssa::checkpoint()?;
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    let mut next = 0;
    let mut fresh = || {
        let value = next;
        next += 1;
        value
    };
    let mut blocks: Vec<Block> = analysis
        .states()
        .iter()
        .map(|state| Block {
            state: state.id,
            phis: (0..state.key.stack_height)
                .map(|slot| Phi {
                    result: fresh(),
                    slot,
                    inputs: Vec::new(),
                })
                .collect(),
            instructions: Vec::new(),
            exit_stack: Vec::new(),
        })
        .collect();

    for state in analysis.states() {
        crate::ssa::checkpoint()?;
        let block = &mut blocks[state.id];
        let mut stack: Vec<ValueId> = block.phis.iter().map(|phi| phi.result).collect();
        let source = &analysis.program().blocks()[state.key.basic_block_index];
        for instruction in source.instructions.iter().take(state.executed_pcs.len()) {
            crate::ssa::checkpoint()?;
            let op = instruction.opcode;
            let (inputs, outputs) = instruction.stack_io();
            let fault = !instruction.is_valid()
                || stack.len() < inputs
                || stack.len() - inputs + outputs > 1024;
            let mut item = Instruction {
                pc: instruction.pc,
                opcode: op,
                immediate: instruction.immediate,
                operands: Vec::new(),
                results: Vec::new(),
                fault,
            };
            if fault {
                block.instructions.push(item);
                break;
            }
            if (opcode::DUP1..=opcode::DUP16).contains(&op) {
                let value = stack[stack.len() - usize::from(op - opcode::DUP1 + 1)];
                item.operands.push(value);
                stack.push(value);
            } else if (opcode::SWAP1..=opcode::SWAP16).contains(&op) {
                let top = stack.len() - 1;
                let other = top - usize::from(op - opcode::SWAP1 + 1);
                item.operands.extend([stack[top], stack[other]]);
                stack.swap(top, other);
            } else {
                item.operands = stack.drain(stack.len() - inputs..).rev().collect();
                item.results = (0..outputs).map(|_| fresh()).collect();
                stack.extend(&item.results);
            }
            block.instructions.push(item);
        }
        block.exit_stack = stack;
    }

    // 一对节点可能既有 true 边又有 false 边；φ 按前驱节点去重，
    // 因为两条边携带相同出栈，不能伪造两份不同的块参数。
    let predecessor_pairs: BTreeSet<_> = analysis.edges().iter().map(|e| (e.from, e.to)).collect();
    for (from, to) in predecessor_pairs {
        crate::ssa::checkpoint()?;
        let outgoing = blocks[from].exit_stack.clone();
        if outgoing.len() != blocks[to].phis.len() {
            return Err(invariant(Kind::EdgeStackHeight)
                .source_state(from)
                .target_state(to)
                .expected(blocks[to].phis.len())
                .observed(outgoing.len()));
        }
        for (phi, value) in blocks[to].phis.iter_mut().zip(outgoing) {
            phi.inputs.push(PhiInput {
                predecessor: from,
                value,
            });
        }
    }
    Ok(Ssa {
        blocks,
        value_count: next,
    })
}
