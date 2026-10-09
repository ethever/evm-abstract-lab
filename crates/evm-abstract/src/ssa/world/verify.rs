//! Edge arguments are uses at the end of the source block. This formulation
//! admits loop phis while forbidding values from unrelated calling contexts.

use super::{WorldSsa, is_call};
use crate::ssa::{SsaInvariantKind as Kind, error::invariant};
use crate::{
    analysis::{FrameKey, MachineEdgeKind, Status, WorldAnalysis},
    ssa::SsaError,
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

fn same_context(a: &FrameKey, b: &FrameKey) -> bool {
    a.code_address == b.code_address
        && a.address == b.address
        && a.caller == b.caller
        && a.is_static == b.is_static
        && a.code_hash == b.code_hash
        && a.mode == b.mode
}

pub(super) fn verify(ir: &WorldSsa, analysis: &WorldAnalysis) -> Result<(), SsaError> {
    crate::ssa::checkpoint()?;
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    if ir.blocks.len() != analysis.states().len() || ir.transitions.len() != analysis.edges().len()
    {
        return Err(invariant(Kind::WorldGraphSize));
    }
    let mut values = BTreeSet::new();
    let mut effects = BTreeSet::new();
    for (id, (block, state)) in ir.blocks.iter().zip(analysis.states()).enumerate() {
        let invariant = |kind| invariant(kind).state(id);
        crate::ssa::checkpoint()?;
        if block.state != id || state.id != id {
            return Err(invariant(Kind::MachineStateIdentity));
        }
        let mut stacks = vec![Vec::new(); state.entry.call_stack.depth()];
        let mut local = BTreeSet::new();
        let incoming: Vec<_> = analysis
            .edges()
            .iter()
            .enumerate()
            .filter_map(|(i, e)| (e.to == id).then_some(i))
            .collect();
        let mut phi_index = 0;
        for (frame, item) in state.entry.call_stack.iter().enumerate() {
            for slot in 0..item.stack.len() {
                let invariant = |kind| invariant(kind).frame(frame).slot(slot);
                let phi = block
                    .phis
                    .get(phi_index)
                    .ok_or_else(|| invariant(Kind::FrameParameterMissing))?;
                phi_index += 1;
                if phi.frame != frame || phi.slot != slot || !values.insert(phi.result) {
                    return Err(invariant(Kind::FrameParameterIdentity).value(phi.result));
                }
                local.insert(phi.result);
                stacks[frame].push(phi.result);
                let expected = incoming
                    .iter()
                    .map(|i| {
                        ir.transitions[*i]
                            .stacks
                            .get(frame)
                            .and_then(|s| s.get(slot))
                            .copied()
                            .map(|v| (*i, v))
                            .ok_or_else(|| invariant(Kind::EdgeArgumentMissing).edge(*i))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if phi.inputs != expected {
                    return Err(invariant(Kind::FramePhiArguments));
                }
            }
        }
        if phi_index != block.phis.len() {
            return Err(invariant(Kind::ExtraFrameParameters));
        }
        if !effects.insert(block.effect.result) {
            return Err(invariant(Kind::DuplicateEffect).effect(block.effect.result));
        }
        let expected_effects: Vec<_> = incoming
            .iter()
            .map(|i| super::EffectInput {
                transition: *i,
                effect: ir.transitions[*i].effect_result,
            })
            .collect();
        if block.effect.inputs != expected_effects {
            return Err(invariant(Kind::EffectPhiCoverage));
        }
        let program = state.program();
        let original = program.and_then(|p| p.blocks().get(state.active().basic_block_index));
        if block.instructions.len() != state.executed_pcs.len()
            || block.effects.len() != block.instructions.len()
        {
            return Err(invariant(Kind::ExecutionEffectCount));
        }
        let mut effect = block.effect.result;
        let stack = stacks
            .last_mut()
            .ok_or_else(|| invariant(Kind::FrameMissing))?;
        for (order, item) in block.instructions.iter().enumerate() {
            crate::ssa::checkpoint()?;
            let invariant = |kind| invariant(kind).pc(item.pc);
            let source = original
                .and_then(|b| b.instructions.get(order))
                .ok_or_else(|| invariant(Kind::InstructionOutsideRuntime))?;
            if item.pc != state.executed_pcs[order]
                || item.pc != source.pc
                || item.opcode != source.opcode
                || item.immediate != source.immediate
            {
                return Err(invariant(Kind::RuntimeInstructionIdentity));
            }
            let (inputs, outputs) = source.stack_io();
            let fault =
                !source.is_valid() || stack.len() < inputs || stack.len() - inputs + outputs > 1024;
            if item.fault != fault {
                return Err(invariant(Kind::InstructionFault));
            }
            let mut operands = Vec::new();
            if !fault {
                if (opcode::DUP1..=opcode::DUP16).contains(&item.opcode) {
                    let value = stack[stack.len() - usize::from(item.opcode - opcode::DUP1 + 1)];
                    operands.push(value);
                    stack.push(value);
                    if !item.results.is_empty() {
                        return Err(invariant(Kind::DupResult));
                    }
                } else if (opcode::SWAP1..=opcode::SWAP16).contains(&item.opcode) {
                    let top = stack.len() - 1;
                    let other = top - usize::from(item.opcode - opcode::SWAP1 + 1);
                    operands.extend([stack[top], stack[other]]);
                    stack.swap(top, other);
                    if !item.results.is_empty() {
                        return Err(invariant(Kind::SwapResult));
                    }
                } else {
                    operands = stack.drain(stack.len() - inputs..).rev().collect();
                    if item.results.len() != if is_call(item.opcode) { 0 } else { outputs } {
                        return Err(invariant(Kind::InstructionResultArity)
                            .expected(if is_call(item.opcode) { 0 } else { outputs })
                            .observed(item.results.len()));
                    }
                    for value in &item.results {
                        if !values.insert(*value) {
                            return Err(invariant(Kind::DuplicateStackValue).value(*value));
                        }
                        local.insert(*value);
                        stack.push(*value);
                    }
                }
            } else if !item.results.is_empty() {
                return Err(invariant(Kind::FaultResult));
            }
            if item.operands != operands || item.operands.iter().any(|v| !local.contains(v)) {
                return Err(invariant(Kind::OperandIdentity));
            }
            let (pc, input, output) = block.effects[order];
            if pc != item.pc || input != effect || !effects.insert(output) {
                return Err(invariant(Kind::InstructionEffectChain)
                    .effect(output)
                    .expected(effect)
                    .observed(input));
            }
            effect = output;
        }
        if block.exit_effect != effect || block.exit_frames != stacks {
            return Err(invariant(Kind::ExitStackOrEffect));
        }
        if let Some(exit) = &state.exit
            && (block.exit_frames.len() != exit.call_stack.depth()
                || block
                    .exit_frames
                    .iter()
                    .zip(exit.call_stack.iter())
                    .any(|(s, f)| s.len() != f.stack.len()))
        {
            return Err(invariant(Kind::ExitFrameShape));
        }
    }
    for (index, (transition, edge)) in ir.transitions.iter().zip(analysis.edges()).enumerate() {
        let invariant = |kind| {
            invariant(kind)
                .edge(index)
                .source_state(edge.from)
                .target_state(edge.to)
        };
        crate::ssa::checkpoint()?;
        if transition.edge != index || transition.kind != edge.kind {
            return Err(invariant(Kind::TransitionIdentity)
                .expected(index)
                .observed(transition.edge));
        }
        let source = &ir.blocks[edge.from];
        let from = &analysis.states()[edge.from];
        let to = &analysis.states()[edge.to];
        let mut expected = source.exit_frames.clone();
        let source_depth = expected.len();
        let target_depth = to.key.frames.len();
        let last = source.instructions.last();
        match edge.kind {
            MachineEdgeKind::Intraprocedural(_) => {
                if target_depth != source_depth || transition.result.is_some() {
                    return Err(invariant(Kind::LocalEdgeDepth)
                        .expected(source_depth)
                        .observed(target_depth));
                }
            }
            MachineEdgeKind::Call => {
                if target_depth != source_depth + 1
                    || !last.is_some_and(|i| is_call(i.opcode))
                    || transition.result.is_some()
                {
                    return Err(invariant(Kind::CallSuspension));
                }
                if to.entry.call_stack.active_child().is_none() {
                    return Err(invariant(Kind::CallChildFrame));
                }
                expected.push(Vec::new());
            }
            MachineEdgeKind::Return | MachineEdgeKind::Revert | MachineEdgeKind::Failure => {
                if target_depth != source_depth && target_depth + 1 != source_depth {
                    return Err(invariant(Kind::ReturnDepth));
                }
                if target_depth == source_depth
                    && !(edge.kind == MachineEdgeKind::Failure
                        && last.is_some_and(|i| is_call(i.opcode)))
                {
                    return Err(invariant(Kind::SameDepthReturn));
                }
                if target_depth + 1 == source_depth
                    && from.entry.call_stack.active_child().is_none()
                {
                    return Err(invariant(Kind::ReturnChildFrame));
                }
                expected.truncate(target_depth);
                let result = transition
                    .result
                    .ok_or_else(|| invariant(Kind::ContinuationResultMissing))?;
                if !values.insert(result) {
                    return Err(invariant(Kind::DuplicateContinuationValue).value(result));
                }
                expected
                    .last_mut()
                    .ok_or_else(|| invariant(Kind::ReturnWithoutCaller))?
                    .push(result);
            }
        }
        // Suspended stacks cannot cross between proxies or unrelated callers.
        let retained = source_depth.min(target_depth);
        for frame in 0..retained {
            if !same_context(&from.key.frames[frame], &to.key.frames[frame]) {
                return Err(invariant(Kind::FrameIdentity).frame(frame));
            }
        }
        if transition.stacks != expected
            || transition.stacks.len() != to.entry.call_stack.depth()
            || transition
                .stacks
                .iter()
                .zip(to.entry.call_stack.iter())
                .any(|(s, f)| s.len() != f.stack.len())
        {
            return Err(invariant(Kind::TransitionArguments));
        }
        let operands = if matches!(edge.kind, MachineEdgeKind::Intraprocedural(_)) {
            Vec::new()
        } else {
            last.map(|i| i.operands.clone()).unwrap_or_default()
        };
        if transition.operands != operands
            || transition.effect_input != source.exit_effect
            || !effects.insert(transition.effect_result)
        {
            return Err(invariant(Kind::TransitionDependencies));
        }
    }
    if values.len() != ir.value_count
        || values.iter().copied().ne(0..ir.value_count)
        || effects.len() != ir.effect_count
        || effects.iter().copied().ne(0..ir.effect_count)
    {
        return Err(invariant(Kind::ContiguousNames));
    }
    Ok(())
}
