//! Allocate prefix definitions first, then wire only receipt-supported edges.

use super::{
    DeferredSsaEdge, PartialBlockCoverage, PartialInstruction, PartialWorldBlock, PartialWorldSsa,
    coverage, deferred_reason, is_call,
};
use crate::ssa::{SsaInvariantKind as Kind, error::invariant};
use crate::{
    analysis::{InstructionProgress, MachineEdgeKind, Status, WorldAnalysis},
    ssa::{EffectInput, EffectPhi, FramePhi, Instruction, SsaError, Transition},
};
use revm_bytecode::opcode;
use std::collections::BTreeMap;

pub(super) fn build(analysis: &WorldAnalysis) -> Result<PartialWorldSsa, SsaError> {
    crate::ssa::checkpoint()?;
    let mut next = 0;
    let mut effect_next = 0;
    let mut blocks = Vec::new();
    for state in analysis.states() {
        let invariant = |kind| invariant(kind).state(state.id);
        crate::ssa::checkpoint()?;
        let mut phis = Vec::new();
        let mut stacks = Vec::new();
        for (frame, entry) in state.entry.call_stack.iter().enumerate() {
            let mut stack = Vec::new();
            for slot in 0..entry.stack.len() {
                phis.push(FramePhi {
                    frame,
                    slot,
                    result: next,
                    inputs: Vec::new(),
                });
                stack.push(next);
                next += 1;
            }
            stacks.push(stack);
        }
        let effect = EffectPhi {
            result: effect_next,
            inputs: Vec::new(),
        };
        effect_next += 1;
        let mut exit_effect = effect.result;
        let block_coverage = coverage(state);
        let mut instructions = Vec::new();
        if block_coverage == PartialBlockCoverage::Current {
            let evidence = state.execution_evidence().expect("current receipt exists");
            if evidence.instructions().len() != state.executed_pcs.len() {
                return Err(invariant(Kind::ReceiptPcCount)
                    .expected(state.executed_pcs.len())
                    .observed(evidence.instructions().len()));
            }
            let original = state
                .program()
                .and_then(|p| p.blocks().get(state.active().basic_block_index));
            let stack = stacks
                .last_mut()
                .ok_or_else(|| invariant(Kind::PartialFrameMissing))?;
            for (order, progress) in evidence.instructions().iter().copied().enumerate() {
                let invariant = |kind| invariant(kind).pc(state.executed_pcs[order]);
                crate::ssa::checkpoint()?;
                let source = original
                    .and_then(|b| b.instructions.get(order))
                    .ok_or_else(|| invariant(Kind::ReceiptOutsideRuntime))?;
                if source.pc != state.executed_pcs[order] {
                    return Err(invariant(Kind::ReceiptPrefix));
                }
                let (inputs, outputs) = source.stack_io();
                let fault = !source.is_valid()
                    || stack.len() < inputs
                    || stack.len() - inputs + outputs > 1024;
                let mut item = Instruction {
                    pc: source.pc,
                    opcode: source.opcode,
                    immediate: source.immediate,
                    operands: Vec::new(),
                    results: Vec::new(),
                    fault: false,
                };
                match progress {
                    InstructionProgress::Started => {}
                    InstructionProgress::Faulted => {
                        if !fault {
                            return Err(invariant(Kind::FaultReceipt));
                        }
                        item.fault = true;
                    }
                    InstructionProgress::OperandsConsumed => {
                        if fault
                            || source.immediate.is_some()
                            || (opcode::DUP1..=opcode::SWAP16).contains(&source.opcode)
                        {
                            return Err(invariant(Kind::PendingOperands));
                        }
                        item.operands = stack.drain(stack.len() - inputs..).rev().collect();
                    }
                    InstructionProgress::Completed | InstructionProgress::Dispatched => {
                        if fault {
                            return Err(invariant(Kind::CompletedReceiptFault));
                        }
                        if (opcode::DUP1..=opcode::DUP16).contains(&source.opcode) {
                            let value =
                                stack[stack.len() - usize::from(source.opcode - opcode::DUP1 + 1)];
                            item.operands.push(value);
                            stack.push(value);
                        } else if (opcode::SWAP1..=opcode::SWAP16).contains(&source.opcode) {
                            let top = stack.len() - 1;
                            let other = top - usize::from(source.opcode - opcode::SWAP1 + 1);
                            item.operands.extend([stack[top], stack[other]]);
                            stack.swap(top, other);
                        } else {
                            item.operands = stack.drain(stack.len() - inputs..).rev().collect();
                            if progress == InstructionProgress::Dispatched
                                && outputs > 0
                                && !is_call(source.opcode)
                            {
                                return Err(invariant(Kind::DispatchReceiptResult));
                            }
                            for _ in 0..if is_call(source.opcode) { 0 } else { outputs } {
                                item.results.push(next);
                                stack.push(next);
                                next += 1;
                            }
                        }
                    }
                }
                let effect_result = if progress == InstructionProgress::Started {
                    None
                } else {
                    let id = effect_next;
                    effect_next += 1;
                    Some(id)
                };
                instructions.push(PartialInstruction {
                    instruction: item,
                    progress,
                    effect_input: exit_effect,
                    effect_result,
                });
                if let Some(effect) = effect_result {
                    exit_effect = effect;
                }
            }
        }
        blocks.push(PartialWorldBlock {
            state: state.id,
            coverage: block_coverage,
            phis,
            instructions,
            exit_frames: stacks,
            effect,
            exit_effect,
            open_incoming: Vec::new(),
            incoming_complete: analysis.status() == Status::Converged,
        });
    }
    let mut transitions = Vec::new();
    let mut deferred_edges = Vec::new();
    for (index, edge) in analysis.edges().iter().enumerate() {
        let invariant = |kind| {
            invariant(kind)
                .edge(index)
                .source_state(edge.from)
                .target_state(edge.to)
        };
        crate::ssa::checkpoint()?;
        if let Some(reason) = deferred_reason(analysis, edge) {
            deferred_edges.push(DeferredSsaEdge {
                edge: index,
                from: edge.from,
                to: edge.to,
                kind: edge.kind,
                reason,
            });
            blocks[edge.to].open_incoming.push(index);
            blocks[edge.to].incoming_complete = false;
            continue;
        }
        let source = &blocks[edge.from];
        let destination = &analysis.states()[edge.to];
        let mut stacks = source.exit_frames.clone();
        let result = match edge.kind {
            MachineEdgeKind::Intraprocedural(_) => None,
            MachineEdgeKind::Call => {
                stacks.push(Vec::new());
                None
            }
            MachineEdgeKind::Return | MachineEdgeKind::Revert | MachineEdgeKind::Failure => {
                if stacks.len() > destination.entry.call_stack.depth() {
                    stacks.truncate(destination.entry.call_stack.depth());
                }
                let stack = stacks
                    .last_mut()
                    .ok_or_else(|| invariant(Kind::PartialReturnCallerMissing))?;
                stack.push(next);
                let result = next;
                next += 1;
                Some(result)
            }
        };
        let operands = if matches!(edge.kind, MachineEdgeKind::Intraprocedural(_)) {
            Vec::new()
        } else {
            source
                .instructions
                .last()
                .map(|i| i.instruction.operands.clone())
                .unwrap_or_default()
        };
        transitions.push(Transition {
            edge: index,
            kind: edge.kind,
            operands,
            stacks,
            effect_input: source.exit_effect,
            effect_result: effect_next,
            result,
        });
        effect_next += 1;
    }
    let by_edge: BTreeMap<_, _> = transitions.iter().map(|t| (t.edge, t)).collect();
    for (index, edge) in analysis.edges().iter().enumerate() {
        let invariant = |kind| {
            invariant(kind)
                .edge(index)
                .source_state(edge.from)
                .target_state(edge.to)
        };
        crate::ssa::checkpoint()?;
        let Some(transition) = by_edge.get(&index) else {
            continue;
        };
        let block = &mut blocks[edge.to];
        for phi in &mut block.phis {
            let value = transition
                .stacks
                .get(phi.frame)
                .and_then(|s| s.get(phi.slot))
                .copied()
                .ok_or_else(|| {
                    invariant(Kind::PartialDestinationArguments)
                        .state(edge.to)
                        .frame(phi.frame)
                        .slot(phi.slot)
                })?;
            phi.inputs.push((index, value));
        }
        block.effect.inputs.push(EffectInput {
            transition: index,
            effect: transition.effect_result,
        });
    }
    Ok(PartialWorldSsa {
        status: analysis.status(),
        blocks,
        transitions,
        deferred_edges,
        frontiers: analysis.frontiers().to_vec(),
        value_count: next,
        effect_count: effect_next,
    })
}
