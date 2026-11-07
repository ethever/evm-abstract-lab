//! Translation admission for a proposed control plan, independent of region discovery.
//!
//! The checker interprets the plan's control skeleton and pairs every reachable
//! control state with an original CFG cursor. Bodies and simultaneous transfers
//! retain original identities; no numeric execution or planner summaries are
//! trusted. Pure syntax cycles are refused rather than treated as EVM progress.

use super::plan::{Plan, PlanError};
use crate::{EntityId, Ir, MAX_STRUCTURED_WIRE_NODES};
use std::collections::BTreeSet;

mod lower;
mod source;

#[cfg(test)]
mod tests;

use lower::{Action, Machine};
use source::{Cursor, Source};

const MAX_STATES: usize = MAX_STRUCTURED_WIRE_NODES;

fn paid(pay: &mut impl FnMut(usize) -> bool, amount: usize) -> Result<(), PlanError> {
    if pay(amount) {
        Ok(())
    } else {
        Err(PlanError::Limit)
    }
}

pub(super) fn verify(
    ir: &Ir,
    plan: &Plan,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<(), PlanError> {
    let source = Source::new(ir, pay)?;
    let machine = Machine::lower(plan, &source, pay)?;
    let mut pending = vec![(machine.entry, source.entry())];
    let mut seen = BTreeSet::new();
    let mut bodies = BTreeSet::new();
    while let Some((position, cursor)) = pending.pop() {
        paid(pay, 1)?;
        if !seen.insert((position, cursor)) {
            continue;
        }
        if seen.len() > MAX_STATES {
            return Err(PlanError::Limit);
        }
        match &machine.actions[position] {
            Action::Go(next) => pending.push((*next, cursor)),
            Action::End => {
                if cursor != Cursor::Empty {
                    return Err(PlanError::Invalid(cursor.entity()));
                }
            }
            Action::Body { block, next } => {
                if cursor != Cursor::Body(*block) {
                    return Err(PlanError::Invalid(*block));
                }
                bodies.insert(*block);
                pending.push((*next, Cursor::Terminator(*block)));
            }
            Action::Transfer { edge, next } => {
                source.transfer(cursor, *edge)?;
                pending.push((*next, Cursor::Body(source.edge(*edge)?.to)));
            }
            Action::Condition {
                condition,
                nonzero,
                zero,
            } => {
                let (yes, no) = source.condition(cursor, *condition)?;
                pending.push((*nonzero, yes));
                pending.push((*zero, no));
            }
            Action::Exit { block, partial } => source.exit(cursor, *block, *partial)?,
            Action::Indirect {
                block,
                arms,
                unresolved,
            } => {
                let choices = source.indirect(cursor, *block, arms, *unresolved, pay)?;
                pending.extend(choices);
            }
            Action::Dispatch { block, transitions } => {
                source.dispatch(cursor, *block, transitions, pay)?;
                pending.extend(
                    transitions
                        .iter()
                        .map(|(edge, next)| (*next, Cursor::Edge(*edge))),
                );
            }
        }
    }
    // This is a secondary completeness check. The product above, rather than
    // this set equality, rejects wrong order, skipped decisions and extra paths.
    if bodies != source.reachable(pay)? {
        return Err(PlanError::Invalid(ir.entry.unwrap_or(EntityId(0))));
    }
    Ok(())
}
