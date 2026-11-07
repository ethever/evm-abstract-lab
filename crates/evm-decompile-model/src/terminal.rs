//! Terminal spelling requires the retained source instruction and fault witness.

use crate::{Block, Terminator};
use evm_bytecode::InvalidOpcodeReason;

/// Canonical spelling for an actual final `0xfe INVALID` instruction.
/// Other traps keep their existing diagnostic rendering. The decoder's fault
/// reason alone is insufficient: the last instruction must have a one-byte
/// original source span inside this block and the matching terminal behavior.
pub fn render_explicit_invalid(block: &Block) -> Option<&'static str> {
    let Terminator::Trap { reason } = &block.terminator else {
        return None;
    };
    let operation = block.operations.last()?;
    let source = operation.source?;
    let block_source = block.source?;
    (operation.opcode == 0xfe
        && source.pc.checked_add(1) == Some(source.end_pc)
        && block_source.pc <= source.pc
        && source.end_pc <= block_source.end_pc
        && operation.inputs.is_empty()
        && operation.outputs.is_empty()
        && operation.replacement.is_none()
        && operation.effects.terminate
        && reason == &InvalidOpcodeReason::ExplicitInvalid.to_string())
        .then_some("evm.invalid();")
}
