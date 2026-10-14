//! Detect a repeated executable/storage context before unfolding another stack.

use crate::analysis::{FrameCode, MachinePayload};

/// Only an ordinary child entry can be folded. Suspended callers keep their
/// continuations and rollback snapshots outside the relative procedure root.
/// CALLER/calldata/value are inputs of that root, not depth-dependent identity.
pub(in crate::analysis) fn is_recursive_entry(payload: &MachinePayload) -> bool {
    let Some(child) = payload.call_stack.active_child() else {
        return false;
    };
    let frame = &child.state;
    if child.continuation.creation.is_some()
        || frame.code != FrameCode::Runtime
        || frame.key.basic_block_index != 0
        || !frame.stack.is_empty()
    {
        return false;
    }
    let active = &frame.key;
    payload
        .call_stack
        .iter()
        .take(payload.call_stack.depth() - 1)
        .any(|ancestor| {
            let key = &ancestor.key;
            ancestor.code == FrameCode::Runtime
                && key.code_address == active.code_address
                && key.code_hash == active.code_hash
                && key.mode == active.mode
                && key.address == active.address
                && key.address_value == active.address_value
                && key.is_static == active.is_static
        })
}

#[cfg(test)]
mod tests;
