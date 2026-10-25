//! Meter the runner's report without charging callers of read-only introspection.

use super::{Kernel, Meter, reserve};
use crate::{
    KernelError, SolveReport,
    cost::{self, ReportCounts},
};

impl<J> Kernel<J> {
    pub(crate) fn solve_report<M: Meter>(
        &self,
        meter: &mut M,
        steps: usize,
    ) -> Result<SolveReport, KernelError<M::Stop>> {
        // len() is constant-time; pay before scanning facts to quote nested
        // reader traversal. Zero steps/ready jobs still consult the same meter.
        let metadata_work = cost::report_metadata_work(self.facts.len());
        reserve(meter, metadata_work)?;
        let (readers, coverage) =
            self.facts
                .iter()
                .try_fold((0usize, 0usize), |(readers, coverage), fact| {
                    Ok((
                        readers
                            .checked_add(fact.readers.len())
                            .ok_or(KernelError::CostOverflow)?,
                        coverage
                            .checked_add(fact.coverage.len())
                            .ok_or(KernelError::CostOverflow)?,
                    ))
                })?;
        let work = cost::report_work(ReportCounts {
            pending: self.worklist.len(),
            jobs: self.jobs.len(),
            facts: self.facts.len(),
            scopes: self.scopes.len(),
            readers,
            coverage,
        });
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
