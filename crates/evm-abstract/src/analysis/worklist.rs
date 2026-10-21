//! Pending fixed-point obligations, independent of node identity and payload joins.
//!
//! The kernel owns removable FIFO records and pending membership. Satisfying a
//! node removes its live record; a later semantic change requeues it at a fresh
//! tail position. Scan and replay-copy charges depend only on live obligations.

use super::{FrontierReason, Limit, MachineError};
use crate::resource::WorkBudget;

pub(in crate::analysis) mod runtime;

/// Both fixed points admit a popped node before preparing its payload. Keep
/// cancellation/work exhaustion ahead of the transfer cap when both apply.
pub(super) fn transfer_stop(
    transfers: usize,
    max_transfers: usize,
    budget: &WorkBudget,
) -> Option<FrontierReason> {
    if transfers >= max_transfers || budget.exhausted() {
        Some(if budget.exhausted() {
            MachineError::work_failure().frontier_reason()
        } else {
            FrontierReason::Budget(Limit::Transfers)
        })
    } else {
        None
    }
}

/// Opaque borrowed transaction ledger used by production capability contexts.
/// Construction remains interpreter-owned; every component uses this same payer.
pub struct MachineMeter<'a>(pub(in crate::analysis) &'a mut WorkBudget);

impl analysis_kernel::Meter for MachineMeter<'_> {
    type Stop = MachineError;

    fn reserve(&mut self, cost: usize) -> Result<(), Self::Stop> {
        MachineError::check_cancelled()?;
        if cost == usize::MAX || !self.0.charge(cost) {
            Err(MachineError::work_failure())
        } else {
            Ok(())
        }
    }

    fn checkpoint(&self) -> Result<(), Self::Stop> {
        MachineError::check_cancelled()?;
        if self.0.exhausted() {
            Err(MachineError::work_failure())
        } else {
            Ok(())
        }
    }
}

/// Map protocol/resource failures without treating an interruption as bottom.
pub(super) fn kernel_failure(error: analysis_kernel::KernelError<MachineError>) -> MachineError {
    use analysis_kernel::KernelError;
    match error {
        KernelError::Stopped(stop) => stop,
        KernelError::CostOverflow => MachineError::work_failure(),
        KernelError::VersionExhausted => MachineError::IdentityExhausted,
        _ => MachineError::Shape,
    }
}

/// EVM compatibility view of the kernel's single FIFO/pending implementation.
#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Worklist(analysis_kernel::worklist::Worklist<usize>);

#[cfg(test)]
impl Worklist {
    pub(super) fn satisfy(&mut self, id: usize) {
        self.0.satisfy(id);
    }

    pub(super) fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.pending()
    }

    pub(super) fn pop_deepest(
        &mut self,
        budget: &mut WorkBudget,
        depth: impl Fn(usize) -> usize,
    ) -> Result<Option<usize>, MachineError> {
        self.0
            .pop_deepest(&mut MachineMeter(budget), depth)
            .map_err(kernel_failure)
    }
}

#[cfg(test)]
impl FromIterator<usize> for Worklist {
    fn from_iter<T: IntoIterator<Item = usize>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests;
