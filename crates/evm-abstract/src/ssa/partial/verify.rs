//! Verify each emitted definition against phases, not nominal opcode arity alone.

use super::{
    DeferredSsaEdge, PartialBlockCoverage, PartialWorldSsa, coverage, deferred_reason, invariant,
    is_call,
};
use crate::{
    analysis::{FrameKey, InstructionProgress, MachineEdgeKind, Status, WorldAnalysis},
    ssa::{EffectInput, SsaError, Transition},
};
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn verify(ir: &PartialWorldSsa, analysis: &WorldAnalysis) -> Result<(), SsaError> {
    if ir.status != analysis.status() || ir.blocks.len() != analysis.states().len() {
        return Err(invariant(
            "partial status or state coverage differs from analysis",
        ));
    }
    if ir.frontiers.len() != analysis.frontiers().len()
        || ir.frontiers.iter().zip(analysis.frontiers()).any(|(a, b)| {
            a.from != b.from || a.pc != b.pc || a.reason != b.reason || a.target != b.target
        })
    {
        return Err(invariant(
            "partial SSA lost or changed an analysis frontier",
        ));
    }
    let mut known = BTreeMap::new();
    let mut previous = None;
    for transition in &ir.transitions {
        if transition.edge >= analysis.edges().len()
            || previous.is_some_and(|index| index >= transition.edge)
            || known.insert(transition.edge, transition).is_some()
        {
            return Err(invariant(
                "partial transition IDs are not ordered original edge IDs",
            ));
        }
        previous = Some(transition.edge);
    }
    let mut deferred = Vec::new();
    for (index, edge) in analysis.edges().iter().enumerate() {
        if edge.from >= ir.blocks.len() || edge.to >= ir.blocks.len() {
            return Err(invariant("analysis edge has no partial state"));
        }
        match deferred_reason(analysis, edge) {
            Some(reason) => {
                if known.contains_key(&index) {
                    return Err(invariant(
                        "unsupported historical edge received an SSA transition",
                    ));
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
                return Err(invariant("supported partial transition is missing"));
            }
            None => {}
        }
    }
    if ir.deferred_edges != deferred {
        return Err(invariant(
            "partial deferred edges do not cover the unsupported graph edges",
        ));
    }
    let mut values = BTreeSet::new();
    let mut effects = BTreeSet::new();
    for (id, (block, state)) in ir.blocks.iter().zip(analysis.states()).enumerate() {
        if block.state != id || state.id != id || block.coverage != coverage(state) {
            return Err(invariant("partial state identity or currentness mismatch"));
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
            return Err(invariant(
                "partial phi coverage claims nonexistent incoming evidence",
            ));
        }
        let mut stacks = Vec::new();
        let mut local = BTreeSet::new();
        let mut phi_index = 0;
        for (frame, input) in state.entry.call_stack.iter().enumerate() {
            let mut stack = Vec::new();
            for slot in 0..input.stack.len() {
                let phi = block
                    .phis
                    .get(phi_index)
                    .ok_or_else(|| invariant("missing partial frame parameter"))?;
                phi_index += 1;
                if phi.frame != frame || phi.slot != slot || !values.insert(phi.result) {
                    return Err(invariant(
                        "partial parameter identity or definition mismatch",
                    ));
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
                            .ok_or_else(|| invariant("missing covered edge parameter"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if phi.inputs != expected {
                    return Err(invariant(
                        "partial phi differs from supported edge arguments",
                    ));
                }
            }
            stacks.push(stack);
        }
        if phi_index != block.phis.len() || !effects.insert(block.effect.result) {
            return Err(invariant(
                "extra partial parameters or duplicate entry effect",
            ));
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
            return Err(invariant(
                "partial effect phi differs from supported incoming edges",
            ));
        }
        let mut effect = block.effect.result;
        if block.coverage != PartialBlockCoverage::Current {
            if !block.instructions.is_empty() {
                return Err(invariant(
                    "stale or unexecuted state received an instruction body",
                ));
            }
        } else {
            let evidence = state.execution_evidence().expect("current receipt exists");
            if block.instructions.len() != evidence.instructions().len()
                || block.instructions.len() != state.executed_pcs.len()
            {
                return Err(invariant(
                    "partial instruction count differs from execution receipt",
                ));
            }
            let original = state
                .program()
                .and_then(|p| p.blocks().get(state.active().basic_block_index));
            let stack = stacks
                .last_mut()
                .ok_or_else(|| invariant("partial state has no active frame"))?;
            for (order, step) in block.instructions.iter().enumerate() {
                let source = original
                    .and_then(|b| b.instructions.get(order))
                    .ok_or_else(|| invariant("partial instruction outside captured bytecode"))?;
                let item = &step.instruction;
                if step.progress != evidence.instructions()[order]
                    || item.pc != state.executed_pcs[order]
                    || item.pc != source.pc
                    || item.opcode != source.opcode
                    || item.immediate != source.immediate
                {
                    return Err(invariant(
                        "partial phase or bytecode identity differs from execution",
                    ));
                }
                if order + 1 < block.instructions.len()
                    && step.progress != InstructionProgress::Completed
                {
                    return Err(invariant(
                        "partial prefix continued after an unfinished or dispatched step",
                    ));
                }
                let (inputs, outputs) = source.stack_io();
                let fault = !source.is_valid()
                    || stack.len() < inputs
                    || stack.len() - inputs + outputs > 1024;
                let mut operands = Vec::new();
                match step.progress {
                    InstructionProgress::Started => {
                        if item.fault || !item.results.is_empty() {
                            return Err(invariant(
                                "started instruction invented a fault or result",
                            ));
                        }
                    }
                    InstructionProgress::Faulted => {
                        if !fault || !item.fault || !item.results.is_empty() {
                            return Err(invariant(
                                "faulted prefix does not match its actual fault",
                            ));
                        }
                    }
                    InstructionProgress::OperandsConsumed => {
                        if fault
                            || item.fault
                            || source.immediate.is_some()
                            || (opcode::DUP1..=opcode::SWAP16).contains(&item.opcode)
                            || !item.results.is_empty()
                        {
                            return Err(invariant("pending instruction invented a normal result"));
                        }
                        operands = stack.drain(stack.len() - inputs..).rev().collect();
                    }
                    InstructionProgress::Completed | InstructionProgress::Dispatched => {
                        if fault || item.fault {
                            return Err(invariant("completed prefix hides an actual fault"));
                        }
                        if step.progress == InstructionProgress::Completed && is_call(item.opcode) {
                            return Err(invariant("CALL receipt does not defer its dispatch"));
                        }
                        if step.progress == InstructionProgress::Dispatched
                            && outputs > 0
                            && !is_call(item.opcode)
                        {
                            return Err(invariant("dispatch invented a normal instruction result"));
                        }
                        if (opcode::DUP1..=opcode::DUP16).contains(&item.opcode) {
                            let value =
                                stack[stack.len() - usize::from(item.opcode - opcode::DUP1 + 1)];
                            operands.push(value);
                            stack.push(value);
                            if !item.results.is_empty() {
                                return Err(invariant("partial DUP invented a definition"));
                            }
                        } else if (opcode::SWAP1..=opcode::SWAP16).contains(&item.opcode) {
                            let top = stack.len() - 1;
                            let other = top - usize::from(item.opcode - opcode::SWAP1 + 1);
                            operands.extend([stack[top], stack[other]]);
                            stack.swap(top, other);
                            if !item.results.is_empty() {
                                return Err(invariant("partial SWAP invented a definition"));
                            }
                        } else {
                            operands = stack.drain(stack.len() - inputs..).rev().collect();
                            if item.results.len() != if is_call(item.opcode) { 0 } else { outputs }
                            {
                                return Err(invariant("partial instruction result arity mismatch"));
                            }
                            for value in &item.results {
                                if !values.insert(*value) {
                                    return Err(invariant("duplicate partial value definition"));
                                }
                                local.insert(*value);
                                stack.push(*value);
                            }
                        }
                    }
                }
                if item.operands != operands || item.operands.iter().any(|v| !local.contains(v)) {
                    return Err(invariant(
                        "partial instruction uses violate stack identity or definition order",
                    ));
                }
                if step.effect_input != effect {
                    return Err(invariant("partial instruction effect chain is broken"));
                }
                match (step.progress, step.effect_result) {
                    (InstructionProgress::Started, None) => {}
                    (InstructionProgress::Started, Some(_)) | (_, None) => {
                        return Err(invariant(
                            "partial effect claims the wrong instruction phase",
                        ));
                    }
                    (_, Some(output)) => {
                        if !effects.insert(output) {
                            return Err(invariant("duplicate partial effect definition"));
                        }
                        effect = output;
                    }
                }
            }
            let exit = state
                .exit
                .as_ref()
                .ok_or_else(|| invariant("current partial receipt has no observed exit"))?;
            if stacks.len() != exit.call_stack.depth()
                || stacks
                    .iter()
                    .zip(exit.call_stack.iter())
                    .any(|(names, frame)| names.len() != frame.stack.len())
            {
                return Err(invariant(
                    "partial frame stacks differ from the observed instruction phase",
                ));
            }
        }
        if block.exit_frames != stacks || block.exit_effect != effect {
            return Err(invariant(
                "partial observed stacks or effects differ from its prefix",
            ));
        }
    }
    for transition in &ir.transitions {
        verify_transition(ir, analysis, transition, &mut values, &mut effects)?;
    }
    if values.iter().copied().ne(0..ir.value_count)
        || effects.iter().copied().ne(0..ir.effect_count)
    {
        return Err(invariant(
            "partial SSA definitions are not unique contiguous names",
        ));
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
    if edge.kind != transition.kind {
        return Err(invariant("partial transition kind changed"));
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
                return Err(invariant(
                    "partial local edge changed frame depth or invented a result",
                ));
            }
        }
        MachineEdgeKind::Call => {
            if target_depth != source_depth + 1
                || !last.is_some_and(|i| is_call(i.opcode))
                || transition.result.is_some()
                || to.entry.call_stack.active_child().is_none()
            {
                return Err(invariant(
                    "partial call is not an evidenced suspension and empty child entry",
                ));
            }
            expected.push(Vec::new());
        }
        MachineEdgeKind::Return | MachineEdgeKind::Revert | MachineEdgeKind::Failure => {
            if target_depth != source_depth && target_depth + 1 != source_depth {
                return Err(invariant(
                    "partial return does not match exactly one caller frame",
                ));
            }
            if target_depth == source_depth
                && !(edge.kind == MachineEdgeKind::Failure
                    && last.is_some_and(|i| is_call(i.opcode)))
            {
                return Err(invariant("same-depth partial return is not a failed call"));
            }
            if target_depth < source_depth && from.entry.call_stack.active_child().is_none() {
                return Err(invariant("partial return popped a root frame"));
            }
            expected.truncate(target_depth);
            let result = transition
                .result
                .ok_or_else(|| invariant("partial continuation has no result"))?;
            if !values.insert(result) {
                return Err(invariant("duplicate partial continuation result"));
            }
            expected
                .last_mut()
                .ok_or_else(|| invariant("partial continuation has no caller"))?
                .push(result);
        }
    }
    for frame in 0..source_depth.min(target_depth) {
        if !same_context(&from.key.frames[frame], &to.key.frames[frame]) {
            return Err(invariant(
                "partial transition crosses unrelated frame identities",
            ));
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
        return Err(invariant(
            "partial edge stack arguments differ from its covered frames",
        ));
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
        return Err(invariant(
            "partial call operands or effect dependencies mismatch",
        ));
    }
    Ok(())
}
