//! Exceptional gas termination can follow a sticky RIPEMD touch in any frame.
//!
//! CALL/CREATE end blocks, so their continuations expose new sticky touches.
//! SELFDESTRUCT also has an instruction-local failure point: its host effects
//! precede the dynamic gas charge. Both envelopes retain normal execution.

use super::{Execution, ExecutionFrontier, TransferContext, WorkBudget, boundary_reason, calls};
use crate::{
    analysis::{FrontierReason, MachinePayload, OutcomeKind},
    domain::relational::Constraint,
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

fn array_scan(array: &ByteArray) -> usize {
    // These counts are cached. work_size itself walks sparse metadata and the
    // retained byte graph to discover exact buffers, so its scan is prepaid.
    tree_work(
        array
            .metadata_len()
            .saturating_add(array.state_root().work_size()),
    )
    .saturating_mul(2)
}

fn store_work(store: &Store, budget: &mut WorkBudget) -> Option<usize> {
    let entries = store.finalization_entries();
    reserve(budget, tree_work(entries).saturating_mul(2))?;
    for log in store.possible_logs().values() {
        reserve(
            budget,
            array_scan(&log.data).saturating_add(log.topics.len()),
        )?;
    }
    // Code bytes remain a linear retained-data cost, never an account count.
    Some(store.work_size().saturating_add(tree_work(entries)))
}

fn reserve_failure(
    payload: &MachinePayload,
    procedure: bool,
    budget: &mut WorkBudget,
) -> Option<()> {
    let mut work = store_work(&payload.store, budget)?;
    if procedure || payload.call_stack.active_child().is_some() {
        // Relative roots and children restore here; transaction roots already
        // pay checkpoint restoration inside checked finalization.
        work = work.saturating_add(store_work(payload.active().saved_store.state(), budget)?);
    }
    reserve(budget, payload.call_stack.depth().saturating_mul(8))?;
    for frame in payload.call_stack.iter() {
        reserve(budget, frame.stack.len().saturating_add(4))?;
        work = frame.stack.iter().fold(
            work.saturating_add(frame.key.jump_history.len())
                .saturating_add(frame.call_value.work_size())
                .saturating_add(frame.address_operand.as_ref().map_or(0, |v| v.work_size()))
                .saturating_add(frame.caller_operand.as_ref().map_or(0, |v| v.work_size()))
                .saturating_add(32),
            |work, value| work.saturating_add(value.work_size()),
        );
        for array in [&frame.memory, &frame.calldata, &frame.returndata] {
            reserve(budget, array_scan(array))?;
            work = work.saturating_add(array.work_size());
        }
        // Programs and saved checkpoints clone shared immutable handles.
    }
    for child in payload.call_stack.children() {
        work = work
            .saturating_add(child.continuation.output_offset.work_size())
            .saturating_add(child.continuation.output_size.work_size());
    }
    reserve(
        budget,
        payload.relations.len().saturating_add(1).saturating_mul(8),
    )?;
    work = work.saturating_add(payload.relations.work_size());
    for constraint in payload.relations.constraints() {
        if let Constraint::Members { values, .. } = constraint {
            work = work.saturating_add(values.len());
        }
    }
    // Prepay failure's clone, finish's child-completion snapshot/output clone,
    // relative/child rollback, caller normalization, and their drop/error paths.
    // Checked root lifecycle and output-copy work stay owned by finish.
    reserve(budget, work.saturating_mul(8))
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
