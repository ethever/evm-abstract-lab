//! Borrowed final validation, separate from construction/observation work.

use super::{Error, WorkBudget, checkpoint, pay};
use crate::domain::symbolic::{ByteStateId, ByteStateKind, ExprId};
use std::collections::BTreeSet;

// A fixed upper bound for a cached check: eight iterator/option/flag steps,
// eight cached metric/policy field reads, eight cancellation/bound comparisons,
// and eight error/return/temporary cleanup steps. The same bound covers the
// simpler root and conditional checks. No expression children or native N.
pub(super) const METADATA_WORK: usize = 8 + 8 + 8 + 8;

fn set_work(entries: usize) -> usize {
    // The pinned std B-tree has at most 11 keys/12 edges per node. A binary
    // height bound plus two covers the insertion path and a new split root.
    // Per level: 11 comparisons, two full key/edge moves (shift/split), node
    // allocation+release and one full node's slot lifetime. Charge even on hits;
    // allocation and final teardown are thus funded before any insertion.
    let levels = (entries.saturating_add(1).ilog2() as usize).saturating_add(2);
    levels.saturating_mul(11 + 2 * (11 + 12) + 2 + 11 + 12)
}

fn push<'a>(
    pending: &mut Vec<&'a ByteStateId>,
    state: &'a ByteStateId,
    budget: &mut WorkBudget,
) -> Result<(), Error> {
    // Edge reference, push, eventual pop and reference cleanup, also on error.
    pay(budget, 4)?;
    if pending.len() == pending.capacity() {
        let extra = pending.capacity().max(4);
        let capacity = pending.capacity().saturating_add(extra);
        // Relocate live references, allocate new slots and release the buffer.
        // reserve_exact keeps the pinned Vec's requested reference capacity.
        pay(
            budget,
            pending.len().saturating_add(capacity).saturating_add(2),
        )?;
        pending.reserve_exact(extra);
    }
    pending.push(state);
    checkpoint(budget)?;
    Ok(())
}

pub(super) fn expressions<'a>(
    root: &'a ByteStateId,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&'a ExprId) -> Result<(), Error>,
) -> Result<(), Error> {
    // Empty containers allocate nothing. They own only references and u64 ids;
    // early refusal cannot drop an expression, byte state or native payload.
    pay(budget, 4)?;
    let mut pending = Vec::new();
    let mut seen = BTreeSet::new();
    push(&mut pending, root, budget)?;
    while !pending.is_empty() {
        pay(budget, set_work(seen.len()).saturating_add(8))?;
        let state = pending.pop().expect("nonempty pending states");
        if !seen.insert(state.id()) {
            continue;
        }
        let mut expression = |expression: &'a ExprId| {
            pay(budget, METADATA_WORK)?;
            visit(expression)
        };
        match state.kind() {
            ByteStateKind::Zero | ByteStateKind::Exact(_) => {}
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => {
                expression(source.token())?;
                expression(length)?;
                for binding in bindings {
                    expression(&binding.value)?;
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            }
            | ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                expression(offset)?;
                expression(value)?;
                push(&mut pending, previous, budget)?;
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                expression(destination)?;
                expression(&source.offset)?;
                expression(&source.length)?;
                expression(length)?;
                push(&mut pending, previous, budget)?;
                push(&mut pending, &source.state, budget)?;
            }
            ByteStateKind::Slice { source } => {
                expression(&source.offset)?;
                expression(&source.length)?;
                push(&mut pending, &source.state, budget)?;
            }
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                expression(condition)?;
                push(&mut pending, then_state, budget)?;
                push(&mut pending, else_state, budget)?;
            }
        }
    }
    checkpoint(budget)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{CloneError, SummaryError};

    #[test]
    fn refused_pending_growth_keeps_an_unallocated_empty_vector() {
        let root = ByteStateId::zero();
        for maximum in [0, 4, 9] {
            let mut pending = Vec::new();
            let mut budget = WorkBudget::new(maximum);
            assert_eq!(
                push(&mut pending, &root, &mut budget),
                Err(SummaryError::Budget(CloneError::Work))
            );
            assert_eq!(pending.capacity(), 0);
            assert!(pending.is_empty());
        }
    }
}
