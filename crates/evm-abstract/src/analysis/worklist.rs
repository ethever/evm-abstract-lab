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
        crate::resource::clone_payment::pay(self.0, cost).map_err(MachineError::from)
    }

    fn checkpoint(&self) -> Result<(), Self::Stop> {
        crate::resource::clone_payment::checkpoint(self.0).map_err(MachineError::from)
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

#[cfg(test)]
mod tests;
