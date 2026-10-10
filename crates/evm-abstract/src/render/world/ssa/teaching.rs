//! 以三地址指令展示已验证的跨合约栈 SSA，保留完整状态与帧栈身份。
//!
//! DUP/SWAP 只调整已有名称；调用结果在恢复调用者的转换处定义。
//! 检查点和粗粒度效果只在跨帧转换处展示，完整证据由详细视图提供。

use evm_abstract_notation::Symbol;

use crate::render::{
    instruction::{InstructionLayout, write_ssa_body},
    world::teaching::References,
};
use crate::{
    analysis::{MachineEdgeKind, MachineState, WorldAnalysis},
    ssa::{Instruction, Transition, ValueId, WorldBlock, WorldSsa},
};
use revm_bytecode::opcode;
use std::fmt::Write;

#[cfg(test)]
mod tests;

/// 简洁视图保留原图的状态、转换编号和操作数弹出顺序。
pub(in crate::render::world) fn render(analysis: &WorldAnalysis, ir: &WorldSsa) -> String {
    let references = References::new(analysis);
    let mut output = String::from("Verified cross-contract SSA:\n");
    writeln!(
        output,
        "  states={} | transitions={} | stack values={}",
        ir.blocks().len(),
        ir.transitions().len(),
        ir.value_count()
    )
    .unwrap();
    output.push_str("  σᵢ = machine state; Bᵢ = frame-local block; fᵢ = frame; Tᵢ = transition; %value = stack value.\n");
    output.push_str("  Frames are oldest caller first; stacks are bottom-to-top; instruction operands are EVM pop order.\n");
    output.push_str("  DUP/SWAP preserve value names; μᵢ is a coarse machine-effect bundle including rollback checkpoints.\n");
    for block in ir.blocks() {
        output.push('\n');
        write_block(
            &mut output,
            &analysis.states()[block.state],
            block,
            &references,
        );
    }
    output.push_str("\nTransitions\n");
    if ir.transitions().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, transition) in ir.transitions().iter().enumerate() {
        write_transition(&mut output, analysis, index, transition);
    }
    output
}

fn write_block(
    output: &mut String,
    state: &MachineState,
    block: &WorldBlock,
    references: &References,
) {
    let frame = state.active();
    let code = references
        .code(frame)
        .expect("captured active frame has a code reference");
    let owner = references.address_input(frame.address_value);
    writeln!(
        output,
        "{} | {code} | {} active | state owner={owner} | context={:?}:",
        Symbol::State(block.state),
        Symbol::Frame(state.key.frames.len() - 1),
        frame.jump_history
    )
    .unwrap();
    for phi in &block.phis {
        let inputs = phi
            .inputs
            .iter()
            .map(|(transition, value)| format!("{}: %{value}", Symbol::Transition(*transition)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "    %{} = φ({inputs}) ; {} slot {}",
            phi.result,
            Symbol::Frame(phi.frame),
            phi.slot
        )
        .unwrap();
    }
    let layout = state
        .program()
        .and_then(|program| program.blocks().get(frame.basic_block_index))
        .map(|source| InstructionLayout::block(output, source.id, source.start_pc));
    if block.instructions.is_empty() {
        writeln!(
            output,
            "    (no bytecode instructions; {})",
            super::empty_instruction_reason(state)
        )
        .unwrap();
    }
    for instruction in &block.instructions {
        write_instruction(
            output,
            instruction,
            layout
                .as_ref()
                .expect("bytecode instruction has a real block"),
        );
    }
    if let Some(layout) = &layout {
        layout.indent(output);
    } else {
        output.push_str("    ");
    }
    output.push_str("stack out (before dispatch) ");
    write_stacks(output, &block.exit_frames);
    output.push('\n');
}

fn write_instruction(output: &mut String, instruction: &Instruction, layout: &InstructionLayout) {
    layout.write_pc(output, instruction.pc);
    write_ssa_body(output, instruction);
    if instruction.fault {
        output.push_str(" ; exceptional halt (opcode/stack)");
    } else if (opcode::DUP1..=opcode::DUP16).contains(&instruction.opcode) {
        output.push_str(" ; duplicate alias");
    } else if (opcode::SWAP1..=opcode::SWAP16).contains(&instruction.opcode) {
        output.push_str(" ; aliases reordered");
    }
    output.push('\n');
}

fn write_stacks(output: &mut String, stacks: &[Vec<ValueId>]) {
    for (frame, stack) in stacks.iter().enumerate() {
        if frame != 0 {
            output.push_str("; ");
        }
        write!(output, "{}: {}", Symbol::Frame(frame), super::values(stack)).unwrap();
    }
}

fn write_transition(
    output: &mut String,
    analysis: &WorldAnalysis,
    index: usize,
    transition: &Transition,
) {
    let edge = &analysis.edges()[transition.edge];
    write!(
        output,
        "  {} | {} → {} | ",
        Symbol::Transition(index),
        Symbol::State(edge.from),
        Symbol::State(edge.to)
    )
    .unwrap();
    if let MachineEdgeKind::Intraprocedural(kind) = transition.kind {
        writeln!(output, "{kind:?}").unwrap();
        return;
    }
    writeln!(
        output,
        "{:?} | operands {}",
        transition.kind,
        super::values(&transition.operands)
    )
    .unwrap();
    let source_depth = analysis.states()[edge.from].key.frames.len();
    let destination_depth = analysis.states()[edge.to].key.frames.len();
    match transition.kind {
        MachineEdgeKind::Call => writeln!(
            output,
            "    suspend {}; enter {}; save rollback checkpoint",
            Symbol::Frame(source_depth - 1),
            Symbol::Frame(destination_depth - 1)
        ),
        MachineEdgeKind::Return => writeln!(
            output,
            "    resume {}; commit child effects",
            Symbol::Frame(destination_depth - 1)
        ),
        MachineEdgeKind::Revert => writeln!(
            output,
            "    resume {}; rollback to saved checkpoint; retain revert data",
            Symbol::Frame(destination_depth - 1)
        ),
        MachineEdgeKind::Failure if source_depth > destination_depth => writeln!(
            output,
            "    resume {}; rollback to saved checkpoint; empty returndata",
            Symbol::Frame(destination_depth - 1)
        ),
        MachineEdgeKind::Failure => writeln!(
            output,
            "    {} continues; invocation rejected; empty returndata",
            Symbol::Frame(destination_depth - 1)
        ),
        MachineEdgeKind::Intraprocedural(_) => unreachable!(),
    }
    .unwrap();
    if let Some(result) = transition.result {
        // 子帧的最后指令可能是 RETURN/REVERT，不能用它判断调用结果类型。
        let source = &analysis.states()[edge.from];
        let creation = if source_depth > destination_depth {
            source
                .entry
                .call_stack
                .active_child()
                .is_some_and(|child| child.continuation.creation.is_some())
        } else {
            source
                .program()
                .and_then(|program| program.blocks().get(source.active().basic_block_index))
                .and_then(|block| {
                    let pc = source.executed_pcs.last()?;
                    block
                        .instructions
                        .iter()
                        .find(|instruction| instruction.pc == *pc)
                })
                .is_some_and(|instruction| {
                    matches!(instruction.opcode, opcode::CREATE | opcode::CREATE2)
                })
        };
        if creation {
            let value = if transition.kind == MachineEdgeKind::Return {
                "created address"
            } else {
                "0"
            };
            writeln!(output, "    CREATE address %{result} = {value}").unwrap();
        } else {
            let value = usize::from(transition.kind == MachineEdgeKind::Return);
            writeln!(output, "    CALL result %{result} = {value}").unwrap();
        }
    }
    write!(output, "    stack in ").unwrap();
    write_stacks(output, &transition.stacks);
    writeln!(
        output,
        " | effects {} → {}",
        Symbol::Effect(transition.effect_input),
        Symbol::Effect(transition.effect_result)
    )
    .unwrap();
}
