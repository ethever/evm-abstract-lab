//! 以三地址指令展示已验证的跨合约栈 SSA，保留完整状态与帧栈身份。
//!
//! DUP/SWAP 只调整已有名称；调用结果在恢复调用者的转换处定义。
//! 检查点和粗粒度效果只在跨帧转换处展示，完整证据由详细视图提供。

use crate::render::world::teaching::References;
use crate::{
    analysis::{MachineEdgeKind, MachineState, WorldAnalysis},
    ssa::{Instruction, Transition, ValueId, WorldBlock, WorldSsa},
};
use revm_bytecode::opcode::{self, OpCode};
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
    output.push_str("  S# = machine state; B# = frame-local block; F# = frame; T# = transition; %value = stack value.\n");
    output.push_str("  Frames are oldest caller first; stacks are bottom-to-top; instruction operands are EVM pop order.\n");
    output.push_str("  DUP/SWAP preserve value names; !effect is a coarse machine-effect bundle including rollback checkpoints.\n");
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
    write!(output, "  S{} | {code} ", block.state).unwrap();
    if let Some(source) = state
        .program()
        .and_then(|program| program.blocks().get(frame.basic_block_index))
    {
        write!(output, "B{} @ 0x{:04x}", source.id, source.start_pc).unwrap();
    } else {
        output.push_str(super::empty_instruction_reason(state));
    }
    let owner = references
        .address(frame.address)
        .map_or_else(|| frame.address.to_string(), str::to_owned);
    writeln!(
        output,
        " | F{} active | state owner={owner}:",
        state.key.frames.len() - 1,
    )
    .unwrap();
    for phi in &block.phis {
        let inputs = phi
            .inputs
            .iter()
            .map(|(transition, value)| format!("T{transition}: %{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "    %{} = phi({inputs}) ; F{} slot {}",
            phi.result, phi.frame, phi.slot
        )
        .unwrap();
    }
    if block.instructions.is_empty() {
        writeln!(output, "    (no bytecode instructions)").unwrap();
    }
    for instruction in &block.instructions {
        write_instruction(output, instruction);
    }
    write!(output, "    stack out (before dispatch) ").unwrap();
    write_stacks(output, &block.exit_frames);
    output.push('\n');
}

fn write_instruction(output: &mut String, instruction: &Instruction) {
    write!(output, "    {:04x}: ", instruction.pc).unwrap();
    if !instruction.results.is_empty() {
        let results = instruction
            .results
            .iter()
            .map(|value| format!("%{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(output, "{results} = ").unwrap();
    }
    write!(output, "{}", OpCode::name_by_op(instruction.opcode)).unwrap();
    if let Some(immediate) = instruction.immediate {
        write!(output, " 0x{immediate:x}").unwrap();
    }
    for value in &instruction.operands {
        write!(output, " %{value}").unwrap();
    }
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
        write!(output, "F{frame}: {}", super::values(stack)).unwrap();
    }
}

fn write_transition(
    output: &mut String,
    analysis: &WorldAnalysis,
    index: usize,
    transition: &Transition,
) {
    let edge = &analysis.edges()[transition.edge];
    write!(output, "  T{index} | S{} -> S{} | ", edge.from, edge.to).unwrap();
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
            "    suspend F{}; enter F{}; save rollback checkpoint",
            source_depth - 1,
            destination_depth - 1
        ),
        MachineEdgeKind::Return => writeln!(
            output,
            "    resume F{}; commit child effects",
            destination_depth - 1
        ),
        MachineEdgeKind::Revert => writeln!(
            output,
            "    resume F{}; rollback to saved checkpoint; retain revert data",
            destination_depth - 1
        ),
        MachineEdgeKind::Failure if source_depth > destination_depth => writeln!(
            output,
            "    resume F{}; rollback to saved checkpoint; empty returndata",
            destination_depth - 1
        ),
        MachineEdgeKind::Failure => writeln!(
            output,
            "    F{} continues; invocation rejected; empty returndata",
            destination_depth - 1
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
        " | effects !{} -> !{}",
        transition.effect_input, transition.effect_result
    )
    .unwrap();
}
