//! 完整调用上下文的 SSA 文本；保留原始定义与边输入，不把抽象图解释为执行轨迹。
//!
//! 调用者提供已经验证的 WorldSsa。效果名表示包含检查点的整机 bundle，
//! 并不拆分内存别名或声称提供精确的 MemorySSA。

use crate::{
    analysis::{FrameCode, FrameKey, MachineEdgeKind, MachineState, WorldAnalysis},
    bytecode::Program,
    ssa::{Instruction, Transition, ValueId, WorldBlock, WorldSsa},
};
use revm_bytecode::opcode::{self, OpCode};
use std::fmt::Write;

pub(super) mod teaching;

#[cfg(test)]
mod tests;

/// 将已验证的栈 SSA 和粗粒度效果 SSA 连同原图上下文一起展示。
pub(super) fn render(analysis: &WorldAnalysis, ssa: &WorldSsa) -> String {
    let mut output = String::from("Verified cross-contract SSA:\n");
    writeln!(
        output,
        "  blocks={} | transitions={} | stack values={} | effect bundles={}",
        ssa.blocks().len(),
        ssa.transitions().len(),
        ssa.value_count(),
        ssa.effect_count()
    )
    .unwrap();
    output.push_str("  S# = machine state; B# = frame-local basic block; F# = frame; T# = transition; %value = stack value; !effect = machine effects.\n");
    output.push_str("  Frames are oldest caller first; stacks are bottom-to-top; instruction operands are pop order.\n");
    output.push_str("  Effects are coarse bundles of memory, calldata, returndata, persistent/transient storage, balances, nonces, code/account lifecycle, logs, environment and rollback checkpoints; this is not alias-partitioned MemorySSA.\n");
    output.push_str("  DUP/SWAP results reuse existing value names. fault marks invalid-opcode or stack faults only; other execution failures are recorded in transitions and outcomes.\n");
    output.push_str("\nBlocks\n");
    for block in ssa.blocks() {
        write_block(&mut output, &analysis.states()[block.state], block);
    }
    output.push_str("\nTransitions\n");
    if ssa.transitions().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, transition) in ssa.transitions().iter().enumerate() {
        write_transition(&mut output, analysis, index, transition);
    }
    output
}

fn write_block(output: &mut String, state: &MachineState, block: &WorldBlock) {
    let depth = state.key.frames.len();
    writeln!(
        output,
        "  S{} | active=F{} | call depth={} | machine code identity={}",
        block.state,
        depth - 1,
        depth,
        state.key.code_identity
    )
    .unwrap();
    for (index, (key, frame)) in state
        .key
        .frames
        .iter()
        .zip(state.entry.call_stack.iter())
        .enumerate()
    {
        write_frame(
            output,
            index,
            index + 1 == depth,
            key,
            frame.program.as_ref(),
        );
    }
    output.push_str("    frame phis (stack in):\n");
    if block.phis.is_empty() {
        output.push_str("      (none)\n");
    }
    for phi in &block.phis {
        let inputs = phi
            .inputs
            .iter()
            .map(|(transition, value)| format!("T{transition}: %{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "      %{} = frame phi(F{}, slot={}, inputs=[{inputs}])",
            phi.result, phi.frame, phi.slot
        )
        .unwrap();
    }
    if block.effect.inputs.is_empty() {
        writeln!(
            output,
            "    !{} = effect phi(root world/entry; inputs=[])",
            block.effect.result
        )
        .unwrap();
    } else {
        let inputs = block
            .effect
            .inputs
            .iter()
            .map(|input| format!("T{}: !{}", input.transition, input.effect))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "    !{} = effect phi(inputs=[{inputs}])",
            block.effect.result
        )
        .unwrap();
    }
    output.push_str("    bytecode instructions:\n");
    if block.instructions.is_empty() {
        writeln!(output, "      (none; {})", empty_instruction_reason(state)).unwrap();
    }
    for instruction in &block.instructions {
        write_instruction(output, instruction);
    }
    output.push_str("    stack out (before dispatch):\n");
    write_stacks(output, &block.exit_frames);
    output.push_str("    instruction effects:\n");
    if block.effects.is_empty() {
        output.push_str("      (none)\n");
    }
    for (pc, input, result) in &block.effects {
        writeln!(output, "      pc=0x{pc:04x}: !{input} -> !{result}").unwrap();
    }
    writeln!(output, "    exit effect: !{}", block.exit_effect).unwrap();
}

fn write_frame(
    output: &mut String,
    index: usize,
    active: bool,
    frame: &FrameKey,
    program: Option<&Program>,
) {
    let role = if active { "active" } else { "suspended" };
    let location = program
        .and_then(|program| program.blocks().get(frame.basic_block_index))
        .map_or_else(
            || match frame.mode {
                FrameCode::InvalidDelegation => {
                    "invalid nested delegation; exceptional halt".to_owned()
                }
                FrameCode::Precompile(_) => "native precompile".to_owned(),
                FrameCode::Empty => "empty executable code".to_owned(),
                FrameCode::Runtime | FrameCode::InitCode => {
                    "synthetic end-of-code continuation".to_owned()
                }
            },
            |block| format!("pc=0x{:04x}", block.start_pc),
        );
    writeln!(
        output,
        "    F{index} {role} | B{} ({location}) | mode={:?} | stack height={}",
        frame.basic_block_index, frame.mode, frame.stack_height
    )
    .unwrap();
    writeln!(
        output,
        "      code address={} | storage owner={} | code hash={}",
        frame.code_address, frame.address, frame.code_hash
    )
    .unwrap();
    writeln!(
        output,
        "      caller={} | static={} | jump history={:?}",
        frame.caller, frame.is_static, frame.jump_history
    )
    .unwrap();
}

fn write_instruction(output: &mut String, instruction: &Instruction) {
    let immediate = instruction
        .immediate
        .map_or_else(|| "none".to_owned(), |value| format!("0x{value:x}"));
    writeln!(
        output,
        "      pc=0x{:04x}: {} | opcode=0x{:02x} | immediate={} | operands={} | results={} | fault={}",
        instruction.pc,
        OpCode::name_by_op(instruction.opcode),
        instruction.opcode,
        immediate,
        values(&instruction.operands),
        values(&instruction.results),
        instruction.fault
    )
    .unwrap();
}

fn write_stacks(output: &mut String, stacks: &[Vec<ValueId>]) {
    for (frame, stack) in stacks.iter().enumerate() {
        writeln!(output, "      F{frame}: {}", values(stack)).unwrap();
    }
}

fn values(values: &[ValueId]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("%{value}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn write_transition(
    output: &mut String,
    analysis: &WorldAnalysis,
    index: usize,
    transition: &Transition,
) {
    let edge = &analysis.edges()[transition.edge];
    writeln!(
        output,
        "  T{index} | edge={} | S{} -> S{} | kind={:?}",
        transition.edge, edge.from, edge.to, transition.kind
    )
    .unwrap();
    writeln!(output, "    operands={}", values(&transition.operands)).unwrap();
    output.push_str("    destination stack in:\n");
    write_stacks(output, &transition.stacks);
    writeln!(
        output,
        "    effect: !{} -> !{}",
        transition.effect_input, transition.effect_result
    )
    .unwrap();
    if let Some(result) = transition.result {
        writeln!(
            output,
            "    deferred result: %{result} ({})",
            deferred_kind(analysis, edge.from, edge.to)
        )
        .unwrap();
    } else {
        output.push_str("    deferred result: none\n");
    }
    let effect = match transition.kind {
        MachineEdgeKind::Intraprocedural(_) => "carry effects within the active frame",
        MachineEdgeKind::Call => "suspend caller; enter child with a rollback checkpoint",
        MachineEdgeKind::Return => "resume caller; commit child effects",
        MachineEdgeKind::Revert => "resume caller; restore saved checkpoint; retain revert data",
        MachineEdgeKind::Failure => {
            "reject call or restore child checkpoint; resume caller with empty returndata"
        }
    };
    writeln!(output, "    dispatch: {effect}").unwrap();
}

// 返回边的字节码属于子帧；必须从其 continuation 判断 CREATE 地址结果。
// 同深度的即时失败则从源调用指令判断，避免把 CREATE 的零地址误标成布尔值。
fn deferred_kind(analysis: &WorldAnalysis, from: usize, to: usize) -> &'static str {
    let source = &analysis.states()[from];
    let creation = if source.key.frames.len() > analysis.states()[to].key.frames.len() {
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
                block
                    .instructions
                    .get(source.executed_pcs.len().checked_sub(1)?)
            })
            .is_some_and(|instruction| {
                matches!(instruction.opcode, opcode::CREATE | opcode::CREATE2)
            })
    };
    if creation {
        "CREATE/CREATE2 address; zero on failure or revert"
    } else {
        "CALL success boolean; one on return, zero on failure or revert"
    }
}

fn empty_instruction_reason(state: &MachineState) -> &'static str {
    match state.active().mode {
        FrameCode::InvalidDelegation => "invalid nested delegation; exceptional halt",
        FrameCode::Precompile(_) => "native execution; no bytecode instructions",
        FrameCode::Empty => "empty executable code; implicit completion",
        FrameCode::Runtime | FrameCode::InitCode => {
            "synthetic end-of-code continuation or no executed bytecode instruction"
        }
    }
}
