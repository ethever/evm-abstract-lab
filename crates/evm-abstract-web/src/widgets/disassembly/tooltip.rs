//! Instruction metadata and stack effects belong to the source-row hover.

use evm_abstract_protocol::DisasmInstruction;

pub(super) fn instruction(instruction: &DisasmInstruction, executed: bool) -> String {
    let signature = if instruction.valid && instruction.opcode != 0xfe {
        format!(
            "Stack signature: {} → {} (normal execution)\n{}\nCounts are stack elements: required top-of-stack window → resulting window. Values below it are unchanged; these are not the total stack heights.",
            instruction.stack_inputs,
            instruction.stack_outputs,
            effect(instruction),
        )
    } else {
        "Invalid instruction for this fork: exceptional halt, no normal stack result.".into()
    };
    format!(
        "0x{:04x} · opcode 0x{:02x} · {} encoded bytes\n{}{}\n{}\n{}",
        instruction.pc,
        instruction.opcode,
        instruction.size,
        instruction.name,
        instruction
            .immediate
            .as_ref()
            .map_or(String::new(), |value| format!(" {value}")),
        signature,
        if executed {
            "Observed in current execution evidence"
        } else {
            "Decoded source; no current execution receipt for this instruction"
        },
    )
}

fn effect(instruction: &DisasmInstruction) -> String {
    match instruction.opcode {
        0x01 => "Pop two values and push their sum modulo 2^256.".into(),
        0x50 => "Pop and discard the top value.".into(),
        0x5f => "Push zero onto the stack.".into(),
        0x60..=0x7f => "Push the immediate value onto the stack.".into(),
        0x80..=0x8f => format!(
            "Copy stack position {} (1 = top) onto the top; retain all original values.",
            instruction.opcode - 0x7f,
        ),
        0x90..=0x9f => format!(
            "Exchange the top with stack position {} (1 = top); stack height is unchanged.",
            instruction.opcode - 0x8e,
        ),
        0xf1 | 0xf2 | 0xf4 | 0xfa => "Consume call arguments; if the caller resumes, push the success flag (0 or 1). An incomplete analysis may not have this result yet.".into(),
        0xf0 | 0xf5 => "Consume creation arguments; if the caller resumes, push the created address or zero on failure. An incomplete analysis may not have this result yet.".into(),
        _ => format!(
            "Consume {} stack element(s) and produce {} stack element(s).",
            instruction.stack_inputs, instruction.stack_outputs,
        ),
    }
}
