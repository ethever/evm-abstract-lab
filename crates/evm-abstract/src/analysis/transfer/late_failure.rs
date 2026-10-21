//! Exceptional gas termination can follow a sticky RIPEMD touch in any frame.
//!
//! CALL/CREATE end blocks, so their continuations expose new sticky touches.
//! SELFDESTRUCT also has an instruction-local failure point: its host effects
//! precede the dynamic gas charge. Both envelopes retain normal execution.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    EvmExecution, EvmExecutionFrontier, TransferContext, WorkBudget, boundary_reason, calls,
};

use crate::{
    analysis::{EvmMachinePayload, FrontierReason, OutcomeKind},
    domain::provenance::MAX_ORIGINS,
    world::{EvmByteArray, EvmStore},
};
use alloy_primitives::{Address, U256};

#[cfg(test)]
mod tests;

// A fixed-width address lookup has at most usize::BITS tree levels and at
// most 11 key comparisons per std BTree node; 16 pays those comparisons and
// fixed entry bookkeeping. Reserve before reading even known-untouched input.
const TOUCH_QUERY_WORK: usize = (usize::BITS as usize + 1) * 16;

fn reserve(budget: &mut WorkBudget, work: usize) -> Option<()> {
    crate::resource::clone_payment::pay(budget, work).ok()
}

fn tree_work(entries: usize) -> usize {
    let height = (usize::BITS - entries.leading_zeros()) as usize + 1;
    // 11 comparisons per ordered-tree node, rounded up for bookkeeping.
    entries
        .saturating_add(1)
        .saturating_mul(height)
        .saturating_mul(16)
}

// Construction, parent Vec growth and normalization are separate from the
// actual snapshot copies paid by the common completion boundaries.
const MAP_ENTRY_WORK: usize = 4; // allocation, entry copy, removal, release
const VALUE_INLINE_WORK: usize = 20; // fixed numeric/identity/flag/handle fields
const MAX_VALUE_CLONE_WORK: usize = VALUE_INLINE_WORK + MAP_ENTRY_WORK * MAX_ORIGINS;
const ARRAY_FIXED_WORK: usize = 16; // headers, root, metrics, flags and failure
const EMPTY_DATA_WORK: usize = ARRAY_FIXED_WORK + 2 * MAX_VALUE_CLONE_WORK;
const STORE_ENTRY_WORK: usize = 12; // fixed key/value fields and node bookkeeping
const FRAME_KEY_WORK: usize = 20; // ten fixed fields, including Vec bookkeeping

fn store_error_work<N: WordElement>(store: &EvmStore<N>, budget: &mut WorkBudget) -> Option<usize> {
    let entries = store.finalization_entries();
    // Admit the flat runtime-length census before inspecting code entries.
    reserve(budget, entries.saturating_add(1).saturating_mul(2))?;
    // finish can construct ONE error key. code_identity really builds an
    // address set and hashes raw runtime bytes: this is not Program.clone.
    // One set insertion + up to eight lookups per entry; the maximum key
    // record is address + tag + code hash + three lifecycle flags. Four
    // byte passes cover Vec append/growth and the final record-buffer hash.
    let record_bytes = 20 + 1 + 32 + 3;
    Some(
        tree_work(entries)
            .saturating_mul(9)
            .saturating_add(entries.saturating_mul(record_bytes * 4))
            .saturating_add(store.code_identity_bytes().saturating_mul(2)),
    )
}

fn reserve_failure<N: WordElement>(
    payload: &EvmMachinePayload<N>,
    procedure: bool,
    budget: &mut WorkBudget,
) -> Option<()> {
    let child = payload.call_stack.active_child().is_some();
    let mut key_error = if child || !procedure {
        store_error_work(&payload.store, budget)?
    } else {
        0
    };
    if child {
        key_error = key_error.max(store_error_work(
            payload.active().saved_store.state(),
            budget,
        )?);
    } else if procedure {
        // Relative root completion restores directly in finish_root_store;
        // child restore/output copies have their own payers inside finish.
        payload
            .active()
            .saved_store
            .state()
            .pay_clone_with(budget)
            .ok()?;
    }
    let depth = payload.call_stack.depth();
    reserve(budget, depth.saturating_mul(4))?;
    let mut key_frames = 0usize;
    let mut normalize_values = 1usize; // the parent's newly pushed result
    for (index, frame) in payload.call_stack.iter().enumerate() {
        reserve(budget, frame.stack.len().saturating_add(4))?;
        let key = FRAME_KEY_WORK.saturating_add(frame.key.jump_history.len().saturating_mul(2));
        key_frames = key_frames.saturating_add(key);
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
    // Input and completed-call captures, saved-store restoration and output
    // snapshots are paid at their actual clone sites by calls::failure/finish.
    // Keep only this envelope's construction, sticky-touch and error-key work.
    let mut work = EMPTY_DATA_WORK;
    if procedure || child {
        work = work.saturating_add(TOUCH_QUERY_WORK.saturating_mul(3));
        if child {
            // Restoration can add the sticky touch after finish's saved-store
            // quote. Keep the extra entry's allocation/COW/drop allowance.
            work = work.saturating_add(STORE_ENTRY_WORK);
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

fn refused<N: WordElement>(result: &mut EvmExecution<N>, pc: usize, budget: &WorkBudget) -> bool {
    // Building a MachineKey hashes code and clones frame keys. Refused
    // admission retains the location without doing that unpaid extra work.
    result.frontiers.push(EvmExecutionFrontier {
        pc,
        reason: boundary_reason(
            crate::resource::clone_payment::checkpoint(budget)
                .err()
                .map_or(FrontierReason::Work, FrontierReason::from),
        ),
        target: None,
        range: None,
        payload: None,
    });
    false
}

pub(super) fn at_block_entry<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> bool {
    let frame = result.payload.active();
    let pc = frame.program.as_ref().map_or(0, |program| {
        program
            .blocks()
            .get(frame.key.basic_block_index)
            .and_then(|block| block.instructions.first())
            .map_or(program.byte_len(), |instruction| instruction.pc)
    });
    if reserve(context.budget, TOUCH_QUERY_WORK).is_none() {
        return refused(result, pc, context.budget);
    }
    let ripemd = Address::from_word(U256::from(3).into());
    let touched = result.payload.store.touched_in_transaction(ripemd);
    if touched == Some(false) {
        return true;
    }
    if reserve_failure(&result.payload, context.procedure, context.budget).is_none() {
        return refused(result, pc, context.budget);
    }
    let frontiers = result.frontiers.len();
    calls::failure(result, context, pc);
    if result.frontiers.len() != frontiers {
        return false;
    }
    if context.budget.exhausted() {
        return refused(result, pc, context.budget);
    }
    true
}

/// The host may touch RIPEMD before SELFDESTRUCT's dynamic gas charge fails.
/// Restore through the ordinary completion path so only that sticky touch,
/// not the transferred balance or destruction effects, survives failure.
pub(super) fn after_selfdestruct<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    payload: &EvmMachinePayload<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    if reserve(context.budget, TOUCH_QUERY_WORK).is_none() {
        return refused(result, pc, context.budget);
    }
    let ripemd = Address::from_word(U256::from(3).into());
    if payload.store.touched_in_transaction(ripemd) == Some(false) {
        return true;
    }
    if reserve_failure(payload, context.procedure, context.budget).is_none() {
        return refused(result, pc, context.budget);
    }
    // This borrowed candidate has no calls::failure input owner. Its input
    // copy remains separate from finish's completed-call/output snapshots.
    if payload.pay_clone_with(context.budget).is_err() {
        return refused(result, pc, context.budget);
    }
    let frontiers = result.frontiers.len();
    calls::finish(
        result,
        payload.clone(),
        OutcomeKind::Failure,
        EvmByteArray::<D::Element>::empty(),
        context,
        pc,
    );
    if result.frontiers.len() != frontiers {
        return false;
    }
    if context.budget.exhausted() {
        return refused(result, pc, context.budget);
    }
    true
}
