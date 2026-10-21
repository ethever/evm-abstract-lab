//! Fold a known child only after a structural call edge repeats on the active stack.

use crate::domain::WordElement;

use crate::analysis::{EvmChildFrame, EvmFrameState, EvmMachinePayload, FrameCode};

/// A repeated callee alone can be a leaf self-call with unrelated calldata.
/// Waiting for a repeated active edge keeps that first child on the ordinary
/// executor. Completed children have been popped and cannot trigger folding.
/// CALLER/calldata/value remain inputs of the relative root, not its identity.
pub(in crate::analysis) fn is_recursive_entry<N: WordElement>(
    payload: &EvmMachinePayload<N>,
) -> bool {
    let stack = &payload.call_stack;
    let Some((child, ancestors)) = stack.children().split_last() else {
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
    let parent = stack.parent().expect("a child has a suspended parent");
    let mut ancestor_parent = &stack.root().state;
    for ancestor_child in ancestors {
        if same_edge(parent, child, ancestor_parent, ancestor_child) {
            return true;
        }
        ancestor_parent = &ancestor_child.state;
    }
    false
}

fn same_edge<N: WordElement>(
    parent: &EvmFrameState<N>,
    child: &EvmChildFrame<N>,
    other_parent: &EvmFrameState<N>,
    other_child: &EvmChildFrame<N>,
) -> bool {
    // CALL-family instructions terminate their basic block, and the suspended
    // parent stays at that block. Its immutable program identity, block index,
    // and typed continuation therefore identify the actual call site without
    // including variable output ranges or extending the public frame format.
    other_child.continuation.creation.is_none()
        && parent.key.basic_block_index == other_parent.key.basic_block_index
        && child.continuation.return_block == other_child.continuation.return_block
        && same_context(parent, other_parent)
        && same_context(&child.state, &other_child.state)
}

fn same_context<N: WordElement>(frame: &EvmFrameState<N>, other: &EvmFrameState<N>) -> bool {
    let key = &frame.key;
    let other_key = &other.key;
    frame.code == other.code
        && key.code_address == other_key.code_address
        && key.code_hash == other_key.code_hash
        && key.mode == other_key.mode
        && key.address == other_key.address
        && key.address_value == other_key.address_value
        && key.is_static == other_key.is_static
}

#[cfg(test)]
mod tests;
