//! Meter the runner's report without charging callers of read-only introspection.

use super::{Kernel, Meter, reserve};
use crate::{KernelError, SolveReport};

// Logical visits bound pending membership, both job scans, facts, scopes,
// obligations and nested reader scans, including scope/job metadata lookups.
fn traversal_work(counts: [usize; 6]) -> Option<usize> {
    counts
        .into_iter()
        .zip([4, 8, 6, 4, 5, 6])
        .try_fold(16usize, |work, (count, visits)| {
            work.checked_add(count.checked_mul(visits)?)
        })
}

impl<J> Kernel<J> {
    pub(crate) fn solve_report<M: Meter>(
        &self,
        meter: &mut M,
        steps: usize,
    ) -> Result<SolveReport, KernelError<M::Stop>> {
        // len() is constant-time; pay before scanning facts to quote nested
        // reader traversal. Zero steps/ready jobs still consult the same meter.
        let metadata_work = self
            .facts
            .len()
            .checked_add(1)
            .ok_or(KernelError::CostOverflow)?;
        reserve(meter, metadata_work)?;
        let readers = self.facts.iter().try_fold(0usize, |count, fact| {
            count
                .checked_add(fact.readers.len())
                .ok_or(KernelError::CostOverflow)
        })?;
        let work = traversal_work([
            self.worklist.len(),
            self.jobs.len(),
            self.facts.len(),
            self.scopes.len(),
            self.obligations.len(),
            readers,
        ])
        .ok_or(KernelError::CostOverflow)?;
        reserve(meter, work)?;
        let report = SolveReport {
            closure: self.closure(),
            local_limits: self.local_limits(),
            steps,
        };
        meter.checkpoint().map_err(KernelError::Stopped)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::traversal_work;

    #[test]
    fn report_work_rejects_overflow_in_counts_and_accumulation() {
        assert!(traversal_work([0; 6]).is_some());
        assert_eq!(traversal_work([0, 0, usize::MAX, 0, 0, 0]), None);
        assert_eq!(traversal_work([usize::MAX / 4, 1, 0, 0, 0, 0]), None);
    }
}
