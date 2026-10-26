//! Source labels distinguish block extent from the actual last opcode offset.
//! Formatting reads decoded code only; it makes no execution/SSA assertion.
use evm_abstract_protocol::DisasmInstruction;

pub(super) fn title(start_pc: usize, instructions: &[DisasmInstruction]) -> String {
    let end = instructions.last().and_then(|instruction| {
        instruction
            .pc
            .checked_add(instruction.size.saturating_sub(1))
    });
    match end {
        Some(end) if end != start_pc => format!("Block 0x{start_pc:04x}–0x{end:04x}"),
        _ => format!("Block 0x{start_pc:04x}"),
    }
}

pub(super) fn ending(instructions: &[DisasmInstruction]) -> String {
    instructions.last().map_or_else(
        || "Decoded instructions unavailable".into(),
        |instruction| format!("{} @ 0x{:04x}", instruction.name, instruction.pc),
    )
}

pub(super) fn instruction(instruction: &DisasmInstruction) -> String {
    let operand = instruction
        .immediate
        .as_ref()
        .map_or(String::new(), |value| {
            let mut chars = value.chars();
            let prefix: String = chars.by_ref().take(24).collect();
            format!(" {prefix}{}", if chars.next().is_some() { "…" } else { "" })
        });
    format!("{:04x}  {}{operand}", instruction.pc, instruction.name)
}

#[cfg(test)]
mod tests;
