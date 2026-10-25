//! Known-code MAY control, evaluated in the existing Body/Resume relation.
//!
//! Each block starts with an arbitrary erased stack prefix. Only the suffix
//! established inside that block supplies constants; crossing any block or
//! call boundary forgets it. This deliberately loses correlations, while never
//! taking constants from the private Native adapter arrays or stack.

use super::capabilities::Capabilities;
use crate::{
    analysis::{EdgeKind, MachineEdgeKind, MachineError, ProcessLocation},
    bytecode::Program,
    resource::{WorkBudget, clone_payment::pay},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

mod region;
pub(in crate::analysis::transfer::procedure) use region::Cache;
pub(super) use region::Plan;

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub capabilities: Capabilities,
    pub successors: [Option<(ProcessLocation, MachineEdgeKind)>; 2],
    pub continuation: Option<ProcessLocation>,
}

impl Step {
    fn boundary(self) -> bool {
        self.capabilities.effects
            || self.capabilities.returns
            || self.capabilities.reverts
            || self.continuation.is_some()
    }

    pub(super) fn at(
        program: &Program,
        index: usize,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        if index == 0 && program.blocks().is_empty() {
            pay(budget, 32)?;
            return Ok(Self {
                capabilities: Capabilities::boundary(crate::analysis::FrameCode::Empty),
                successors: [None, None],
                continuation: None,
            });
        }
        let block = program.blocks().get(index).ok_or(MachineError::Shape)?;
        // Includes decoding-field inspection, the local literal stack, bounded
        // word comparisons and the at-most-two retained control successors.
        pay(
            budget,
            block
                .instructions
                .len()
                .saturating_mul(128)
                .saturating_add(32),
        )?;
        let mut step = Self {
            capabilities: Capabilities::empty(),
            successors: [None, None],
            continuation: None,
        };
        let next = if index + 1 < program.blocks().len() {
            ProcessLocation::Block(index + 1)
        } else {
            ProcessLocation::End
        };
        let mut stack = Vec::<Option<U256>>::new();
        for instruction in &block.instructions {
            MachineError::check_cancelled()?;
            if !instruction.is_valid() {
                return Ok(step);
            }
            step.capabilities.observe(instruction.opcode);
            let op = instruction.opcode;
            match op {
                opcode::JUMP | opcode::JUMPI => {
                    let destination = stack.pop().flatten();
                    let condition = if op == opcode::JUMPI {
                        stack.pop().flatten()
                    } else {
                        Some(U256::from(1))
                    };
                    if condition != Some(U256::ZERO) {
                        let target = match destination {
                            Some(value) => usize::try_from(value)
                                .ok()
                                .and_then(|pc| program.jumpdest_blocks().get(&pc).copied())
                                .map(ProcessLocation::Block),
                            None => Some(ProcessLocation::JumpTargets),
                        };
                        step.successors[0] = target.map(|location| {
                            (
                                location,
                                MachineEdgeKind::Intraprocedural(if op == opcode::JUMP {
                                    EdgeKind::Jump
                                } else {
                                    EdgeKind::BranchTrue
                                }),
                            )
                        });
                    }
                    if op == opcode::JUMPI && condition.is_none_or(|value| value == U256::ZERO) {
                        step.successors[1] = Some((
                            next,
                            MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse),
                        ));
                    }
                    // Invalid destinations have no normal taken edge. Entered
                    // Failure is emitted by the process transfer independently.
                    return Ok(step);
                }
                opcode::CALL
                | opcode::STATICCALL
                | opcode::DELEGATECALL
                | opcode::CALLCODE
                | opcode::CREATE
                | opcode::CREATE2 => {
                    step.continuation = Some(next);
                    return Ok(step);
                }
                opcode::STOP | opcode::RETURN | opcode::REVERT | opcode::SELFDESTRUCT => {
                    return Ok(step);
                }
                opcode::PUSH0..=opcode::PUSH32 => stack.push(instruction.immediate),
                opcode::DUP1..=opcode::DUP16 => {
                    let depth = usize::from(op - opcode::DUP1) + 1;
                    stack.push(stack.len().checked_sub(depth).and_then(|i| stack[i]));
                }
                opcode::SWAP1..=opcode::SWAP16 => {
                    let depth = usize::from(op - opcode::SWAP1) + 2;
                    if let Some(i) = stack.len().checked_sub(depth) {
                        let top = stack.len() - 1;
                        stack.swap(i, top);
                    } else if let Some(top) = stack.last_mut() {
                        *top = None;
                    }
                }
                opcode::PC => stack.push(Some(U256::from(instruction.pc))),
                opcode::CODESIZE => stack.push(Some(U256::from(program.byte_len()))),
                opcode::ISZERO => {
                    let value = stack
                        .pop()
                        .flatten()
                        .map(|value| U256::from(u8::from(value == U256::ZERO)));
                    stack.push(value);
                }
                opcode::EQ => {
                    let first = stack.pop().flatten();
                    let second = stack.pop().flatten();
                    stack.push(first.zip(second).map(|(a, b)| U256::from(u8::from(a == b))));
                }
                _ => {
                    let (inputs, outputs) = instruction.stack_io();
                    for _ in 0..inputs {
                        stack.pop();
                    }
                    stack.extend(std::iter::repeat_n(None, outputs));
                }
            }
            if stack.len() > 1024 {
                // This suffix alone exceeds the protocol stack bound, under
                // every possible erased prefix. Only exceptional execution remains.
                return Ok(step);
            }
        }
        step.successors[0] = Some((
            next,
            MachineEdgeKind::Intraprocedural(EdgeKind::Fallthrough),
        ));
        Ok(step)
    }
}
