//! Bounded contexts of the most recent distinct jump sources.
//!
//! Transfer has already applied context_depth. Removing older occurrences
//! keeps recent private callsites without retaining one key per loop iteration.
//! The result never grows that bound. General paths can still have many source
//! permutations, so the usual state and work limits remain necessary.

use super::{BTreeSet, MachineError};

#[cfg(test)]
mod tests;

pub(in crate::analysis) fn work_size(length: usize) -> usize {
    let levels = usize::try_from(length.max(1).ilog2())
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    // Charge retained items and tree levels conservatively; these are logical
    // work units, not a measurement of machine comparisons or elapsed time.
    length.saturating_mul(levels).saturating_mul(8)
}

pub(in crate::analysis) fn canonical(history: &[usize]) -> Result<Vec<usize>, MachineError> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for source in history.iter().rev() {
        MachineError::check_cancelled()?;
        if seen.insert(*source) {
            output.push(*source);
        }
    }
    output.reverse();
    Ok(output)
}
