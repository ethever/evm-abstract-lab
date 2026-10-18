//! Exceptional gas termination can follow a sticky RIPEMD touch in any frame.
//!
//! CALL/CREATE end blocks, so their continuations expose new sticky touches.
//! SELFDESTRUCT also has an instruction-local failure point: its host effects
//! precede the dynamic gas charge. Both envelopes retain normal execution.

use super::{Execution, ExecutionFrontier, TransferContext, WorkBudget, boundary_reason, calls};
use crate::{
    analysis::{FrontierReason, MachinePayload, OutcomeKind},
    domain::{AbstractValue, provenance::MAX_ORIGINS, relational::Constraint},
    world::{ByteArray, Store},
};
use alloy_primitives::{Address, U256};

#[cfg(test)]
mod tests;

// A fixed-width address lookup has at most usize::BITS tree levels and at
// most 11 key comparisons per std BTree node; 16 pays those comparisons and
// fixed entry bookkeeping. Reserve before reading even known-untouched input.
const TOUCH_QUERY_WORK: usize = (usize::BITS as usize + 1) * 16;

fn reserve(budget: &mut WorkBudget, work: usize) -> Option<()> {
    (!budget.exhausted() && work != usize::MAX && budget.charge(work)).then_some(())
}

fn tree_work(entries: usize) -> usize {
    let height = (usize::BITS - entries.leading_zeros()) as usize + 1;
    // 11 comparisons per ordered-tree node, rounded up for bookkeeping.
    entries
        .saturating_add(1)
        .saturating_mul(height)
        .saturating_mul(16)
}

// These units include eventual temporary-container drops. Numeric candidates,
// expressions, byte/slot roots, Programs, paths and snapshots clone handles;
// only the small OriginSet inside a value owns a cloned tree.
const HANDLE_CLONE_WORK: usize = 2; // retain + release
const MAP_ENTRY_WORK: usize = 4; // allocation, entry copy, removal, release
const VALUE_INLINE_WORK: usize = 20; // fixed numeric/identity/flag/handle fields
const MAX_VALUE_CLONE_WORK: usize = VALUE_INLINE_WORK + MAP_ENTRY_WORK * MAX_ORIGINS;
const ARRAY_FIXED_WORK: usize = 16; // headers, root, metrics, flags and failure
const EMPTY_DATA_WORK: usize = ARRAY_FIXED_WORK + 2 * MAX_VALUE_CLONE_WORK;
const STORE_ENTRY_WORK: usize = 12; // fixed key/value fields and node bookkeeping
const FRAME_KEY_WORK: usize = 20; // ten fixed fields, including Vec bookkeeping
const FRAME_FIXED_WORK: usize = 12; // Program/Snapshot handles, mode and headers
const CONSTRAINT_INLINE_WORK: usize = 16; // fixed words and up to two Expr handles

fn value_clone_work(value: &AbstractValue) -> usize {
    VALUE_INLINE_WORK.saturating_add(
        value
            .provenance()
            .origins()
            .work_size()
            .saturating_mul(MAP_ENTRY_WORK),
    )
}

fn array_work(array: &ByteArray, budget: &mut WorkBudget) -> Option<usize> {
    // Only the directly owned value metadata is inspected, after admission.
    // Conditional subtrees, Exact buffers and byte-state history stay shared.
    reserve(budget, array.cloned_value_count().saturating_add(1))?;
    let bytes = array.stored_bytes();
    let operands = array.symbolic_facts();
    let cells = bytes.len().saturating_add(operands.len());
    let expressions = operands.len();
    let allocations = array.allocation_facts().len();
    let values = bytes
        .map(|(_, value)| value)
        .chain(operands.map(|(_, value)| value))
        .chain([array.len(), array.default_byte()])
        .fold(0usize, |work, value| {
            work.saturating_add(value_clone_work(value))
        });
    Some(
        ARRAY_FIXED_WORK
            .saturating_add(values)
            .saturating_add(cells.saturating_mul(MAP_ENTRY_WORK))
            .saturating_add(expressions.saturating_mul(HANDLE_CLONE_WORK))
            .saturating_add(allocations.saturating_mul(MAP_ENTRY_WORK + 2 * HANDLE_CLONE_WORK))
            .saturating_add(
                array
                    .cloned_conditional_branches()
                    .saturating_mul(HANDLE_CLONE_WORK + 2),
            ),
    )
}

struct StoreWork {
    clone: usize,
    key_error: usize,
}

fn store_work(store: &Store, key_error: bool, budget: &mut WorkBudget) -> Option<StoreWork> {
    let entries = store.finalization_entries();
    // O(1) census first: two flat passes cover scalar/log metadata and, when
    // needed, runtime lengths. No AV, topic, log data or code entry is read yet.
    reserve(budget, entries.saturating_add(1).saturating_mul(2))?;
    let mut clone = store
        .cloned_values()
        .fold(entries.saturating_mul(STORE_ENTRY_WORK), |work, value| {
            work.saturating_add(value_clone_work(value))
        });
    for log in store.possible_logs().values() {
        reserve(budget, log.topics.len().saturating_add(1))?;
        clone = log.topics.iter().fold(
            clone.saturating_add(array_work(&log.data, budget)?),
            |work, value| work.saturating_add(value_clone_work(value)),
        );
    }
    let key_error = if key_error {
        // finish can construct ONE error key. code_identity really builds an
        // address set and hashes raw runtime bytes: this is not Program.clone.
        // One set insertion + up to eight lookups per entry; the maximum key
        // record is address + tag + code hash + three lifecycle flags. Four
        // byte passes cover Vec append/growth and the final record-buffer hash.
        let record_bytes = 20 + 1 + 32 + 3;
        tree_work(entries)
            .saturating_mul(9)
            .saturating_add(entries.saturating_mul(record_bytes * 4))
            .saturating_add(store.code_identity_bytes().saturating_mul(2))
    } else {
        0
    };
    Some(StoreWork { clone, key_error })
}

fn reserve_failure(
    payload: &MachinePayload,
    procedure: bool,
    budget: &mut WorkBudget,
) -> Option<()> {
    let child = payload.call_stack.active_child().is_some();
    let current = store_work(&payload.store, child || !procedure, budget)?;
    let saved = if procedure || child {
        // Snapshot.clone shares an Arc. Only the active snapshot is restored;
        // the original payload keeps it shared, so restoration clones one Store.
        Some(store_work(
            payload.active().saved_store.state(),
            child,
            budget,
        )?)
    } else {
        // Checked transaction finalization owns its staging/rollback work.
        None
    };
    let depth = payload.call_stack.depth();
    reserve(budget, depth.saturating_mul(4))?;
    let mut frames = 0usize;
    let mut key_frames = 0usize;
    let mut normalize_values = 1usize; // the parent's newly pushed result
    for (index, frame) in payload.call_stack.iter().enumerate() {
        reserve(budget, frame.stack.len().saturating_add(4))?;
        let key = FRAME_KEY_WORK.saturating_add(frame.key.jump_history.len().saturating_mul(2));
        key_frames = key_frames.saturating_add(key);
        frames = frame.stack.iter().fold(
            frames
                .saturating_add(key)
                .saturating_add(FRAME_FIXED_WORK)
                .saturating_add(value_clone_work(&frame.call_value))
                .saturating_add(frame.address_operand.as_ref().map_or(0, value_clone_work))
                .saturating_add(frame.caller_operand.as_ref().map_or(0, value_clone_work)),
            |work, value| work.saturating_add(value_clone_work(value)),
        );
        for array in [&frame.memory, &frame.calldata, &frame.returndata] {
            frames = frames.saturating_add(array_work(array, budget)?);
        }
        if index + 1 < depth {
            normalize_values = normalize_values
                .saturating_add(frame.stack.len())
                .saturating_add(1)
                .saturating_add(usize::from(frame.address_operand.is_some()))
                .saturating_add(usize::from(frame.caller_operand.is_some()));
        }
    }
    let children = payload.call_stack.children().len();
    reserve(budget, children.saturating_mul(2).saturating_add(1))?;
    for child in payload.call_stack.children() {
        frames = frames
            .saturating_add(8) // continuation fixed fields and child Vec entry
            .saturating_add(value_clone_work(&child.continuation.output_offset))
            .saturating_add(value_clone_work(&child.continuation.output_size));
    }
    let constraints = payload.relations.constraints();
    reserve(budget, constraints.len().saturating_add(1))?;
    let mut relations = 8usize // path Arc, bottom flag and owned-set header
        .saturating_add(
            constraints
                .len()
                .saturating_mul(CONSTRAINT_INLINE_WORK + MAP_ENTRY_WORK),
        );
    for constraint in constraints {
        if let Constraint::Members { values, .. } = constraint {
            // Members owns U256s; finite numeric candidates and the path do not.
            relations = relations.saturating_add(values.len().saturating_mul(2 + MAP_ENTRY_WORK));
        }
    }
    let payload_clone = current
        .clone
        .saturating_add(frames)
        .saturating_add(relations);
    // Failure clones the input once; a child additionally retains CompletedCall.
    let mut work = payload_clone
        .saturating_mul(if child { 2 } else { 1 })
        .saturating_add(EMPTY_DATA_WORK);
    let mut key_error = current.key_error;
    if let Some(saved) = saved {
        work = work
            .saturating_add(saved.clone)
            // Three touched-map searches/updates; the flat Store clone bound
            // also covers first-write imbl COW and its temporary node drops.
            .saturating_add(TOUCH_QUERY_WORK.saturating_mul(3));
        key_error = key_error.max(saved.key_error);
        if child {
            // output_store clones the restored Store; one new touch entry may
            // have appeared. No other historical saved Store is restored here.
            work = work
                .saturating_add(saved.clone)
                .saturating_add(STORE_ENTRY_WORK);
        }
    }
    if child {
        let parent_stack = payload
            .call_stack
            .parent()
            .expect("child has a parent")
            .stack
            .len();
        normalize_values =
            normalize_values.saturating_add(children.saturating_sub(1).saturating_mul(2));
        work = work
            // Vec growth moves the parent's inline values, without cloning
            // their owned origin sets; constructing the new result is bounded.
            .saturating_add(parent_stack.saturating_mul(VALUE_INLINE_WORK))
            .saturating_add(MAX_VALUE_CLONE_WORK)
            .saturating_add(normalize_values.saturating_mul(2))
            .saturating_add(depth.saturating_sub(1).saturating_mul(2))
            // Empty input/completed/output data and the overwritten returndata
            // headers are fixed; output COPY retains its existing paid owner.
            .saturating_add(EMPTY_DATA_WORK.saturating_mul(2));
    }
    if key_error != 0 {
        work = work.saturating_add(key_error).saturating_add(key_frames);
    }
    // The original result.payload (or borrowed SELFDESTRUCT candidate) keeps
    // old Arc graphs live through finish. This only prices cloned containers'
    // drops, not arbitrary last-owner DAG destruction. Newly created lifecycle
    // nodes, checked finalization and output-copy work retain their owners.
    reserve(budget, work)
}

fn refused(result: &mut Execution, pc: usize) -> bool {
    // Building a MachineKey hashes code and clones frame keys. Refused
    // admission retains the location without doing that unpaid extra work.
    result.frontiers.push(ExecutionFrontier {
        pc,
        reason: boundary_reason(FrontierReason::Work),
        target: None,
        range: None,
        payload: None,
    });
    false
}

pub(super) fn at_block_entry(result: &mut Execution, context: &mut TransferContext<'_>) -> bool {
    let frame = result.payload.active();
    let pc = frame.program.as_ref().map_or(0, |program| {
        program
            .blocks()
            .get(frame.key.basic_block_index)
            .and_then(|block| block.instructions.first())
            .map_or(program.byte_len(), |instruction| instruction.pc)
    });
    if reserve(context.budget, TOUCH_QUERY_WORK).is_none() {
        return refused(result, pc);
    }
    let ripemd = Address::from_word(U256::from(3).into());
    let touched = result.payload.store.touched_in_transaction(ripemd);
    if touched == Some(false) {
        return true;
    }
    if reserve_failure(&result.payload, context.procedure, context.budget).is_none() {
        return refused(result, pc);
    }
    let frontiers = result.frontiers.len();
    calls::failure(result, context, pc);
    if result.frontiers.len() != frontiers {
        return false;
    }
    if context.budget.exhausted() {
        return refused(result, pc);
    }
    true
}

/// The host may touch RIPEMD before SELFDESTRUCT's dynamic gas charge fails.
/// Restore through the ordinary completion path so only that sticky touch,
/// not the transferred balance or destruction effects, survives failure.
pub(super) fn after_selfdestruct(
    result: &mut Execution,
    payload: &MachinePayload,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> bool {
    if reserve(context.budget, TOUCH_QUERY_WORK).is_none() {
        return refused(result, pc);
    }
    let ripemd = Address::from_word(U256::from(3).into());
    if payload.store.touched_in_transaction(ripemd) == Some(false) {
        return true;
    }
    if reserve_failure(payload, context.procedure, context.budget).is_none() {
        return refused(result, pc);
    }
    let frontiers = result.frontiers.len();
    calls::finish(
        result,
        payload.clone(),
        OutcomeKind::Failure,
        ByteArray::empty(),
        context,
        pc,
    );
    if result.frontiers.len() != frontiers {
        return false;
    }
    if context.budget.exhausted() {
        return refused(result, pc);
    }
    true
}
