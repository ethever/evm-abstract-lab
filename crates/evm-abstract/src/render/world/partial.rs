//! Recorded SSA progress keeps open paths separate from completed graph proofs.
//!
//! State and transition names refer to the original native machine graph.
//! Pending instructions retain their recorded phase without manufacturing a
//! normal result or an unobserved effect; unexecuted nodes contain no TAC.

use evm_abstract_notation::Symbol;

use crate::{
    analysis::{InstructionProgress, WorldAnalysis},
    render::instruction::{InstructionLayout, write_ssa_body},
    ssa::{PartialBlockCoverage, PartialWorldSsa},
};
use revm_bytecode::opcode;
use std::fmt::Write;

pub(super) fn render(analysis: &WorldAnalysis, ir: &PartialWorldSsa) -> String {
    render_with_mode(analysis, ir, false)
}

pub(super) fn render_verbose(analysis: &WorldAnalysis, ir: &PartialWorldSsa) -> String {
    render_with_mode(analysis, ir, true)
}

fn render_with_mode(analysis: &WorldAnalysis, ir: &PartialWorldSsa, verbose: bool) -> String {
    let mut output = String::from("Partial SSA (machine state IDs):\n");
    write!(
        output,
        "  status={:?} | states={} | known transitions={} | deferred transitions={} | stack values={}",
        ir.status(),
        ir.blocks().len(),
        ir.transitions().len(),
        ir.deferred_edges().len(),
        ir.value_count(),
    )
    .unwrap();
    if verbose {
        write!(output, " | effect bundles={}", ir.effect_count()).unwrap();
    }
    output.push('\n');
    writeln!(
        output,
        "  coverage: current={} | unexecuted={} | stale={} | frontiers={}",
        ir.blocks()
            .iter()
            .filter(|block| matches!(block.coverage, PartialBlockCoverage::Current))
            .count(),
        ir.blocks()
            .iter()
            .filter(|block| matches!(block.coverage, PartialBlockCoverage::Unexecuted))
            .count(),
        ir.blocks()
            .iter()
            .filter(|block| matches!(block.coverage, PartialBlockCoverage::Stale))
            .count(),
        ir.frontiers().len(),
    )
    .unwrap();
    output.push_str(
        "  σᵢ = native machine state; Tᵢ = original machine edge; %value = stack name.\n",
    );
    output.push_str(
        "  Only recorded prefixes and edges are shown; incoming paths may still be incomplete.\n",
    );
    if verbose {
        output
            .push_str("  μᵢ = recorded machine effects; progress = observed instruction phase.\n");
        output.push_str("  Current = evidence for the current joined entry; Stale = evidence for an earlier entry; Unexecuted = no transfer receipt.\n");
    }
    output.push_str("\nBlocks\n");
    for block in ir.blocks() {
        let state = &analysis.states()[block.state];
        let frame = state.active();
        write!(output, "{}", Symbol::State(block.state)).unwrap();
        if verbose || block.coverage != PartialBlockCoverage::Current {
            write!(output, " | coverage={:?}", block.coverage).unwrap();
        }
        if verbose {
            write!(output, " | incoming complete={}", block.incoming_complete).unwrap();
        }
        if verbose || !block.open_incoming.is_empty() {
            write!(
                output,
                " | open incoming=[{}]",
                block
                    .open_incoming
                    .iter()
                    .map(|edge| Symbol::Edge(*edge).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }
        output.push('\n');
        writeln!(
            output,
            "  active={} | depth={} | mode={:?} | code={} | storage owner={}",
            Symbol::Frame(state.key.frames.len() - 1),
            state.key.frames.len(),
            frame.mode,
            super::environment::owner(analysis, frame.code_address),
            frame.address_value
        )
        .unwrap();
        for phi in &block.phis {
            let inputs = phi
                .inputs
                .iter()
                .map(|(edge, value)| format!("{}: %{value}", Symbol::Transition(*edge)))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "    %{} = partial φ({inputs}) ; {} slot {}",
                phi.result,
                Symbol::Frame(phi.frame),
                phi.slot
            )
            .unwrap();
        }
        if verbose {
            let inputs = block
                .effect
                .inputs
                .iter()
                .map(|input| {
                    format!(
                        "{}: {}",
                        Symbol::Transition(input.transition),
                        Symbol::Effect(input.effect)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "    {} = partial effect φ({inputs})",
                Symbol::Effect(block.effect.result)
            )
            .unwrap();
        }
        let layout = state
            .program()
            .and_then(|program| program.blocks().get(frame.basic_block_index))
            .filter(|_| !block.instructions.is_empty())
            .map(|source| InstructionLayout::block(&mut output, source.id, source.start_pc));
        if block.instructions.is_empty()
            && (verbose || block.coverage != PartialBlockCoverage::Current)
        {
            output.push_str("    (no bytecode instruction evidence)\n");
        }
        for item in &block.instructions {
            let layout = layout.as_ref().expect("recorded instruction has bytecode");
            layout.write_pc(&mut output, item.instruction.pc);
            write_ssa_body(&mut output, &item.instruction);
            let exceptional_dispatch = if item.progress == InstructionProgress::Dispatched {
                exceptional_dispatch_note(item.instruction.opcode, frame.is_static)
            } else {
                None
            };
            if verbose
                || matches!(
                    item.progress,
                    InstructionProgress::Started
                        | InstructionProgress::OperandsConsumed
                        | InstructionProgress::Faulted
                )
            {
                write!(output, " ; progress={:?}", item.progress).unwrap();
                match item.progress {
                    InstructionProgress::Started => {
                        output.push_str("; no operands consumed or normal result")
                    }
                    InstructionProgress::OperandsConsumed => output.push_str(
                        "; pending operation, no normal result; state may be partially updated",
                    ),
                    InstructionProgress::Completed => {}
                    InstructionProgress::Dispatched if exceptional_dispatch.is_none() => {
                        output.push_str(dispatch_note(
                            item.instruction.opcode,
                            state.key.frames.len(),
                        ));
                    }
                    InstructionProgress::Dispatched => {}
                    InstructionProgress::Faulted => {
                        output.push_str("; recorded opcode/stack fault")
                    }
                }
            }
            if let Some(note) = exceptional_dispatch {
                output.push_str(note);
            }
            for (index, frontier) in ir.frontiers().iter().enumerate().filter(|(_, frontier)| {
                frontier.from == Some(block.state) && frontier.pc == Some(item.instruction.pc)
            }) {
                write!(
                    output,
                    " ; frontier {}: {:?}",
                    Symbol::Frontier(index),
                    frontier.reason
                )
                .unwrap();
            }
            output.push('\n');
            if verbose {
                layout.indent(&mut output);
                if let Some(effect) = item.effect_result {
                    let label = if item.progress == InstructionProgress::OperandsConsumed {
                        "observed partial effect"
                    } else {
                        "effect"
                    };
                    writeln!(
                        output,
                        "  {label} {} → {}",
                        Symbol::Effect(item.effect_input),
                        Symbol::Effect(effect)
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "  effect {} remains open",
                        Symbol::Effect(item.effect_input)
                    )
                    .unwrap();
                }
            }
        }
        let current = block.coverage == PartialBlockCoverage::Current;
        let stack_label = if current {
            "recorded prefix stack"
        } else {
            "entry stack parameters (no current exit)"
        };
        write!(output, "    {stack_label}:").unwrap();
        for (frame, stack) in block.exit_frames.iter().enumerate() {
            let values = stack
                .iter()
                .map(|value| format!("%{value}"))
                .collect::<Vec<_>>()
                .join(", ");
            write!(output, " {}: [{values}]", Symbol::Frame(frame)).unwrap();
        }
        output.push('\n');
        if verbose {
            let effect_label = if current {
                "recorded prefix effect"
            } else {
                "entry effect (no current exit)"
            };
            writeln!(
                output,
                "    {effect_label}: {}",
                Symbol::Effect(block.exit_effect)
            )
            .unwrap();
        }
    }
    if verbose || !ir.transitions().is_empty() {
        output.push_str("\nKnown transitions\n");
    }
    for transition in ir.transitions() {
        let edge = &analysis.edges()[transition.edge];
        write!(
            output,
            "  {} | {} → {} | kind={:?}",
            Symbol::Transition(transition.edge),
            Symbol::State(edge.from),
            Symbol::State(edge.to),
            transition.kind
        )
        .unwrap();
        if verbose {
            write!(
                output,
                " | effect {} → {}",
                Symbol::Effect(transition.effect_input),
                Symbol::Effect(transition.effect_result)
            )
            .unwrap();
        }
        output.push('\n');
        if let Some(value) = transition.result {
            writeln!(output, "    deferred result: %{value}").unwrap();
        }
        for (frame, stack) in transition.stacks.iter().enumerate() {
            let values = stack
                .iter()
                .map(|value| format!("%{value}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(output, "    {}: [{values}]", Symbol::Frame(frame)).unwrap();
        }
    }
    if verbose || !ir.deferred_edges().is_empty() {
        output.push_str("\nDeferred edges\n");
    }
    for edge in ir.deferred_edges() {
        writeln!(
            output,
            "  {} | {} → {} | kind={:?} | reason={:?}",
            Symbol::Transition(edge.edge),
            Symbol::State(edge.from),
            Symbol::State(edge.to),
            edge.kind,
            edge.reason
        )
        .unwrap();
    }
    if verbose || !ir.frontiers().is_empty() {
        output.push_str("\nFrontiers retained\n");
    }
    for (index, frontier) in ir.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  {} | from={} | pc={:?} | reason={:?}",
            Symbol::Frontier(index),
            frontier.from.map_or_else(
                || "none".to_owned(),
                |state| Symbol::State(state).to_string()
            ),
            frontier.pc,
            frontier.reason
        )
        .unwrap();
        if let Some(target) = &frontier.target {
            writeln!(output, "    target code identity={}", target.code_identity).unwrap();
            for (frame, key) in target.frames.iter().enumerate() {
                writeln!(output, "      {} | {} | mode={:?} | stack height={} | code={} | code hash={} | storage owner={} | address={} | caller={} | static={} | context={:?}",
                    Symbol::Frame(frame), Symbol::Block(key.basic_block_index), key.mode,
                    key.stack_height, key.code_address, key.code_hash, key.address_value,
                    key.address, key.caller, key.is_static, key.jump_history).unwrap();
            }
        } else {
            output.push_str("    target=none\n");
        }
    }
    output
}

/// These dispatches terminate abnormally in transfer; they are not ordinary
/// call/branch dispatches, even though they share the same recorded phase.
fn exceptional_dispatch_note(op: u8, is_static: bool) -> Option<&'static str> {
    if is_static
        && matches!(
            op,
            opcode::SSTORE
                | opcode::TSTORE
                | opcode::CREATE
                | opcode::CREATE2
                | opcode::SELFDESTRUCT
                | opcode::LOG0..=opcode::LOG4
        )
    {
        Some(" ; exceptional halt: state change in static frame")
    } else if op == opcode::RETURNDATACOPY {
        Some(" ; exceptional halt: return data out of bounds")
    } else {
        None
    }
}

fn dispatch_note(op: u8, depth: usize) -> &'static str {
    match op {
        opcode::CALL
        | opcode::CALLCODE
        | opcode::DELEGATECALL
        | opcode::STATICCALL
        | opcode::CREATE
        | opcode::CREATE2 => "; call dispatch; inspect recorded continuations",
        opcode::JUMP | opcode::JUMPI => "; branch dispatch; inspect recorded transitions",
        opcode::RETURN | opcode::REVERT if depth == 1 => {
            "; return dispatch; inspect recorded outcomes"
        }
        opcode::RETURN | opcode::REVERT => {
            "; return dispatch; inspect recorded caller continuations"
        }
        _ if depth == 1 => "; termination dispatch; inspect recorded outcomes",
        _ => "; termination dispatch; inspect recorded caller continuations",
    }
}
