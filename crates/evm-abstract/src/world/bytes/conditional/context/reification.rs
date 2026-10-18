//! Demand-local, prepaid temporary operands for the existing finite algorithm.

use super::memo::Demand;
use crate::domain::{AbstractValue, Domain, Profile};
use revm_bytecode::opcode;

/// None is an ordinary fallback only while the shared demand is still running.
/// The caller must recheck it before quoting or executing the original action.
pub(super) fn prepare(
    domain: Domain,
    op: u8,
    args: &[AbstractValue],
    demand: &Demand<'_>,
    mut check: impl FnMut() -> bool,
) -> Option<Vec<AbstractValue>> {
    // Guard length before any slice walk. These constant-size tag checks leave
    // unprepared/oracle, all-finite and unsupported operations on the old path.
    if !demand.is_budgeted()
        || domain.spec().profile() != Profile::Product
        || !(1..=3).contains(&args.len())
        || !matches!(op, 0x01..=0x0b | 0x10..=0x1e)
    {
        return None;
    }
    let arity = match op {
        opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
        opcode::ADDMOD | opcode::MULMOD => 3,
        _ => 2,
    };
    if args.len() != arity || args.iter().all(|value| value.constants().is_some()) {
        return None;
    }
    // One existing fixed-width candidate unit per operand funds span/product
    // arithmetic and these at most three stack-only descriptors before use.
    if !demand.pay_reader(|| args.len().saturating_mul(256)) {
        return None;
    }
    let mut plans = [None; 3];
    let mut tuples = 1usize;
    let mut construction = 0usize;
    let mut scratch = args.len();
    for (value, plan) in args.iter().zip(&mut plans) {
        scratch = scratch.saturating_add(value.work_size());
        let count = if let Some(values) = value.constants() {
            values.len()
        } else {
            let enumeration = value.numeric().complete_enumeration(domain.capacity())?;
            let count = enumeration.count();
            // New tree payload and its Arc/root shell have their own lifetime;
            // the original operand/metadata clone is counted separately above.
            scratch = scratch.saturating_add(count.saturating_add(1));
            construction = construction.saturating_add(enumeration.work());
            *plan = Some(enumeration);
            count
        };
        tuples = tuples.checked_mul(count)?;
        if tuples > domain.capacity() {
            return None;
        }
    }
    // Keep the existing scratch cap/refusal handling. Both argument snapshots
    // and owned sets prepay release, including empty/interrupted construction.
    if !demand.reserve_owned(scratch, scratch, 0) || !demand.pay_reader(|| construction) || !check()
    {
        return None;
    }
    let mut out = Vec::with_capacity(args.len());
    for (value, plan) in args.iter().zip(plans) {
        let temporary = if let Some(plan) = plan {
            let numeric = plan.build(&mut check)?;
            let mut temporary = value.clone().with_numeric(numeric);
            // Equivalent operands keep every metadata flag. with_numeric alone
            // would clear this marker when filtering exposes a singleton.
            if value.symbolic_limit_reached() {
                temporary = temporary.with_symbolic_limit();
            }
            temporary
        } else {
            value.clone()
        };
        out.push(temporary);
    }
    check().then_some(out)
}

#[cfg(test)]
mod tests;
