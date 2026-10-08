//! Recorded SSA progress keeps open paths separate from completed graph proofs.
//!
//! State and transition names refer to the original native machine graph.
//! Pending instructions retain their recorded phase without manufacturing a
//! normal result or an unobserved effect; unexecuted nodes contain no TAC.

use crate::{
    analysis::{InstructionProgress, WorldAnalysis},
    render::instruction::{InstructionLayout, write_ssa_body},
    ssa::{PartialBlockCoverage, PartialWorldSsa},
};
use std::fmt::Write;

pub(super) fn render(analysis: &WorldAnalysis, ir: &PartialWorldSsa) -> String {
    let mut output = String::from("Partial SSA (machine state IDs):\n");
    writeln!(
        output,
        "  status={:?} | states={} | known transitions={} | deferred transitions={} | stack values={} | effect bundles={}",
        ir.status(),
        ir.blocks().len(),
        ir.transitions().len(),
        ir.deferred_edges().len(),
        ir.value_count(),
        ir.effect_count(),
    )
    .unwrap();
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
    output.push_str("  S# = native machine state; T# = original machine edge; %value = stack name; !effect = recorded machine effects.\n");
    output.push_str("  Instruction progress and known edges are local evidence. Open incoming paths and unresolved frontiers remain part of this artifact.\n");
    output.push_str("  Current = evidence for the current joined entry; Stale = evidence for an earlier entry; Unexecuted = no transfer receipt.\n");
    output.push_str("\nBlocks\n");
    for block in ir.blocks() {
        let state = &analysis.states()[block.state];
        let frame = state.active();
        writeln!(
            output,
            "S{} | coverage={:?} | incoming complete={} | open incoming={:?}",
            block.state, block.coverage, block.incoming_complete, block.open_incoming,
        )
        .unwrap();
        writeln!(
            output,
            "  active=F{} | depth={} | mode={:?} | code={} | storage owner={}",
            state.key.frames.len() - 1,
            state.key.frames.len(),
            frame.mode,
            super::environment::owner(analysis, frame.code_address),
            frame.address_value,
        )
        .unwrap();
        for phi in &block.phis {
            let inputs = phi
                .inputs
                .iter()
                .map(|(edge, value)| format!("T{edge}: %{value}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "    %{} = partial phi({inputs}) ; F{} slot {}",
                phi.result, phi.frame, phi.slot,
            )
            .unwrap();
        }
        let inputs = block
            .effect
            .inputs
            .iter()
            .map(|input| format!("T{}: !{}", input.transition, input.effect))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "    !{} = partial effect phi({inputs})",
            block.effect.result
        )
        .unwrap();
        let layout = state
            .program()
            .and_then(|program| program.blocks().get(frame.basic_block_index))
            .filter(|_| !block.instructions.is_empty())
            .map(|source| InstructionLayout::block(&mut output, source.id, source.start_pc));
        if block.instructions.is_empty() {
            output.push_str("    (no bytecode instruction evidence)\n");
        }
        for item in &block.instructions {
            let layout = layout.as_ref().expect("recorded instruction has bytecode");
            layout.write_pc(&mut output, item.instruction.pc);
            write_ssa_body(&mut output, &item.instruction);
            write!(output, " ; progress={:?}", item.progress).unwrap();
            match item.progress {
                InstructionProgress::Started => {
                    output.push_str("; no operands consumed or normal result")
                }
                InstructionProgress::OperandsConsumed => {
                    output.push_str("; pending operation, no normal result")
                }
                InstructionProgress::Completed => {}
                InstructionProgress::Dispatched => {
                    output.push_str("; results belong only to recorded transitions")
                }
                InstructionProgress::Faulted => output.push_str("; recorded opcode/stack fault"),
            }
            output.push('\n');
            layout.indent(&mut output);
            if let Some(effect) = item.effect_result {
                let label = if item.progress == InstructionProgress::OperandsConsumed {
                    "observed partial effect"
                } else {
                    "effect"
                };
                writeln!(output, "  {label} !{} -> !{effect}", item.effect_input).unwrap();
            } else {
                writeln!(output, "  effect !{} remains open", item.effect_input).unwrap();
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
            write!(output, " F{frame}: [{values}]").unwrap();
        }
        let effect_label = if current {
            "recorded prefix effect"
        } else {
            "entry effect (no current exit)"
        };
        writeln!(output, "\n    {effect_label}: !{}", block.exit_effect).unwrap();
    }
    output.push_str("\nKnown transitions\n");
    for transition in ir.transitions() {
        let edge = &analysis.edges()[transition.edge];
        writeln!(
            output,
            "  T{} | S{} -> S{} | kind={:?} | effect !{} -> !{}",
            transition.edge,
            edge.from,
            edge.to,
            transition.kind,
            transition.effect_input,
            transition.effect_result,
        )
        .unwrap();
        if let Some(value) = transition.result {
            writeln!(output, "    deferred result: %{value}").unwrap();
        }
        for (frame, stack) in transition.stacks.iter().enumerate() {
            let values = stack
                .iter()
                .map(|value| format!("%{value}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(output, "    F{frame}: [{values}]").unwrap();
        }
    }
    output.push_str("\nDeferred edges\n");
    for edge in ir.deferred_edges() {
        writeln!(
            output,
            "  T{} | S{} -> S{} | kind={:?} | reason={:?}",
            edge.edge, edge.from, edge.to, edge.kind, edge.reason
        )
        .unwrap();
    }
    output.push_str("\nFrontiers retained\n");
    for (index, frontier) in ir.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  U{index} | from={:?} | pc={:?} | reason={:?} | target={:?}",
            frontier.from, frontier.pc, frontier.reason, frontier.target
        )
        .unwrap();
    }
    output
}
