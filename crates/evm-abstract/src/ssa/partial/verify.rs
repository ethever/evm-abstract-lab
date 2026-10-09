//! Verify each emitted definition against phases, not nominal opcode arity alone.

use super::{
    DeferredSsaEdge, PartialBlockCoverage, PartialWorldSsa, coverage, deferred_reason, is_call,
};
use crate::ssa::{SsaInvariantKind as Kind, error::invariant};
use crate::{
    analysis::{FrameKey, InstructionProgress, MachineEdgeKind, Status, WorldAnalysis},
    ssa::{EffectInput, SsaError, Transition},
};
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn verify(ir: &PartialWorldSsa, analysis: &WorldAnalysis) -> Result<(), SsaError> {
    crate::ssa::checkpoint()?;
    if ir.status != analysis.status() || ir.blocks.len() != analysis.states().len() {
        return Err(invariant(Kind::PartialStatusOrBlockCount));
    }
    if ir.frontiers.len() != analysis.frontiers().len()
        || ir.frontiers.iter().zip(analysis.frontiers()).any(|(a, b)| {
            a.from != b.from || a.pc != b.pc || a.reason != b.reason || a.target != b.target
        })
    {
        return Err(invariant(Kind::FrontierCoverage));
    }
    let mut known = BTreeMap::new();
    let mut previous = None;
    for transition in &ir.transitions {
        crate::ssa::checkpoint()?;
        if transition.edge >= analysis.edges().len()
            || previous.is_some_and(|index| index >= transition.edge)
            || known.insert(transition.edge, transition).is_some()
        {
            return Err(invariant(Kind::PartialTransitionOrder).edge(transition.edge));
        }
        previous = Some(transition.edge);
    }
    let mut deferred = Vec::new();
    for (index, edge) in analysis.edges().iter().enumerate() {
        let invariant = |kind| {
            invariant(kind)
                .edge(index)
                .source_state(edge.from)
                .target_state(edge.to)
        };
        crate::ssa::checkpoint()?;
        if edge.from >= ir.blocks.len() || edge.to >= ir.blocks.len() {
            return Err(invariant(Kind::EdgeStateMissing));
        }
        match deferred_reason(analysis, edge) {
            Some(reason) => {
                if known.contains_key(&index) {
                    return Err(invariant(Kind::UnsupportedTransition));
                }
                deferred.push(DeferredSsaEdge {
                    edge: index,
                    from: edge.from,
                    to: edge.to,
                    kind: edge.kind,
                    reason,
                });
            }
            None if !known.contains_key(&index) => {
                return Err(invariant(Kind::TransitionMissing));
            }
            None => {}
        }
    }
    if ir.deferred_edges != deferred {
        return Err(invariant(Kind::DeferredEdgeCoverage));
    }
    let mut values = BTreeSet::new();
    let mut effects = BTreeSet::new();
    for (id, (block, state)) in ir.blocks.iter().zip(analysis.states()).enumerate() {
        let invariant = |kind| invariant(kind).state(id);
        crate::ssa::checkpoint()?;
        if block.state != id || state.id != id || block.coverage != coverage(state) {
            return Err(invariant(Kind::PartialStateIdentity));
        }
        let incoming: Vec<_> = analysis
            .edges()
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| (edge.to == id).then_some(index))
            .collect();
        let open: Vec<_> = incoming
            .iter()
            .copied()
            .filter(|i| !known.contains_key(i))
            .collect();
        if block.open_incoming != open
            || block.incoming_complete
                != (analysis.status() == Status::Converged && open.is_empty())
        {
            return Err(invariant(Kind::PartialPhiCoverage));
        }
        let mut stacks = Vec::new();
        let mut local = BTreeSet::new();
        let mut phi_index = 0;
        for (frame, input) in state.entry.call_stack.iter().enumerate() {
            let mut stack = Vec::new();
            for slot in 0..input.stack.len() {
                let invariant = |kind| invariant(kind).frame(frame).slot(slot);
                let phi = block
                    .phis
                    .get(phi_index)
                    .ok_or_else(|| invariant(Kind::PartialFrameParameterMissing))?;
                phi_index += 1;
                if phi.frame != frame || phi.slot != slot || !values.insert(phi.result) {
                    return Err(invariant(Kind::PartialParameterIdentity).value(phi.result));
                }
                local.insert(phi.result);
                stack.push(phi.result);
                let expected = incoming
                    .iter()
                    .filter_map(|i| known.get(i).map(|t| (*i, *t)))
                    .map(|(i, t)| {
                        t.stacks
                            .get(frame)
                            .and_then(|s| s.get(slot))
                            .copied()
                            .map(|value| (i, value))
                            .ok_or_else(|| invariant(Kind::CoveredEdgeParameterMissing).edge(i))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if phi.inputs != expected {
                    return Err(invariant(Kind::PartialPhiArguments));
                }
            }
            stacks.push(stack);
        }
        if phi_index != block.phis.len() || !effects.insert(block.effect.result) {
            return Err(invariant(Kind::PartialParameterOrEffect));
        }
        let expected_effects: Vec<_> = incoming
            .iter()
            .filter_map(|i| {
                known.get(i).map(|t| EffectInput {
                    transition: *i,
                    effect: t.effect_result,
                })
            })
            .collect();
        if block.effect.inputs != expected_effects {
            return Err(invariant(Kind::PartialEffectPhiCoverage));
        }
        let mut effect = block.effect.result;
        if block.coverage != PartialBlockCoverage::Current {
            if !block.instructions.is_empty() {
                return Err(invariant(Kind::UnexecutedBody));
            }
        } else {
            let evidence = state.execution_evidence().expect("current receipt exists");
            if block.instructions.len() != evidence.instructions().len()
                || block.instructions.len() != state.executed_pcs.len()
            {
                return Err(invariant(Kind::PartialInstructionCount));
            }
            let original = state
                .program()
                .and_then(|p| p.blocks().get(state.active().basic_block_index));
            let stack = stacks
                .last_mut()
                .ok_or_else(|| invariant(Kind::PartialActiveFrameMissing))?;
            for (order, step) in block.instructions.iter().enumerate() {
                crate::ssa::checkpoint()?;
                let invariant = |kind| invariant(kind).pc(step.instruction.pc);
                let source = original
                    .and_then(|b| b.instructions.get(order))
                    .ok_or_else(|| invariant(Kind::PartialInstructionOutsideRuntime))?;
                let item = &step.instruction;
                if step.progress != evidence.instructions()[order]
                    || item.pc != state.executed_pcs[order]
                    || item.pc != source.pc
                    || item.opcode != source.opcode
                    || item.immediate != source.immediate
                {
                    return Err(invariant(Kind::PartialInstructionIdentity));
                }
                if order + 1 < block.instructions.len()
                    && step.progress != InstructionProgress::Completed
                {
                    return Err(invariant(Kind::UnfinishedPrefix));
                }
                let (inputs, outputs) = source.stack_io();
                let fault = !source.is_valid()
                    || stack.len() < inputs
                    || stack.len() - inputs + outputs > 1024;
                let mut operands = Vec::new();
                match step.progress {
                    InstructionProgress::Started => {
                        if item.fault || !item.results.is_empty() {
                            return Err(invariant(Kind::StartedResult));
                        }
                    }
                    InstructionProgress::Faulted => {
                        if !fault || !item.fault || !item.results.is_empty() {
                            return Err(invariant(Kind::FaultedPrefix));
                        }
                    }
                    InstructionProgress::OperandsConsumed => {
                        if fault
                            || item.fault
                            || source.immediate.is_some()
                            || (opcode::DUP1..=opcode::SWAP16).contains(&item.opcode)
                            || !item.results.is_empty()
                        {
                            return Err(invariant(Kind::PendingResult));
                        }
                        operands = stack.drain(stack.len() - inputs..).rev().collect();
                    }
                    InstructionProgress::Completed | InstructionProgress::Dispatched => {
                        if fault || item.fault {
                            return Err(invariant(Kind::CompletedFault));
                        }
                        if step.progress == InstructionProgress::Completed && is_call(item.opcode) {
                            return Err(invariant(Kind::CallDispatch));
                        }
                        if step.progress == InstructionProgress::Dispatched
                            && outputs > 0
                            && !is_call(item.opcode)
                        {
                            return Err(invariant(Kind::DispatchResult));
                        }
                        if (opcode::DUP1..=opcode::DUP16).contains(&item.opcode) {
                            let value =
                                stack[stack.len() - usize::from(item.opcode - opcode::DUP1 + 1)];
                            operands.push(value);
                            stack.push(value);
                            if !item.results.is_empty() {
                                return Err(invariant(Kind::PartialDupResult));
                            }
                        } else if (opcode::SWAP1..=opcode::SWAP16).contains(&item.opcode) {
                            let top = stack.len() - 1;
                            let other = top - usize::from(item.opcode - opcode::SWAP1 + 1);
                            operands.extend([stack[top], stack[other]]);
                            stack.swap(top, other);
                            if !item.results.is_empty() {
                                return Err(invariant(Kind::PartialSwapResult));
                            }
                        } else {
                            operands = stack.drain(stack.len() - inputs..).rev().collect();
                            if item.results.len() != if is_call(item.opcode) { 0 } else { outputs }
                            {
                                return Err(invariant(Kind::PartialInstructionArity)
                                    .expected(if is_call(item.opcode) { 0 } else { outputs })
                                    .observed(item.results.len()));
                            }
                            for value in &item.results {
                                if !values.insert(*value) {
                                    return Err(
                                        invariant(Kind::DuplicatePartialValue).value(*value)
                                    );
                                }
                                local.insert(*value);
                                stack.push(*value);
                            }
                        }
                    }
                }
                if item.operands != operands || item.operands.iter().any(|v| !local.contains(v)) {
                    return Err(invariant(Kind::PartialOperandIdentity));
                }
                if step.effect_input != effect {
                    return Err(invariant(Kind::PartialInstructionEffectChain)
                        .expected(effect)
                        .observed(step.effect_input));
                }
                match (step.progress, step.effect_result) {
                    (InstructionProgress::Started, None) => {}
                    (InstructionProgress::Started, Some(_)) | (_, None) => {
                        return Err(invariant(Kind::PartialEffectPhase));
                    }
                    (_, Some(output)) => {
                        if !effects.insert(output) {
                            return Err(invariant(Kind::DuplicatePartialEffect).effect(output));
                        }
                        effect = output;
                    }
                }
            }
            let exit = state
                .exit
                .as_ref()
                .ok_or_else(|| invariant(Kind::CurrentExitMissing))?;
            if stacks.len() != exit.call_stack.depth()
                || stacks
                    .iter()
                    .zip(exit.call_stack.iter())
                    .any(|(names, frame)| names.len() != frame.stack.len())
            {
                return Err(invariant(Kind::PartialExitFrameShape));
            }
        }
        if block.exit_frames != stacks || block.exit_effect != effect {
            return Err(invariant(Kind::PartialExitStackOrEffect));
        }
    }
    for transition in &ir.transitions {
        crate::ssa::checkpoint()?;
        verify_transition(ir, analysis, transition, &mut values, &mut effects)?;
    }
    if values.iter().copied().ne(0..ir.value_count)
        || effects.iter().copied().ne(0..ir.effect_count)
    {
        return Err(invariant(Kind::PartialContiguousNames));
    }
    Ok(())
}

fn same_context(a: &FrameKey, b: &FrameKey) -> bool {
    a.code_address == b.code_address
        && a.code_hash == b.code_hash
        && a.mode == b.mode
        && a.address == b.address
        && a.address_value == b.address_value
        && a.caller == b.caller
        && a.is_static == b.is_static
}

fn verify_transition(
    ir: &PartialWorldSsa,
    analysis: &WorldAnalysis,
    transition: &Transition,
    values: &mut BTreeSet<usize>,
    effects: &mut BTreeSet<usize>,
) -> Result<(), SsaError> {
    let edge = &analysis.edges()[transition.edge];
    let invariant = |kind| {
        invariant(kind)
            .edge(transition.edge)
            .source_state(edge.from)
            .target_state(edge.to)
    };
    if edge.kind != transition.kind {
        return Err(invariant(Kind::PartialTransitionKind));
    }
    let source = &ir.blocks[edge.from];
    let from = &analysis.states()[edge.from];
    let to = &analysis.states()[edge.to];
    let mut expected = source.exit_frames.clone();
    let source_depth = expected.len();
    let target_depth = to.entry.call_stack.depth();
    let last = source.instructions.last().map(|s| &s.instruction);
    match edge.kind {
        MachineEdgeKind::Intraprocedural(_) => {
            if target_depth != source_depth || transition.result.is_some() {
                return Err(invariant(Kind::PartialLocalEdgeDepth)
                    .expected(source_depth)
                    .observed(target_depth));
            }
        }
        MachineEdgeKind::Call => {
            if target_depth != source_depth + 1
                || !last.is_some_and(|i| is_call(i.opcode))
                || transition.result.is_some()
                || to.entry.call_stack.active_child().is_none()
            {
                return Err(invariant(Kind::PartialCallSuspension));
            }
            expected.push(Vec::new());
        }
        MachineEdgeKind::Return | MachineEdgeKind::Revert | MachineEdgeKind::Failure => {
            if target_depth != source_depth && target_depth + 1 != source_depth {
                return Err(invariant(Kind::PartialReturnDepth));
            }
            if target_depth == source_depth
                && !(edge.kind == MachineEdgeKind::Failure
                    && last.is_some_and(|i| is_call(i.opcode)))
            {
                return Err(invariant(Kind::PartialSameDepthReturn));
            }
            if target_depth < source_depth && from.entry.call_stack.active_child().is_none() {
                return Err(invariant(Kind::RootReturn));
            }
            expected.truncate(target_depth);
            let result = transition
                .result
                .ok_or_else(|| invariant(Kind::PartialContinuationResultMissing))?;
            if !values.insert(result) {
                return Err(invariant(Kind::DuplicatePartialContinuationValue).value(result));
            }
            expected
                .last_mut()
                .ok_or_else(|| invariant(Kind::PartialCallerMissing))?
                .push(result);
        }
    }
    for frame in 0..source_depth.min(target_depth) {
        if !same_context(&from.key.frames[frame], &to.key.frames[frame]) {
            return Err(invariant(Kind::PartialFrameIdentity).frame(frame));
        }
    }
    if transition.stacks != expected
        || transition.stacks.len() != target_depth
        || transition
            .stacks
            .iter()
            .zip(to.entry.call_stack.iter())
            .any(|(s, frame)| s.len() != frame.stack.len())
    {
        return Err(invariant(Kind::PartialEdgeArguments));
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
        return Err(invariant(Kind::PartialTransitionDependencies));
    }
    Ok(())
}
